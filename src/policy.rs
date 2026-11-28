use crate::syntax::{Selector, SyntaxMatcher};
use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Deserializer};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::io::Read;

pub const MAX_FILE: usize = 1024 * 1024;
const MAX_POLICY: usize = 1024 * 1024;
const MAX_RULES: usize = 128;
const MAX_PROVENANCE_URL: usize = 2048;
pub type Result<T> = std::result::Result<T, String>;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Policy {
    version: u32,
    rules: Vec<Rule>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Rule {
    id: String,
    kind: Kind,
    paths: Vec<String>,
    #[serde(default, deserialize_with = "present")]
    exclude: Option<Vec<String>>,
    #[serde(default, deserialize_with = "present")]
    anchors: Option<Vec<String>>,
    remediation: String,
    #[serde(default, deserialize_with = "present")]
    pattern: Option<Pattern>,
    #[serde(default, deserialize_with = "present")]
    requires: Option<Pattern>,
    #[serde(default, deserialize_with = "present")]
    selector: Option<Selector>,
    #[serde(default, deserialize_with = "present")]
    provenance: Option<Provenance>,
    fixtures: Vec<FixtureSource>,
}

/// Where a rule came from. It is validated and then dropped: matchers, fixtures
/// and finding output ignore it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Provenance {
    finding: String,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum Kind {
    Text,
    ForbiddenPath,
    Syntax,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
enum Pattern {
    Literal { value: String },
    Regex { value: String },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FixtureSource {
    path: String,
    #[serde(default, deserialize_with = "present")]
    content: Option<String>,
    #[serde(default, deserialize_with = "present")]
    expect: Option<Option<Location>>,
    #[serde(default, rename = "expectAll", deserialize_with = "present")]
    expect_all: Option<Vec<Location>>,
}

pub struct Fixture {
    pub path: String,
    pub content: Option<String>,
    pub expect: Expect,
}

/// A fixture asserts either the first match (`expect`, the v1 format) or every
/// match in all-match order (`expectAll`, text rules only).
pub enum Expect {
    First(Option<Location>),
    All(Vec<Location>),
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd)]
#[serde(deny_unknown_fields)]
pub struct Location {
    pub line: usize,
    pub column: usize,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

pub struct CompiledRule {
    pub id: String,
    pub remediation: String,
    pub scope: GlobSet,
    pub exclude: GlobSet,
    /// Forbidden-path rules only: globs for files that must exist for an
    /// absent forbidden path to count as clean. `None` keeps the v1 default,
    /// where no matching file is always a pass.
    pub anchors: Option<GlobSet>,
    pub matcher: Matcher,
    /// Digest of what decides the matches inside one file (see `matcher_digest`).
    /// A baseline entry recorded under another digest is incompatible.
    pub matcher_digest: String,
    pub fixtures: Vec<Fixture>,
}

pub enum Matcher {
    Literal(String),
    Regex(Regex),
    ForbiddenPath,
    Syntax(SyntaxMatcher),
    /// A text rule with `requires`: the trigger's matches are findings only
    /// when the requirement matches nowhere in the same file.
    Requires {
        trigger: Box<Matcher>,
        requirement: Box<Matcher>,
    },
}

pub(crate) fn present<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> std::result::Result<Option<T>, D::Error> {
    T::deserialize(deserializer).map(Some)
}

pub fn valid_path(path: &str) -> bool {
    path.len() <= 4096
        && !path.contains('\\')
        && !path.chars().any(char::is_control)
        && path
            .split('/')
            .all(|part| !matches!(part, "" | "." | ".." | ".git"))
}

/// Rule IDs are slash-separated parts of lowercase letters, digits and hyphens.
pub fn valid_rule_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id.split('/').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
        })
}

/// `sha256:` over the canonical JSON of a rule's `kind`, `pattern`,
/// `requires` and `selector`, which decide what matches inside one file. The ID, scope,
/// remediation, provenance and fixtures are left out: none of them changes the
/// count in a file that stays in scope, and a file that enters the scope has
/// no baseline allowance. The `counting` version changes whenever Rulecheck's
/// all-match semantics do, so old baselines stop matching.
fn matcher_digest(rule: &Value) -> String {
    let mut semantics = serde_json::Map::new();
    semantics.insert("counting".into(), Value::from(1));
    for key in ["kind", "pattern", "requires", "selector"] {
        if let Some(value) = rule.get(key) {
            semantics.insert(key.into(), value.clone());
        }
    }
    let mut text = String::new();
    canonical(&Value::Object(semantics), &mut text);
    format!("sha256:{:x}", Sha256::digest(text.as_bytes()))
}

/// Compact JSON with object keys sorted at every level, independent of how
/// `serde_json` orders maps.
fn canonical(value: &Value, out: &mut String) {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<_> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (index, key) in keys.into_iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                out.push_str(&Value::from(key.as_str()).to_string());
                out.push(':');
                canonical(&map[key], out);
            }
            out.push('}');
        }
        Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                canonical(item, out);
            }
            out.push(']');
        }
        scalar => out.push_str(&scalar.to_string()),
    }
}

pub fn bounded_read(reader: impl Read, limit: usize) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(std::io::Error::other(format!(
            "input exceeds {limit} bytes"
        )));
    }
    Ok(bytes)
}

pub fn read(reader: impl Read) -> Result<Vec<u8>> {
    bounded_read(reader, MAX_POLICY).map_err(|err| format!("policy stdin: {err}"))
}

pub fn load(bytes: &[u8]) -> Result<Vec<CompiledRule>> {
    let policy: Policy =
        serde_json::from_slice(bytes).map_err(|err| format!("invalid policy JSON: {err}"))?;
    if policy.version != 1 {
        return Err(format!(
            "unsupported policy version {}; use version 1",
            policy.version
        ));
    }
    if !(1..=MAX_RULES).contains(&policy.rules.len()) {
        return Err(format!("policy must have 1 to {MAX_RULES} rules"));
    }

    // The typed parse above accepted the document, so the raw rules line up with it.
    let raw: Value =
        serde_json::from_slice(bytes).map_err(|err| format!("invalid policy JSON: {err}"))?;
    let raw = raw["rules"].as_array().ok_or("invalid policy JSON")?;

    let mut ids = BTreeSet::new();
    let mut compiled = Vec::with_capacity(policy.rules.len());
    for (rule, raw) in policy.rules.into_iter().zip(raw) {
        if !valid_rule_id(&rule.id) {
            return Err(format!(
                "invalid rule ID {:?}; use lowercase letters, digits, hyphens and slashes",
                rule.id
            ));
        }
        if !ids.insert(rule.id.clone()) {
            return Err(format!("[{}] duplicate rule ID", rule.id));
        }
        let id = rule.id.clone();
        let mut rule = rule.compile().map_err(|err| format!("[{id}] {err}"))?;
        rule.matcher_digest = matcher_digest(raw);
        compiled.push(rule);
    }
    compiled.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(compiled)
}

impl Rule {
    fn compile(self) -> Result<CompiledRule> {
        if self.remediation.trim().is_empty()
            || self.remediation.len() > 512
            || self.remediation.chars().any(char::is_control)
        {
            return Err("provide a single-line remediation up to 512 bytes".into());
        }

        if let Some(provenance) = &self.provenance
            && !valid_provenance_url(&provenance.finding)
        {
            return Err(format!(
                "provenance.finding must be an https:// URL of at most {MAX_PROVENANCE_URL} characters, without spaces"
            ));
        }

        if !(1..=16).contains(&self.paths.len()) {
            return Err("provide 1 to 16 path globs".into());
        }
        let scope = globs("path", &self.paths)?;
        let exclude = match &self.exclude {
            None => GlobSet::empty(),
            Some(exclude) if (1..=16).contains(&exclude.len()) => globs("exclude", exclude)?,
            Some(_) => return Err("provide 1 to 16 exclude globs or omit exclude".into()),
        };
        let anchors = match (&self.anchors, self.kind) {
            (None, _) => None,
            (Some(_), Kind::Text | Kind::Syntax) => {
                return Err(
                    "anchors apply to forbidden-path rules only; text and syntax rules \
                     already exit 2 when no file is in scope"
                        .into(),
                );
            }
            (Some(anchors), Kind::ForbiddenPath) if (1..=16).contains(&anchors.len()) => {
                Some(globs("anchor", anchors)?)
            }
            (Some(_), Kind::ForbiddenPath) => {
                return Err("provide 1 to 16 anchor globs or omit anchors".into());
            }
        };

        if self.requires.is_some() && !matches!(self.kind, Kind::Text) {
            return Err(
                "only text rules take requires; it names text that must appear in the same file"
                    .into(),
            );
        }
        let matcher = match (self.kind, self.pattern, self.selector) {
            (Kind::Text, Some(pattern), None) => {
                let trigger = Matcher::compile(pattern)?;
                match self.requires {
                    None => trigger,
                    Some(requires) => Matcher::Requires {
                        trigger: Box::new(trigger),
                        requirement: Box::new(Matcher::requirement(requires)?),
                    },
                }
            }
            (Kind::ForbiddenPath, None, None) => Matcher::ForbiddenPath,
            (Kind::Syntax, None, Some(selector)) => Matcher::Syntax(selector.compile()?),
            _ => {
                return Err(
                    "text rules require a pattern, syntax rules require a selector, \
                     forbidden-path rules omit both"
                        .into(),
                );
            }
        };

        if !(1..=64).contains(&self.fixtures.len()) {
            return Err("provide 1 to 64 fixtures".into());
        }
        let mut fixtures = Vec::with_capacity(self.fixtures.len());
        for fixture in self.fixtures {
            let path = fixture.path;
            if !valid_path(&path) {
                return Err(format!("invalid fixture path {path:?}"));
            }
            if matcher.needs_text() != fixture.content.is_some() {
                return Err(format!("{path}: provide content only for text fixtures"));
            }
            let expect = match (fixture.expect, fixture.expect_all) {
                (Some(expect), None) => Expect::First(expect),
                (None, Some(all)) => {
                    if let Some(reason) = matcher.all_matches_unsupported() {
                        return Err(format!(
                            "{path}: expectAll is not available because {reason}; use expect"
                        ));
                    }
                    if all.windows(2).any(|pair| pair[0] >= pair[1]) {
                        return Err(format!(
                            "{path}: list expectAll locations in ascending order without duplicates"
                        ));
                    }
                    Expect::All(all)
                }
                _ => {
                    return Err(format!(
                        "{path}: provide exactly one of expect or expectAll"
                    ));
                }
            };
            let locations = match &expect {
                Expect::First(location) => location.as_slice(),
                Expect::All(locations) => locations,
            };
            if locations
                .iter()
                .any(|location| location.line == 0 || location.column == 0)
            {
                return Err(format!("{path}: fixture uses one-based locations"));
            }
            // A forbidden path that is also an anchor could never be clean.
            if !locations.is_empty() && anchors.as_ref().is_some_and(|set| set.is_match(&path)) {
                return Err(format!(
                    "{path}: a forbidden path must not match the rule's anchors; anchor a file \
                     that should always exist next to it"
                ));
            }
            fixtures.push(Fixture {
                path,
                content: fixture.content,
                expect,
            });
        }

        Ok(CompiledRule {
            id: self.id,
            remediation: self.remediation,
            matcher,
            matcher_digest: String::new(),
            scope,
            exclude,
            anchors,
            fixtures,
        })
    }
}

/// Mirrors the provenance `pattern` and `maxLength` in schema/policy-v1.json: an
/// https:// URL with no whitespace or control characters, counted in Unicode scalars.
fn valid_provenance_url(url: &str) -> bool {
    url.strip_prefix("https://").is_some_and(|rest| {
        !rest.is_empty() && !rest.chars().any(|c| c.is_whitespace() || c.is_control())
    }) && url.chars().count() <= MAX_PROVENANCE_URL
}

fn globs(field: &str, patterns: &[String]) -> Result<GlobSet> {
    let mut set = GlobSetBuilder::new();
    for pattern in patterns {
        if !valid_path(pattern) || pattern.len() > 256 {
            return Err(format!(
                "invalid {field} glob {pattern:?}; use a relative slash-separated glob up to 256 bytes"
            ));
        }
        let glob = GlobBuilder::new(pattern)
            .literal_separator(true)
            .backslash_escape(false)
            .build()
            .map_err(|err| format!("invalid {field} glob: {err}"))?;
        set.add(glob);
    }
    set.build()
        .map_err(|err| format!("invalid {field} scope: {err}"))
}

impl Matcher {
    fn compile(pattern: Pattern) -> Result<Self> {
        let value = match &pattern {
            Pattern::Literal { value } | Pattern::Regex { value } => value,
        };
        if value.is_empty() || value.len() > 4096 {
            return Err("use a pattern of 1 to 4096 bytes".into());
        }
        match pattern {
            Pattern::Literal { value } => Ok(Self::Literal(value)),
            Pattern::Regex { value } => RegexBuilder::new(&value)
                .nest_limit(64)
                .size_limit(256 * 1024)
                .dfa_size_limit(128 * 1024)
                .build()
                .map(Self::Regex)
                .map_err(|err| format!("invalid regex: {err}")),
        }
    }

    /// A `requires` pattern. One that can match empty text would be met by
    /// every file, so the rule could never report and is refused.
    fn requirement(pattern: Pattern) -> Result<Self> {
        let matcher = Self::compile(pattern).map_err(|err| format!("requires: {err}"))?;
        if let Self::Regex(regex) = &matcher
            && regex.is_match("")
        {
            return Err(
                "requires: the regex can match empty text, so every file would meet the \
                 requirement and the rule could never report; match at least one character"
                    .into(),
            );
        }
        Ok(matcher)
    }

    /// Whether the text contains any match. Only text matchers are used as
    /// requirements.
    fn is_met(&self, content: &str) -> bool {
        match self {
            Self::Literal(value) => content.contains(value.as_str()),
            Self::Regex(regex) => regex.is_match(content),
            _ => unreachable!("requirements are literal or regex patterns"),
        }
    }

    pub fn needs_text(&self) -> bool {
        !matches!(self, Self::ForbiddenPath)
    }

    pub fn location(&self, path: &str, content: Option<&str>) -> Result<Option<Location>> {
        Ok(self.find(path, content)?.map(|(location, _)| location))
    }

    /// The first match as a one-based line and Unicode scalar column, plus its
    /// zero-based, half-open UTF-8 byte range. Forbidden paths have no range.
    /// Only a syntax rule can fail, when its file does not parse.
    pub fn find(
        &self,
        path: &str,
        content: Option<&str>,
    ) -> Result<Option<(Location, Option<Span>)>> {
        let Some(content) = content else {
            return Ok(matches!(self, Self::ForbiddenPath)
                .then_some((Location { line: 1, column: 1 }, None)));
        };
        let span = match self {
            Self::Literal(value) => content.find(value).map(|start| Span {
                start,
                end: start + value.len(),
            }),
            Self::Regex(regex) => regex.find(content).map(|found| Span {
                start: found.start(),
                end: found.end(),
            }),
            Self::Syntax(selector) => selector.find(path, content)?,
            Self::ForbiddenPath => return Ok(Some((Location { line: 1, column: 1 }, None))),
            Self::Requires {
                trigger,
                requirement,
            } => {
                // The requirement is searched only once the trigger matched.
                return Ok(trigger
                    .find(path, Some(content))?
                    .filter(|_| !requirement.is_met(content)));
            }
        };
        Ok(span.map(|span| (locate(content, span.start), Some(span))))
    }

    /// Why this matcher cannot report every match, or `None` when it can.
    pub fn all_matches_unsupported(&self) -> Option<&'static str> {
        match self {
            Self::Syntax(_) => Some(
                "syntax rules do not count matching nodes yet, and reporting only their first \
                 match would undercount",
            ),
            Self::ForbiddenPath => Some("forbidden-path rules report at most one finding per path"),
            Self::Requires { trigger, .. } => trigger.all_matches_unsupported(),
            Self::Literal(_) | Self::Regex(_) => None,
        }
    }

    /// Every match in all-match mode, in ascending byte order, stopping after
    /// `limit` matches. Literal matches are non-overlapping: the search resumes
    /// at the end of each match. Regex matches follow `Regex::find_iter`:
    /// leftmost-first and non-overlapping, empty matches included, except an
    /// empty match that starts where the previous match ended. A forbidden
    /// path is one finding. With `requires`, every trigger match is a finding
    /// when the requirement matches nowhere in the file, and none is when it
    /// does. Syntax rules are rejected before a scan starts.
    pub fn find_all(
        &self,
        path: &str,
        content: Option<&str>,
        limit: usize,
    ) -> Result<Vec<(Location, Option<Span>)>> {
        let spans: Vec<Span> = match (self, content) {
            (Self::Literal(value), Some(content)) => content
                .match_indices(value.as_str())
                .take(limit)
                .map(|(start, found)| Span {
                    start,
                    end: start + found.len(),
                })
                .collect(),
            (Self::Regex(regex), Some(content)) => regex
                .find_iter(content)
                .take(limit)
                .map(|found| Span {
                    start: found.start(),
                    end: found.end(),
                })
                .collect(),
            (
                Self::Requires {
                    trigger,
                    requirement,
                },
                Some(text),
            ) => {
                let found = trigger.find_all(path, content, limit)?;
                return Ok(match found.is_empty() || requirement.is_met(text) {
                    true => Vec::new(),
                    false => found,
                });
            }
            (Self::Syntax(_), _) => {
                return Err(self.all_matches_unsupported().unwrap_or_default().into());
            }
            _ => return Ok(self.find(path, content)?.into_iter().collect()),
        };
        let content = content.unwrap_or_default();
        // Matches ascend, so lines and columns are counted in one pass.
        let (mut offset, mut line, mut column) = (0, 1, 1);
        Ok(spans
            .into_iter()
            .map(|span| {
                let skipped = &content[offset..span.start];
                match skipped.rfind('\n') {
                    Some(last) => {
                        line += skipped.bytes().filter(|byte| *byte == b'\n').count();
                        column = skipped[last + 1..].chars().count() + 1;
                    }
                    None => column += skipped.chars().count(),
                }
                offset = span.start;
                (Location { line, column }, Some(span))
            })
            .collect())
    }
}

/// The one-based line and Unicode scalar column of a byte offset.
fn locate(content: &str, start: usize) -> Location {
    let before = &content[..start];
    let line = before.bytes().filter(|byte| *byte == b'\n').count() + 1;
    let column = before
        .rsplit('\n')
        .next()
        .unwrap_or_default()
        .chars()
        .count()
        + 1;
    Location { line, column }
}

impl CompiledRule {
    pub fn applies_to(&self, path: &str) -> bool {
        self.scope.is_match(path) && !self.exclude.is_match(path)
    }

    /// Whether `path` is one of the files that show the guarded area exists.
    pub fn anchored_by(&self, path: &str) -> bool {
        self.anchors.as_ref().is_some_and(|set| set.is_match(path))
    }
}
