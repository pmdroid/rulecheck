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
