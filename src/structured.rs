//! The `structured` rule kind: a data-only path and one assertion evaluated
//! over a parsed YAML document. Nothing in the file is executed or expanded:
//! the pinned parser emits events, Rulecheck builds a bounded tree from them,
//! and the path walks that tree.
//!
//! Semantics (also in docs/src/content/docs/authoring.md):
//! - Every document of a multi-document file is checked on its own.
//! - `key` selects a mapping value by its scalar key text, `index` a sequence
//!   item, `each` every mapping value or every sequence item. A step that does
//!   not fit the node (missing key, scalar, out-of-range index) selects nothing.
//! - `present` reports a document whose path selects nothing, at the deepest
//!   node the path reached. `absent`, `equals`, `not-equals` and `has-key`
//!   report each selected node, so a missing path reports nothing for them.
//! - Scalars compare by their text after unquoting; tags and quoting style are
//!   ignored and no type resolution happens (`true`, `"true"` and `'true'` are
//!   all the text "true").
//! - A file that does not parse, has a duplicate key in any mapping, nests
//!   deeper or holds more nodes than the limits cannot be judged. So can a
//!   path that reaches an alias or looks up keys in a mapping with a `<<`
//!   merge key, because Rulecheck does not expand either.
use crate::policy::{Result, Span};
use saphyr_parser::{Event, Parser};
use serde::Deserialize;
use std::collections::HashSet;

const MAX_SEGMENTS: usize = 16;
const MAX_VALUE: usize = 1024;
pub const MAX_DEPTH: usize = 128;
pub const MAX_NODES: usize = 200_000;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Structure {
    format: Format,
    path: Vec<Segment>,
    assert: Assert,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum Format {
    Yaml,
}

#[derive(Deserialize)]
#[serde(try_from = "RawSegment")]
enum Segment {
    Key(String),
    Index(usize),
    Each(Each),
}

/// A segment as written: exactly one of `key`, `index` or `each`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSegment {
    #[serde(default, deserialize_with = "crate::policy::present")]
    key: Option<String>,
    #[serde(default, deserialize_with = "crate::policy::present")]
    index: Option<usize>,
    #[serde(default, deserialize_with = "crate::policy::present")]
    each: Option<Each>,
}

impl TryFrom<RawSegment> for Segment {
    type Error = &'static str;
    fn try_from(raw: RawSegment) -> std::result::Result<Self, Self::Error> {
        match (raw.key, raw.index, raw.each) {
            (Some(key), None, None) => Ok(Self::Key(key)),
            (None, Some(index), None) => Ok(Self::Index(index)),
            (None, None, Some(each)) => Ok(Self::Each(each)),
            _ => Err("a structure path segment has exactly one of key, index or each"),
        }
    }
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum Each {
    Mapping,
    Sequence,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
enum Assert {
    Present {},
    Absent {},
    Equals { value: String },
    NotEquals { value: String },
    HasKey { key: String },
}

pub struct StructuredMatcher {
    path: Vec<Segment>,
    assert: Assert,
}

impl Structure {
    pub fn compile(self) -> Result<StructuredMatcher> {
        let Format::Yaml = self.format;
        if self.path.len() > MAX_SEGMENTS {
            return Err(format!(
                "provide at most {MAX_SEGMENTS} structure path segments"
            ));
        }
        let text_ok = |text: &str| text.len() <= MAX_VALUE && !text.chars().any(char::is_control);
        for segment in &self.path {
            if let Segment::Key(key) = segment
                && !text_ok(key)
            {
                return Err(format!(
                    "structure path key {key:?} must be at most {MAX_VALUE} bytes without control characters"
                ));
            }
        }
        match &self.assert {
            Assert::Equals { value } | Assert::NotEquals { value } if !text_ok(value) => {
                return Err(format!(
                    "structure assert value must be at most {MAX_VALUE} bytes without control characters"
                ));
            }
            Assert::HasKey { key } if key.is_empty() || !text_ok(key) => {
                return Err(format!(
                    "structure assert key must be 1 to {MAX_VALUE} bytes without control characters"
                ));
            }
            Assert::Present {} | Assert::Absent {} if self.path.is_empty() => {
                return Err("present and absent assertions need at least one path segment".into());
            }
            _ => {}
        }
        Ok(StructuredMatcher {
            path: self.path,
            assert: self.assert,
        })
    }
}

enum Value {
    Scalar(String),
    Sequence(Vec<usize>),
    /// Key and value node pairs, in source order.
    Mapping(Vec<(usize, usize)>),
    Alias,
}

struct Node {
    value: Value,
    start: usize,
    end: usize,
    /// One-based line and column of `start`, for messages.
    line: usize,
    column: usize,
}

/// One parsed document: its nodes and the index of its root node.
struct Document {
    nodes: Vec<Node>,
    root: usize,
}

/// A selected node and where a finding about it points: the key that
/// selected it, or the node itself.
#[derive(Clone, Copy)]
struct Selected {
    node: usize,
    at: usize,
}

impl StructuredMatcher {
    /// Every finding in ascending byte order, at most `limit`.
    pub fn find_all(&self, path: &str, content: &str, limit: usize) -> Result<Vec<Span>> {
        if !(path.ends_with(".yml") || path.ends_with(".yaml")) {
            return Err(
                "structured yaml rules apply to .yml and .yaml files; narrow the path scope".into(),
            );
        }
        let documents = parse(content)?;
        let mut spans = Vec::new();
        if documents.is_empty() && matches!(self.assert, Assert::Present {}) {
            spans.push(Span { start: 0, end: 0 });
        }
        for document in &documents {
            self.check(document, &mut spans)?;
        }
        spans.sort();
        spans.dedup();
        spans.truncate(limit);
        Ok(spans)
    }

    fn check(&self, document: &Document, spans: &mut Vec<Span>) -> Result<()> {
        let nodes = &document.nodes;
        let mut current = vec![Selected {
            node: document.root,
            at: document.root,
        }];
        let mut deepest = document.root;
        for segment in &self.path {
            let mut next = Vec::new();
            for selected in &current {
                let node = &nodes[selected.node];
                match (segment, &node.value) {
                    (_, Value::Alias) => return Err(alias(node)),
                    (Segment::Key(key), Value::Mapping(pairs)) => {
                        no_merge(nodes, pairs, node)?;
                        if let Some(&(k, v)) = pairs.iter().find(|(k, _)| is_text(nodes, *k, key)) {
                            next.push(Selected { node: v, at: k });
                        }
                    }
                    (Segment::Index(index), Value::Sequence(items)) => {
                        if let Some(&item) = items.get(*index) {
                            next.push(Selected {
                                node: item,
                                at: item,
                            });
                        }
                    }
                    (Segment::Each(Each::Mapping), Value::Mapping(pairs)) => {
                        no_merge(nodes, pairs, node)?;
                        next.extend(pairs.iter().map(|&(k, v)| Selected { node: v, at: k }));
                    }
                    (Segment::Each(Each::Sequence), Value::Sequence(items)) => {
                        next.extend(items.iter().map(|&item| Selected {
                            node: item,
                            at: item,
                        }));
                    }
                    _ => {}
                }
            }
            if next.is_empty() {
                current = next;
                break;
            }
            deepest = next[0].node;
            current = next;
        }

        let span = |index: usize| Span {
            start: nodes[index].start,
            end: nodes[index].end,
        };
        match &self.assert {
            Assert::Present {} => {
                if current.is_empty() {
                    spans.push(span(deepest));
                }
            }
            Assert::Absent {} => spans.extend(current.iter().map(|selected| span(selected.at))),
            Assert::Equals { value } | Assert::NotEquals { value } => {
                let want = matches!(self.assert, Assert::Equals { .. });
                for selected in &current {
                    let node = &nodes[selected.node];
                    if let Value::Alias = node.value {
                        return Err(alias(node));
                    }
                    if is_text(nodes, selected.node, value) != want {
                        spans.push(span(selected.node));
                    }
                }
            }
            Assert::HasKey { key } => {
                for selected in &current {
                    let node = &nodes[selected.node];
                    match &node.value {
                        Value::Alias => return Err(alias(node)),
                        Value::Mapping(pairs) => {
                            no_merge(nodes, pairs, node)?;
                            if !pairs.iter().any(|(k, _)| is_text(nodes, *k, key)) {
                                spans.push(span(selected.at));
                            }
                        }
                        _ => spans.push(span(selected.at)),
                    }
                }
            }
        }
        Ok(())
    }
}

fn is_text(nodes: &[Node], index: usize, text: &str) -> bool {
    matches!(&nodes[index].value, Value::Scalar(value) if value == text)
}

fn alias(node: &Node) -> String {
    format!(
        "the structure path reaches a YAML alias at {}:{}; Rulecheck does not expand aliases, \
         so write the value out or narrow the path scope",
        node.line, node.column
    )
}

fn no_merge(nodes: &[Node], pairs: &[(usize, usize)], node: &Node) -> Result<()> {
    if pairs.iter().any(|(k, _)| is_text(nodes, *k, "<<")) {
        return Err(format!(
            "the structure path reaches a mapping with a << merge key at {}:{}; Rulecheck does \
             not expand merge keys, so write the keys out or narrow the path scope",
            node.line, node.column
        ));
    }
    Ok(())
}

/// A container being built: its node index and, for a mapping, the key
/// texts seen so far, so a duplicate key is found as soon as it appears.
struct Open {
    node: usize,
    keys: Option<HashSet<String>>,
    /// A mapping's pending key node, waiting for its value.
    key: Option<usize>,
}

/// Parses every document into a bounded tree, without recursion.
fn parse(content: &str) -> Result<Vec<Document>> {
    // The parser reports character offsets; findings use byte offsets.
    let bytes: Vec<usize> = content
        .char_indices()
        .map(|(byte, _)| byte)
        .chain([content.len()])
        .collect();
    let byte = |chars: usize| bytes[chars.min(bytes.len() - 1)];

    let mut documents = Vec::new();
    let mut nodes: Vec<Node> = Vec::new();
    let mut open: Vec<Open> = Vec::new();
    let mut root = None;
    let mut total = 0usize;

    for event in Parser::new_from_str(content) {
        let (event, span) = event.map_err(|err| {
            let at = err.marker();
            format!(
                "cannot parse as YAML near {}:{}: {}; fix the syntax or narrow the path scope",
                at.line(),
                at.col() + 1,
                err.info()
            )
        })?;
        let (start, end) = (byte(span.start.index()), byte(span.end.index()));
        let value = match event {
            Event::DocumentStart(_) => {
                nodes.clear();
                root = None;
                continue;
            }
            Event::DocumentEnd => {
                // A document without content still has an empty plain scalar
                // root from the parser, so `root` is always set here.
                if let Some(root) = root.take() {
                    documents.push(Document {
                        nodes: std::mem::take(&mut nodes),
                        root,
                    });
                }
                continue;
            }
            Event::SequenceEnd | Event::MappingEnd => {
                let closed = open.pop().ok_or("unbalanced YAML events")?;
                let last_end = match &nodes[closed.node].value {
                    Value::Sequence(items) => items.last().map(|&i| nodes[i].end),
                    Value::Mapping(pairs) => pairs.last().map(|&(_, v)| nodes[v].end),
                    _ => None,
                };
                let node = &mut nodes[closed.node];
                node.end = last_end.unwrap_or(end).max(node.start);
                continue;
            }
            Event::Scalar(text, ..) => Value::Scalar(text.into_owned()),
            Event::Alias(_) => Value::Alias,
            Event::SequenceStart(..) => Value::Sequence(Vec::new()),
            Event::MappingStart(..) => Value::Mapping(Vec::new()),
            Event::StreamStart | Event::StreamEnd | Event::Nothing => continue,
        };

        total += 1;
        if total > MAX_NODES {
            return Err(format!(
                "the YAML file has more than {MAX_NODES} nodes; narrow the path scope"
            ));
        }
        let is_container = matches!(value, Value::Sequence(_) | Value::Mapping(_));
        let is_mapping = matches!(value, Value::Mapping(_));
        let index = nodes.len();
        nodes.push(Node {
            value,
            start,
            end,
            line: span.start.line(),
            column: span.start.col() + 1,
        });

        match open.last_mut() {
            None => root = Some(index),
            Some(parent) => match &mut nodes[parent.node].value {
                Value::Sequence(items) => items.push(index),
                Value::Mapping(pairs) => match parent.key.take() {
                    None => {
                        if let Value::Scalar(text) = &nodes[index].value {
                            let keys = parent.keys.as_mut().expect("mappings track keys");
                            if !keys.insert(text.clone()) {
                                return Err(format!(
                                    "duplicate YAML key {text:?} at {}:{}; remove one of them, \
                                     since a tool may read either value",
                                    span.start.line(),
                                    span.start.col() + 1
                                ));
                            }
                        }
                        parent.key = Some(index);
                    }
                    Some(key) => pairs.push((key, index)),
                },
                _ => unreachable!("only containers are open"),
            },
        }
        if is_container {
            if open.len() >= MAX_DEPTH {
                return Err(format!(
                    "the YAML file nests deeper than {MAX_DEPTH} levels; narrow the path scope"
                ));
            }
            open.push(Open {
                node: index,
                keys: is_mapping.then(HashSet::new),
                key: None,
            });
        }
    }
    Ok(documents)
}
