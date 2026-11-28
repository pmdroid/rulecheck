//! Bounded cross-file consistency rules. A consistency rule reads a selected
//! inventory (the rule's `paths`, `exclude` and `excludeSets`), extracts
//! values from the files of one or two named scopes inside it with a regex,
//! and relates them in one of two fixed forms:
//!
//! - `same-values`: the values extracted from scope `left` and from scope
//!   `right` must be the same set.
//! - `path-exists`: every value extracted from scope `source` is a relative
//!   path that must name a file in the selected inventory.
//!
//! Nothing else is joined, no snippet is evaluated and no repository code
//! runs. Every input is bounded, and findings depend only on the inventory
//! paths and file contents, so a working-tree check and an exact-commit check
//! of the same files report the same findings.
use crate::policy::{self, Location, Result, Span};
use globset::GlobSet;
use regex::Regex;
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};

/// Extracted values one scope may produce across all its files.
pub const MAX_VALUES: usize = 10_000;
/// The longest extracted value, in bytes, after trimming.
pub const MAX_VALUE: usize = 1024;
const MAX_SCOPE_GLOBS: usize = 64;

#[derive(Deserialize)]
#[serde(tag = "form", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Source {
    SameValues {
        left: ScopeSource,
        right: ScopeSource,
    },
    PathExists {
        source: ScopeSource,
        base: Base,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScopeSource {
    name: String,
    paths: Vec<String>,
    #[serde(default, deserialize_with = "policy::present")]
    exclude: Option<Vec<String>>,
    extract: String,
}

/// What a `path-exists` reference is relative to: the repository root, or the
/// directory of the file it was extracted from.
#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Base {
    Root,
    File,
}

pub struct Scope {
    pub name: String,
    paths: GlobSet,
    exclude: Option<GlobSet>,
    extract: Regex,
}

pub enum Consistency {
    SameValues { left: Scope, right: Scope },
    PathExists { source: Scope, base: Base },
}

impl Source {
    /// Compiles the scopes and returns how many globs they add to the policy.
    pub fn compile(self) -> Result<(Consistency, usize)> {
        match self {
            Self::SameValues { left, right } => {
                let (left, a) = left.compile("left")?;
                let (right, b) = right.compile("right")?;
                if left.name == right.name {
                    return Err(format!(
                        "consistency.left and consistency.right are both named {:?}; give each scope its own name",
                        left.name
                    ));
                }
                Ok((Consistency::SameValues { left, right }, a + b))
            }
            Self::PathExists { source, base } => {
                let (source, count) = source.compile("source")?;
                Ok((Consistency::PathExists { source, base }, count))
            }
        }
    }
}

impl ScopeSource {
    fn compile(self, field: &str) -> Result<(Scope, usize)> {
        let context = |err: String| format!("consistency.{field}: {err}");
        if !policy::valid_name(&self.name) {
            return Err(context(format!(
                "invalid scope name {:?}; use lowercase letters, digits, hyphens and slashes",
                self.name
            )));
        }
        if !(1..=MAX_SCOPE_GLOBS).contains(&self.paths.len()) {
            return Err(context(format!(
                "provide 1 to {MAX_SCOPE_GLOBS} path globs"
            )));
        }
        let mut count = self.paths.len();
        let paths = policy::globs("path", &self.paths).map_err(context)?;
        let exclude = match &self.exclude {
            None => None,
            Some(exclude) if (1..=MAX_SCOPE_GLOBS).contains(&exclude.len()) => {
                count += exclude.len();
                Some(policy::globs("exclude", exclude).map_err(context)?)
            }
            Some(_) => {
                return Err(context(format!(
                    "provide 1 to {MAX_SCOPE_GLOBS} exclude globs or omit exclude"
                )));
            }
        };
        if self.extract.is_empty() || self.extract.len() > 4096 {
            return Err(context("use an extract regex of 1 to 4096 bytes".into()));
        }
        let extract = crate::dialect::compile(&self.extract)
            .map_err(|err| context(format!("invalid extract regex: {err}")))?
            .regex;
        if extract.captures_len() != 2 {
            return Err(context(
                "the extract regex must have exactly one capture group, around the value; \
                 write other groups as (?:...)"
                    .into(),
            ));
        }
        let scope = Scope {
            name: self.name,
            paths,
            exclude,
            extract,
        };
        Ok((scope, count))
    }
}

impl Scope {
    fn contains(&self, path: &str) -> bool {
        self.paths.is_match(path) && !self.exclude.as_ref().is_some_and(|set| set.is_match(path))
    }
}

impl Consistency {
    fn scopes(&self) -> Vec<&Scope> {
        match self {
            Self::SameValues { left, right } => vec![left, right],
            Self::PathExists { source, .. } => vec![source],
        }
    }

    /// Whether an inventory file must be read to extract values from it.
    pub fn reads(&self, path: &str) -> bool {
        self.scopes().iter().any(|scope| scope.contains(path))
    }
}

/// One extracted value and where it is.
struct Occurrence {
    value: String,
    path: String,
    location: Location,
    span: Span,
}

/// A consistency finding: where to fix it, and a message saying what is
/// inconsistent, escaped and capped like a rendered message template.
pub struct Hit {
    pub path: String,
    pub location: Location,
    pub span: Span,
    pub message: String,
}

/// Collects one rule's inventory and extracted values file by file, in
/// ascending path order, then relates them.
pub struct Accumulator<'r> {
    rule: &'r Consistency,
    inventory: BTreeSet<String>,
    occurrences: Vec<Vec<Occurrence>>,
    files: Vec<usize>,
}

impl<'r> Accumulator<'r> {
    pub fn new(rule: &'r Consistency) -> Self {
        let scopes = rule.scopes().len();
        Self {
            rule,
            inventory: BTreeSet::new(),
            occurrences: (0..scopes).map(|_| Vec::new()).collect(),
            files: vec![0; scopes],
        }
    }

    /// Adds a file of the selected inventory. `content` is present when the
    /// file is in a scope that extracts values.
    pub fn add(&mut self, path: &str, content: Option<&str>) -> Result<()> {
        self.inventory.insert(path.to_owned());
        for (index, scope) in self.rule.scopes().into_iter().enumerate() {
            if !scope.contains(path) {
                continue;
            }
            let content = content.ok_or_else(|| {
                format!(
                    "{path}: provide content for a file in scope {:?}",
                    scope.name
                )
            })?;
            self.files[index] += 1;
            let found = &mut self.occurrences[index];
            for captures in scope.extract.captures_iter(content) {
                let Some(group) = captures.get(1) else {
                    continue;
                };
                let text = group.as_str();
                let value = text.trim();
                if value.is_empty() {
                    continue;
                }
                let start = group.start() + (text.len() - text.trim_start().len());
                if value.len() > MAX_VALUE {
                    return Err(format!(
                        "{path}: scope {:?} extracted a value longer than {MAX_VALUE} bytes; \
                         tighten the extract regex",
                        scope.name
                    ));
                }
                if found.len() >= MAX_VALUES {
                    return Err(format!(
                        "scope {:?} extracted more than {MAX_VALUES} values; narrow its paths \
                         or tighten the extract regex",
                        scope.name
                    ));
                }
                found.push(Occurrence {
                    value: value.to_owned(),
                    path: path.to_owned(),
                    location: policy::locate(content, start),
                    span: Span {
                        start,
                        end: start + value.len(),
                    },
                });
            }
        }
        Ok(())
    }

    /// The names of scopes that matched no file. Such a rule cannot be judged.
    pub fn empty_scopes(&self) -> Vec<&'r str> {
        self.rule
            .scopes()
            .into_iter()
            .zip(&self.files)
            .filter(|(_, files)| **files == 0)
            .map(|(scope, _)| scope.name.as_str())
            .collect()
    }

    /// The findings, in no particular order; callers sort them.
    pub fn finish(self) -> Vec<Hit> {
        let mut occurrences = self.occurrences.into_iter();
        match self.rule {
            Consistency::SameValues { left, right } => {
                let left_found = occurrences.next().unwrap_or_default();
                let right_found = occurrences.next().unwrap_or_default();
                let mut hits = missing(&left_found, &right_found, left, right);
                hits.extend(missing(&right_found, &left_found, right, left));
                hits
            }
            Consistency::PathExists { base, .. } => occurrences
                .next()
                .unwrap_or_default()
                .into_iter()
                .filter_map(|occurrence| {
                    let value = crate::message::shown(&occurrence.value);
                    let message = match resolve(*base, &occurrence.path, &occurrence.value) {
                        Ok(target) if self.inventory.contains(&target) => return None,
                        Ok(target) if target == occurrence.value => format!(
                            "{} is not in the selected inventory.",
                            crate::message::shown(&target)
                        ),
                        Ok(target) => format!(
                            "\"{value}\" resolves to {}, which is not in the selected inventory.",
                            crate::message::shown(&target)
                        ),
                        Err(reason) => format!(
                            "\"{value}\" {reason}; a reference must name a file inside the repository."
                        ),
                    };
                    Some(Hit {
                        path: occurrence.path,
                        location: occurrence.location,
                        span: occurrence.span,
                        message: crate::message::capped(message),
                    })
                })
                .collect(),
        }
    }
}

/// Values of `from` missing from `other`, each reported once at its first
/// occurrence. Duplicates within a scope count as one value.
fn missing(
    from: &[Occurrence],
    other: &[Occurrence],
    name: &Scope,
    other_name: &Scope,
) -> Vec<Hit> {
    let present: BTreeSet<&str> = other.iter().map(|o| o.value.as_str()).collect();
    let mut first: BTreeMap<&str, &Occurrence> = BTreeMap::new();
    for occurrence in from {
        first.entry(occurrence.value.as_str()).or_insert(occurrence);
    }
    first
        .into_values()
        .filter(|occurrence| !present.contains(occurrence.value.as_str()))
        .map(|occurrence| Hit {
            path: occurrence.path.clone(),
            location: occurrence.location,
            span: occurrence.span,
            message: crate::message::capped(format!(
                "\"{}\" is in {} but not in {}.",
                crate::message::shown(&occurrence.value),
                name.name,
                other_name.name
            )),
        })
        .collect()
}

/// Resolves a reference lexically to a repository-relative path. Nothing on
/// disk is consulted, so symlinked directories never take part: the result is
/// only looked up in the inventory, which lists regular files alone.
pub fn resolve(
    base: Base,
    from: &str,
    reference: &str,
) -> std::result::Result<String, &'static str> {
    if reference.starts_with('/') {
        return Err("is absolute");
    }
    if reference.contains('\\') || reference.chars().any(char::is_control) {
        return Err("contains a backslash or control character");
    }
    let mut parts: Vec<&str> = match base {
        Base::Root => Vec::new(),
        Base::File => {
            let mut parts: Vec<&str> = from.split('/').collect();
            parts.pop();
            parts
        }
    };
    for part in reference.split('/') {
        match part {
            "" => return Err("has an empty path segment"),
            "." => {}
            ".." => {
                if parts.pop().is_none() {
                    return Err("climbs above the repository root");
                }
            }
            ".git" => return Err("names a .git directory"),
            part => parts.push(part),
        }
    }
    let target = parts.join("/");
    if target.is_empty() {
        return Err("names no file");
    }
    if !policy::valid_path(&target) {
        return Err("is not a valid repository path");
    }
    Ok(target)
}
