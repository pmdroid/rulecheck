//! The `syntax` rule kind: a data-only selector evaluated over a pinned
//! tree-sitter grammar. The selector names a node kind, an optional enclosing
//! node and field, and optional text filters that drop a node, so new rules need
//! no Rust changes. Node and field names are the grammar's own and are checked
//! against it when the policy loads. Matching is one pass over the tree, so
//! its cost grows with file size, not nesting depth.
use crate::policy::{Result, Span};
use regex::Regex;
use serde::Deserialize;
use std::collections::HashSet;
use tree_sitter::{Language, Node, Parser, Tree};

const MAX_NAMES: usize = 16;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Selector {
    language: LanguageName,
    node: String,
    #[serde(default, deserialize_with = "crate::policy::present")]
    inside: Option<Inside>,
    #[serde(default, deserialize_with = "crate::policy::present")]
    ignore: Option<Vec<TextFilter>>,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum LanguageName {
    Typescript,
    Rust,
    Go,
    Python,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Inside {
    kinds: Vec<String>,
    #[serde(default, deserialize_with = "crate::policy::present")]
    field: Option<String>,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
enum TextFilter {
    Literal { value: String },
    Regex { value: String },
}

pub struct SyntaxMatcher {
    language: LanguageName,
    node: String,
    inside: Option<(Vec<String>, Option<String>)>,
    ignore: Vec<Filter>,
    dialect_note: Option<String>,
}

enum Filter {
    Literal(String),
    Regex(Regex),
}

type Dialect = (&'static str, Language, &'static [&'static str]);

/// The grammars a language name covers, with the file extensions each parses.
fn dialects(language: LanguageName) -> Vec<Dialect> {
    match language {
        LanguageName::Typescript => vec![
            (
                "TypeScript",
                tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
                &[".ts", ".mts", ".cts"],
            ),
            (
                "TSX",
                tree_sitter_typescript::LANGUAGE_TSX.into(),
                &[".tsx"],
            ),
        ],
        LanguageName::Rust => vec![("Rust", tree_sitter_rust::LANGUAGE.into(), &[".rs"])],
        LanguageName::Go => vec![("Go", tree_sitter_go::LANGUAGE.into(), &[".go"])],
        LanguageName::Python => vec![(
            "Python",
            tree_sitter_python::LANGUAGE.into(),
            &[".py", ".pyi"],
        )],
    }
}

impl LanguageName {
    /// How messages name the language's grammars and the files they parse.
    fn describe(self) -> (&'static str, &'static str, &'static str) {
        match self {
            Self::Typescript => ("typescript", "TypeScript", ".ts, .mts, .cts and .tsx"),
            Self::Rust => ("rust", "Rust", ".rs"),
            Self::Go => ("go", "Go", ".go"),
            Self::Python => ("python", "Python", ".py and .pyi"),
        }
    }
}

fn known_kind(language: &Language, name: &str) -> bool {
    language.id_for_node_kind(name, true) != 0
}

impl Selector {
    pub fn compile(self) -> Result<SyntaxMatcher> {
        let grammars = dialects(self.language);
        let (_, family, _) = self.language.describe();
        let check_kind = |label: &str, name: &str| {
            if grammars
                .iter()
                .all(|(_, language, _)| known_kind(language, name))
            {
                Ok(())
            } else {
                Err(format!(
                    "selector {label} {name:?} is not a named node kind in every {family} grammar"
                ))
            }
        };
        check_kind("node", &self.node)?;

        let inside = match self.inside {
            None => None,
            Some(Inside { kinds, field }) => {
                if !(1..=MAX_NAMES).contains(&kinds.len()) {
                    return Err("provide 1 to 16 selector inside.kinds".into());
                }
                for kind in &kinds {
                    check_kind("inside kind", kind)?;
                }
                if let Some(field) = &field
                    && grammars
                        .iter()
                        .any(|(_, language, _)| language.field_id_for_name(field).is_none())
                {
                    return Err(format!(
                        "selector inside.field {field:?} is not a field in every {family} grammar"
                    ));
                }
                Some((kinds, field))
            }
        };

        let mut dialect_note = None;
        let ignore = match self.ignore {
            None => Vec::new(),
            Some(filters) if (1..=MAX_NAMES).contains(&filters.len()) => filters
                .into_iter()
                .map(|filter| {
                    let (filter, note) = filter.compile()?;
                    dialect_note = dialect_note.take().or(note);
                    Ok(filter)
                })
                .collect::<Result<_>>()?,
            Some(_) => return Err("provide 1 to 16 selector ignore filters or omit ignore".into()),
        };

        Ok(SyntaxMatcher {
            language: self.language,
            node: self.node,
            inside,
            ignore,
            dialect_note,
        })
    }
}

impl TextFilter {
    fn compile(self) -> Result<(Filter, Option<String>)> {
        let value = match &self {
            Self::Literal { value } | Self::Regex { value } => value,
        };
        if value.is_empty() || value.len() > 4096 {
            return Err("use a selector ignore value of 1 to 4096 bytes".into());
        }
        match self {
            Self::Literal { value } => Ok((Filter::Literal(value), None)),
            Self::Regex { value } => crate::dialect::compile(&value)
                .map(|compiled| (Filter::Regex(compiled.regex), compiled.note))
                .map_err(|err| format!("invalid selector ignore regex: {err}")),
        }
    }
}

impl Filter {
    fn matches(&self, text: &str) -> bool {
        match self {
            Self::Literal(value) => text.contains(value.as_str()),
            Self::Regex(regex) => regex.is_match(text),
        }
    }
}

impl SyntaxMatcher {
    /// How JavaScript would read the first ignore regex that it reads differently.
    pub fn dialect_note(&self) -> Option<&str> {
        self.dialect_note.as_deref()
    }

    /// The byte range of the first selected node in document order. A file the
    /// grammar cannot parse without errors cannot be judged.
    ///
    /// `trees` holds the file's parse trees by grammar, so every syntax rule
    /// that applies to one file shares a single parse. Pass a fresh cache for
    /// each file.
    pub fn find(&self, path: &str, content: &str, trees: &mut Trees) -> Result<Option<Span>> {
        let (name, language) = dialects(self.language)
            .into_iter()
            .find(|(_, _, extensions)| extensions.iter().any(|ext| path.ends_with(ext)))
            .map(|(name, language, _)| (name, language))
            .ok_or_else(|| {
                let (language, _, extensions) = self.language.describe();
                format!(
                    "syntax rules for {language} apply to {extensions} files; narrow the path scope"
                )
            })?;
        let tree = trees.parse(name, &language, content)?;
        let root = tree.root_node();

        // One pass in document order. Each frame on `path` records whether
        // its node lies inside a selected ancestor field, so a node's
        // ancestors are never walked again and deep trees stay linear.
        let mut cursor = root.walk();
        let mut path = vec![self.frame(root, self.inside.is_none())];
        loop {
            let node = cursor.node();
            let frame = path.last().expect("the path holds the current node");
            if frame.enclosed
                && node.is_named()
                && node.kind() == self.node
                && self.kept(node, content)
            {
                return Ok(Some(Span {
                    start: node.start_byte(),
                    end: node.end_byte(),
                }));
            }
            if cursor.goto_first_child() {
                let child = self.child_frame(&path, cursor.node());
                path.push(child);
                continue;
            }
            loop {
                if cursor.goto_next_sibling() {
                    path.pop();
                    let sibling = self.child_frame(&path, cursor.node());
                    path.push(sibling);
                    break;
                }
                if !cursor.goto_parent() {
                    return Ok(None);
                }
                path.pop();
            }
        }
    }

    /// The frame of a node whose own enclosure is already known. A node of an
    /// `inside` kind encloses all its children, or with a field only the
    /// children in that field.
    fn frame(&self, node: Node, enclosed: bool) -> Frame {
        let children = match &self.inside {
            Some((kinds, field)) if !enclosed && kinds.iter().any(|kind| kind == node.kind()) => {
                match field {
                    None => Children::All,
                    Some(field) => Children::Field(
                        node.children_by_field_name(field, &mut node.walk())
                            .map(|child| child.id())
                            .collect(),
                    ),
                }
            }
            _ => Children::None,
        };
        Frame { enclosed, children }
    }

    fn child_frame(&self, path: &[Frame], child: Node) -> Frame {
        let parent = path.last().expect("a child has a parent frame");
        let enclosed = parent.enclosed
            || match &parent.children {
                Children::None => false,
                Children::All => true,
                Children::Field(ids) => ids.contains(&child.id()),
            };
        self.frame(child, enclosed)
    }

    fn kept(&self, node: Node, content: &str) -> bool {
        let text = &content[node.byte_range()];
        !self.ignore.iter().any(|filter| filter.matches(text))
    }
}

/// The parse trees of one file, by grammar name. A tree with errors is kept as
/// its error, so later rules report the same position without parsing again.
#[derive(Default)]
pub struct Trees(Vec<(&'static str, std::result::Result<Tree, String>)>);

impl Trees {
    fn parse(&mut self, name: &'static str, language: &Language, content: &str) -> Result<&Tree> {
        let index = match self.0.iter().position(|(cached, _)| *cached == name) {
            Some(index) => index,
            None => {
                self.0.push((name, parse(name, language, content)));
                self.0.len() - 1
            }
        };
        self.0[index].1.as_ref().map_err(Clone::clone)
    }
}

fn parse(name: &str, language: &Language, content: &str) -> Result<Tree> {
    let mut parser = Parser::new();
    parser
        .set_language(language)
        .map_err(|err| format!("cannot load the {name} grammar: {err}"))?;
    let tree = parser
        .parse(content, None)
        .ok_or_else(|| format!("{name} parser returned no tree"))?;
    let root = tree.root_node();
    if root.has_error() {
        let at = first_error(root).unwrap_or(root).start_position();
        return Err(format!(
            "cannot parse as {name} near {}:{}; fix the syntax or narrow the path scope",
            at.row + 1,
            at.column + 1
        ));
    }
    Ok(tree)
}

struct Frame {
    /// The node lies within an `inside` ancestor (always, without `inside`).
    enclosed: bool,
    /// Which of the node's children an `inside` match encloses.
    children: Children,
}

enum Children {
    None,
    All,
    Field(HashSet<usize>),
}

/// The first error or missing node in document order, found without
/// recursion so a deeply nested file cannot exhaust the stack.
fn first_error(root: Node) -> Option<Node> {
    let mut node = root;
    'descend: loop {
        if node.is_error() || node.is_missing() {
            return Some(node);
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.has_error() {
                node = child;
                continue 'descend;
            }
        }
        return None;
    }
}

/// `ignoreInside` on a text rule: skip a match that lies wholly inside a
/// parsed context, such as a comment or string literal. Each match is judged
/// on its own, so an ignored occurrence never hides a later one, and the file
/// is parsed only once a candidate needs classifying.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IgnoreInside {
    language: ContextLanguage,
    contexts: Vec<ContextName>,
}

/// Languages whose contexts `ignoreInside` can classify.
#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum ContextLanguage {
    Typescript,
    /// Bash scripts. Only constructs whose reading by the pinned grammar is
    /// known to agree with bash are classified; see `bash_ranges`.
    Bash,
}

#[derive(Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
enum ContextName {
    /// TypeScript `//` and `/* */` comments; shell `#` comments.
    Comment,
    /// Quoted strings, and the literal parts of template strings. A `${...}`
    /// substitution is code, not string. In shell, `'...'`, `$'...'` and the
    /// literal parts of `"..."`; expansions and `$(...)` inside are code.
    String,
}

pub struct ContextFilter {
    language: ContextLanguage,
    comments: bool,
    strings: bool,
}

impl IgnoreInside {
    pub fn compile(self) -> Result<ContextFilter> {
        if !(1..=2).contains(&self.contexts.len()) {
            return Err("provide 1 or 2 ignoreInside contexts: \"comment\", \"string\"".into());
        }
        if self.contexts.len() == 2 && self.contexts[0] == self.contexts[1] {
            return Err("ignoreInside lists a context twice".into());
        }
        Ok(ContextFilter {
            language: self.language,
            comments: self.contexts.contains(&ContextName::Comment),
            strings: self.contexts.contains(&ContextName::String),
        })
    }
}

impl ContextLanguage {
    fn dialects(self) -> Vec<Dialect> {
        match self {
            Self::Typescript => dialects(LanguageName::Typescript),
            Self::Bash => vec![("Bash", tree_sitter_bash::LANGUAGE.into(), &[".sh", ".bash"])],
        }
    }

    fn describe(self) -> (&'static str, &'static str) {
        match self {
            Self::Typescript => ("typescript", LanguageName::Typescript.describe().2),
            Self::Bash => ("bash", ".sh and .bash"),
        }
    }
}

impl ContextFilter {
    /// The disjoint byte ranges of the file's ignored contexts, sorted by
    /// start. A file outside the grammar's extensions, or one that does not
    /// parse without errors, cannot be classified, so the match cannot be
    /// judged: that is an error, never a silent skip or a silent finding.
    pub fn ranges(&self, path: &str, content: &str, trees: &mut Trees) -> Result<Vec<Span>> {
        let (name, grammar) = self
            .language
            .dialects()
            .into_iter()
            .find(|(_, _, extensions)| extensions.iter().any(|ext| path.ends_with(ext)))
            .map(|(name, grammar, _)| (name, grammar))
            .ok_or_else(|| {
                let (language, extensions) = self.language.describe();
                format!(
                    "ignoreInside for {language} classifies {extensions} files only; narrow the path scope"
                )
            })?;
        let unclassified = |err: String| format!("ignoreInside cannot classify the match: {err}");
        if let ContextLanguage::Bash = self.language {
            bash_blanks(content).map_err(unclassified)?;
        }
        let tree = trees.parse(name, &grammar, content).map_err(unclassified)?;
        let mut ranges = match self.language {
            ContextLanguage::Typescript => self.typescript_ranges(tree),
            ContextLanguage::Bash => self.bash_ranges(tree, content).map_err(unclassified)?,
        };
        ranges.sort();
        Ok(ranges)
    }

    fn typescript_ranges(&self, tree: &Tree) -> Vec<Span> {
        // One pass in document order. A context node's children are skipped,
        // except a template string's, whose substitutions may hold more
        // contexts. Anonymous tokens are never contexts: the `string` type
        // keyword is an anonymous node of kind "string".
        let mut ranges = Vec::new();
        let mut cursor = tree.root_node().walk();
        'walk: loop {
            let node = cursor.node();
            let descend = match (node.is_named(), node.kind()) {
                (true, "comment") if self.comments => false,
                (true, "string") if self.strings => false,
                (true, "template_string") if self.strings => {
                    carve(node, &mut ranges, |child| {
                        child.kind() == "template_substitution"
                    });
                    true
                }
                _ => true,
            };
            if !descend {
                ranges.push(span(node));
            } else if cursor.goto_first_child() {
                continue;
            }
            loop {
                if cursor.goto_next_sibling() {
                    continue 'walk;
                }
                if !cursor.goto_parent() {
                    break 'walk;
                }
            }
        }
        ranges
    }

    /// The contexts of a Bash file. tree-sitter-bash is close to bash but not
    /// identical, and where they disagree the grammar can read code as a
    /// comment or string. Each construct where a disagreement was found is
    /// checked, and a file that has one cannot be classified:
    ///
    /// - a heredoc whose closing line bash would not accept (` EOF`, `EOF `,
    ///   a delimiter with `;` or other characters), which the grammar ends
    ///   early and reads the rest of the body as code;
    /// - a `#` that does not start a word for bash, such as one right after
    ///   a `\`-newline continuation, which the grammar reads as a comment;
    /// - a backquoted `` `...` `` substitution, which the grammar parses into
    ///   the same `command_substitution` node as `$(...)` although bash
    ///   re-reads its text with different quoting and escaping rules.
    ///
    /// Heredoc bodies are data, never contexts, so matches in them count.
    fn bash_ranges(&self, tree: &Tree, content: &str) -> Result<Vec<Span>> {
        let mut ranges = Vec::new();
        let mut last_comment_end = None;
        let mut cursor = tree.root_node().walk();
        'walk: loop {
            let node = cursor.node();
            let mut descend = true;
            if node.is_named() {
                match node.kind() {
                    "comment" => {
                        comment_starts_word(node, content, last_comment_end)?;
                        last_comment_end = Some(node.end_byte());
                        if self.comments {
                            ranges.push(span(node));
                        }
                        descend = false;
                    }
                    "raw_string" | "ansi_c_string" => {
                        if self.strings {
                            ranges.push(span(node));
                        }
                        descend = false;
                    }
                    "string" | "translated_string" if self.strings => {
                        carve(node, &mut ranges, |child| {
                            child.is_named() && child.kind() != "string_content"
                        });
                    }
                    "command_substitution" if content[node.byte_range()].starts_with('`') => {
                        return Err(format!(
                            "bash re-reads a backquoted `...` substitution with its own quoting rules near {}; write $(...) or narrow the path scope",
                            position(node)
                        ));
                    }
                    "heredoc_redirect" => heredoc_delimited(node, content)?,
                    _ => {}
                }
            }
            if descend && cursor.goto_first_child() {
                continue;
            }
            loop {
                if cursor.goto_next_sibling() {
                    continue 'walk;
                }
                if !cursor.goto_parent() {
                    break 'walk;
                }
            }
        }
        Ok(ranges)
    }
}

fn span(node: Node) -> Span {
    Span {
        start: node.start_byte(),
        end: node.end_byte(),
    }
}

fn position(node: Node) -> String {
    let at = node.start_position();
    format!("{}:{}", at.row + 1, at.column + 1)
}

/// Push the parts of `node` between the children that `code` selects. The
/// walk still descends into those children, which may hold more contexts.
fn carve(node: Node, ranges: &mut Vec<Span>, code: impl Fn(&Node) -> bool) {
    let mut start = node.start_byte();
    for child in node.children(&mut node.walk()) {
        if code(&child) {
            ranges.push(Span {
                start,
                end: child.start_byte(),
            });
            start = child.end_byte();
        }
    }
    ranges.push(Span {
        start,
        end: node.end_byte(),
    });
}

/// The grammar reads a carriage return, vertical tab or form feed as a blank,
/// but bash reads it as part of a word: `true\r# x; rm y` runs `rm y` while
/// the grammar sees a comment. A file with one cannot be classified.
fn bash_blanks(content: &str) -> Result<()> {
    match content.find(['\r', '\u{b}', '\u{c}']) {
        None => Ok(()),
        Some(at) => {
            let line = content[..at].bytes().filter(|byte| *byte == b'\n').count() + 1;
            let column = at - content[..at].rfind('\n').map_or(0, |newline| newline + 1) + 1;
            Err(format!(
                "bash reads a carriage return, vertical tab or form feed as part of a word near {line}:{column}; remove it or narrow the path scope"
            ))
        }
    }
}

/// Bash starts a comment only at a `#` that begins a word: at the start of
/// the file or after a blank, newline or one of `;&|()`. A blank escaped
/// with `\`, or a newline removed as a `\`-newline continuation, does not
/// end the word before it. A `\` at the end of a comment is part of the
/// comment, so the newline after it still ends a word.
fn comment_starts_word(node: Node, content: &str, last_comment_end: Option<usize>) -> Result<()> {
    let start = node.start_byte();
    let bytes = content.as_bytes();
    let starts = match start.checked_sub(1).map(|at| bytes[at]) {
        None => true,
        Some(b';' | b'&' | b'|' | b'(' | b')') => true,
        Some(b' ' | b'\t' | b'\n') => {
            start < 2 || bytes[start - 2] != b'\\' || last_comment_end == Some(start - 1)
        }
        Some(_) => false,
    };
    if starts {
        Ok(())
    } else {
        Err(format!(
            "bash does not start a comment at the # near {}; fix the line or narrow the path scope",
            position(node)
        ))
    }
}

/// A heredoc the grammar ends where bash ends it: a delimiter word of
/// letters, digits and `_` (bare, `\`-escaped, or in one pair of quotes),
/// closed by a line that is exactly the delimiter, after tabs only with
/// `<<-`, and no earlier body line equal to it. The grammar also ends a
/// heredoc at ` EOF`, `EOF ` or `EOF;`, which bash does not.
fn heredoc_delimited(node: Node, content: &str) -> Result<()> {
    let refuse = || {
        Err(format!(
            "bash may end this heredoc elsewhere than the bash grammar near {}; close it with a line that is exactly its delimiter (letters, digits, _) or narrow the path scope",
            position(node)
        ))
    };
    let mut strip_tabs = false;
    let (mut start, mut end) = (None, None);
    for child in node.children(&mut node.walk()) {
        match child.kind() {
            "<<-" => strip_tabs = true,
            "heredoc_start" => start = start.or(Some(child)),
            "heredoc_end" => end = Some(child),
            _ => {}
        }
    }
    let (Some(start), Some(end)) = (start, end) else {
        return refuse();
    };
    let quoted = &content[start.byte_range()];
    let word = quoted
        .strip_prefix('\\')
        .or_else(|| {
            quoted
                .strip_prefix('\'')
                .and_then(|word| word.strip_suffix('\''))
        })
        .or_else(|| {
            quoted
                .strip_prefix('"')
                .and_then(|word| word.strip_suffix('"'))
        })
        .unwrap_or(quoted);
    let bytes = content.as_bytes();
    let line_start = content[..end.start_byte()].rfind('\n').map(|at| at + 1);
    let closed = !word.is_empty()
        && word
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        && !end.is_missing()
        && &content[end.byte_range()] == word
        && matches!(bytes.get(end.end_byte()), None | Some(b'\n'))
        && line_start.is_some_and(|line_start| {
            let indent = &content[line_start..end.start_byte()];
            indent.is_empty() || (strip_tabs && indent.bytes().all(|byte| byte == b'\t'))
        });
    if !closed {
        return refuse();
    }
    // The body runs from the line after the redirection to the closing line.
    let body_start = content[start.end_byte()..]
        .find('\n')
        .map_or(content.len(), |at| start.end_byte() + at + 1);
    let body_end = line_start.unwrap_or_default();
    if body_start < body_end
        && content[body_start..body_end].lines().any(|line| {
            let line = if strip_tabs {
                line.trim_start_matches('\t')
            } else {
                line
            };
            line == word
        })
    {
        return refuse();
    }
    Ok(())
}

/// Whether `span` lies wholly inside one of the sorted, disjoint `ranges`. A
/// match that starts in a context and ends outside it is not ignored.
pub fn inside(ranges: &[Span], span: Span) -> bool {
    let after = ranges.partition_point(|range| range.start <= span.start);
    after > 0 && span.end <= ranges[after - 1].end
}
