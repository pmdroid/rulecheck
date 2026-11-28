//! Regex patterns are compiled in the Rust `regex` dialect, unchanged. Rules
//! are often ported from JavaScript, where some of the same text means
//! something else. This module compiles a pattern with the evaluator's limits
//! and, without changing what it matches, explains the JavaScript-only
//! constructs: a compile error gains a porting hint, and a pattern that is
//! valid in both dialects but can match differently gets a note that is shown
//! when one of its fixtures fails.
use regex::{Regex, RegexBuilder};
use regex_syntax::ast::{
    self, ClassSetBinaryOp, ClassSetBinaryOpKind, ClassSetItem, ErrorKind, Visitor,
    parse::ParserBuilder,
};

const NEST_LIMIT: u32 = 64;

/// A compiled pattern and, when it uses a nested character class or a class
/// set operator, a note on how JavaScript would read the same text.
pub struct Compiled {
    pub regex: Regex,
    pub note: Option<String>,
}

/// Compiles `value` with the evaluator's nesting, program and DFA limits. On
/// failure the error is the regex crate's message, plus a porting hint when the
/// construct is JavaScript-only.
pub fn compile(value: &str) -> Result<Compiled, String> {
    let regex = RegexBuilder::new(value)
        .nest_limit(NEST_LIMIT)
        .size_limit(256 * 1024)
        .dfa_size_limit(128 * 1024)
        .build()
        .map_err(|err| explain(value, err))?;
    // The pattern compiled, so it also parses; the walk is iterative.
    let note = ParserBuilder::new()
        .nest_limit(NEST_LIMIT)
        .build()
        .parse(value)
        .ok()
        .and_then(|ast| ast::visit(&ast, Classes::default()).ok())
        .and_then(Classes::note);
    Ok(Compiled { regex, note })
}

/// The regex crate's error for `value`, plus a porting hint when the pattern
/// fails on a JavaScript-only construct.
pub fn explain(value: &str, err: impl std::fmt::Display) -> String {
    match ParserBuilder::new()
        .nest_limit(NEST_LIMIT)
        .build()
        .parse(value)
    {
        Err(syntax) => match hint(value, &syntax) {
            Some(hint) => format!("{err}\nhint: {hint}"),
            None => err.to_string(),
        },
        Ok(_) => err.to_string(),
    }
}

fn hint(value: &str, error: &ast::Error) -> Option<&'static str> {
    let span = error.span();
    let text = value.get(span.start.offset..).unwrap_or_default();
    Some(match error.kind() {
        ErrorKind::UnsupportedLookAround => {
            "the Rust regex dialect has no look-around. Match a token edge with \\b, \\b{start} \
             or \\b{end}; consume the next character, as in name(?:[^_]|$), instead of a \
             look-ahead; or move the condition into paths, exclude or a syntax selector"
        }
        ErrorKind::UnsupportedBackreference => {
            "the Rust regex dialect has no backreferences. List each alternative instead, \
             as in (?:'x'|\"x\") for (['\"])x\\1"
        }
        ErrorKind::EscapeUnrecognized if text.starts_with("\\k") => {
            "the Rust regex dialect has no backreferences, so \\k<name> is not an escape. \
             List each alternative instead"
        }
        ErrorKind::ClassUnclosed => {
            "in the Rust regex dialect [ inside a character class opens a nested class, \
             while JavaScript reads it as a literal bracket. Escape a literal bracket as \\[, \
             and write JavaScript's [^] as (?s:.)"
        }
        _ => return None,
    })
}

/// Collects the class constructs whose meaning differs from JavaScript without
/// the `v` flag: nested classes and the `&&`, `--` and `~~` set operators.
#[derive(Default)]
struct Classes {
    nested: bool,
    operators: Vec<&'static str>,
}

impl Classes {
    fn note(self) -> Option<String> {
        let mut parts = Vec::new();
        if self.nested {
            parts.push("a nested character class".to_owned());
        }
        for operator in &self.operators {
            parts.push(format!("the {operator} set operator"));
        }
        if parts.is_empty() {
            return None;
        }
        let used = match parts.as_slice() {
            [one] => one.clone(),
            [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
            [] => unreachable!(),
        };
        let literal = match (self.nested, self.operators.is_empty()) {
            (true, true) => "[",
            (false, false) => "the operator characters",
            _ => "[ and the operator characters",
        };
        Some(format!(
            "This pattern uses {used} inside a character class, which the Rust regex dialect \
             applies as set operations but JavaScript without the v flag reads as literal \
             characters; to match {literal} literally, escape each one with a backslash"
        ))
    }
}

impl Visitor for Classes {
    type Output = Self;
    type Err = ();

    fn finish(self) -> Result<Self, ()> {
        Ok(self)
    }

    fn visit_class_set_item_pre(&mut self, item: &ClassSetItem) -> Result<(), ()> {
        if matches!(item, ClassSetItem::Bracketed(_)) {
            self.nested = true;
        }
        Ok(())
    }

    fn visit_class_set_binary_op_pre(&mut self, op: &ClassSetBinaryOp) -> Result<(), ()> {
        let operator = match op.kind {
            ClassSetBinaryOpKind::Intersection => "&&",
            ClassSetBinaryOpKind::Difference => "--",
            ClassSetBinaryOpKind::SymmetricDifference => "~~",
        };
        if !self.operators.contains(&operator) {
            self.operators.push(operator);
        }
        Ok(())
    }
}
