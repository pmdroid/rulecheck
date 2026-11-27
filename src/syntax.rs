//! The `syntax` rule kind: a data-only selector evaluated over a pinned
//! tree-sitter grammar. The selector names a node kind, an optional enclosing
//! node and field, and optional text filters that drop a node, so new rules need
//! no Rust changes. Node and field names are the grammar's own and are checked
//! against it when the policy loads.
use crate::policy::{Result, Span};
use regex::{Regex, RegexBuilder};
use serde::Deserialize;
use tree_sitter::{Language, Node, Parser};

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
}

enum Filter {
    Literal(String),
    Regex(Regex),
}

/// The grammars a language name covers, with the file extensions each parses.
fn dialects(language: LanguageName) -> [(&'static str, Language, &'static [&'static str]); 2] {
    match language {
        LanguageName::Typescript => [
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
    }
}

fn known_kind(language: &Language, name: &str) -> bool {
    language.id_for_node_kind(name, true) != 0
}

impl Selector {
    pub fn compile(self) -> Result<SyntaxMatcher> {
        let grammars = dialects(self.language);
        let check_kind = |label: &str, name: &str| {
            if grammars
                .iter()
                .all(|(_, language, _)| known_kind(language, name))
            {
                Ok(())
            } else {
                Err(format!(
                    "selector {label} {name:?} is not a named node kind in every TypeScript grammar"
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
                        "selector inside.field {field:?} is not a field in every TypeScript grammar"
                    ));
                }
                Some((kinds, field))
            }
        };

        let ignore = match self.ignore {
            None => Vec::new(),
            Some(filters) if (1..=MAX_NAMES).contains(&filters.len()) => filters
                .into_iter()
                .map(TextFilter::compile)
                .collect::<Result<_>>()?,
            Some(_) => return Err("provide 1 to 16 selector ignore filters or omit ignore".into()),
        };

        Ok(SyntaxMatcher {
            language: self.language,
            node: self.node,
            inside,
            ignore,
        })
    }
}

impl TextFilter {
    fn compile(self) -> Result<Filter> {
        let value = match &self {
            Self::Literal { value } | Self::Regex { value } => value,
        };
        if value.is_empty() || value.len() > 4096 {
            return Err("use a selector ignore value of 1 to 4096 bytes".into());
        }
        match self {
            Self::Literal { value } => Ok(Filter::Literal(value)),
            Self::Regex { value } => RegexBuilder::new(&value)
                .nest_limit(64)
                .size_limit(256 * 1024)
                .dfa_size_limit(128 * 1024)
                .build()
                .map(Filter::Regex)
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
    /// The byte range of the first selected node in document order. A file the
    /// grammar cannot parse without errors cannot be judged.
    pub fn find(&self, path: &str, content: &str) -> Result<Option<Span>> {
        let (name, language) = dialects(self.language)
            .into_iter()
            .find(|(_, _, extensions)| extensions.iter().any(|ext| path.ends_with(ext)))
            .map(|(name, language, _)| (name, language))
            .ok_or("syntax rules for typescript apply to .ts, .mts, .cts and .tsx files; narrow the path scope")?;
        let mut parser = Parser::new();
        parser
            .set_language(&language)
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

        let mut cursor = root.walk();
        loop {
            let node = cursor.node();
            if node.is_named() && node.kind() == self.node && self.selects(node, content) {
                return Ok(Some(Span {
                    start: node.start_byte(),
                    end: node.end_byte(),
                }));
            }
            if cursor.goto_first_child() || cursor.goto_next_sibling() {
                continue;
            }
            loop {
                if !cursor.goto_parent() {
                    return Ok(None);
                }
                if cursor.goto_next_sibling() {
                    break;
                }
            }
        }
    }

    fn selects(&self, node: Node, content: &str) -> bool {
        if let Some((kinds, field)) = &self.inside {
            let mut child = node;
            let enclosed = loop {
                let Some(parent) = child.parent() else {
                    break false;
                };
                if kinds.iter().any(|kind| kind == parent.kind())
                    && field.as_deref().is_none_or(|field| {
                        parent
                            .children_by_field_name(field, &mut parent.walk())
                            .any(|body| body.id() == child.id())
                    })
                {
                    break true;
                }
                child = parent;
            };
            if !enclosed {
                return false;
            }
        }
        let text = &content[node.byte_range()];
        !self.ignore.iter().any(|filter| filter.matches(text))
    }
}

fn first_error(node: Node) -> Option<Node> {
    if node.is_error() || node.is_missing() {
        return Some(node);
    }
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .filter(|child| child.has_error())
        .find_map(first_error)
}
