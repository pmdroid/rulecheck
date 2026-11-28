use crate::consistency::{self, Consistency};
use crate::dialect;
use crate::message::Template;
use crate::structured::{Structure, StructuredMatcher};
use crate::syntax::{Selector, SyntaxMatcher, Trees};
use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use regex::Regex;
use serde::{Deserialize, Deserializer};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;

pub const MAX_FILE: usize = 1024 * 1024;
/// Policy limits, kept equal to `maxPolicy` and `maxRules` in tools/build.mjs,
/// `maxPolicy` in bin/rulecheck.mjs and `maxItems` in schema/policy-v1.json.
const MAX_POLICY: usize = 4 * 1024 * 1024;
const MAX_RULES: usize = 1024;
/// Inline `paths` and `exclude` globs per rule. Measured with globset 0.4.22, a
/// set's match cost grows about linearly with its glob count (about 0.16 µs per
/// path at 16 globs, 0.28 µs at 64 and 0.86 µs at 256), so the caps below bound
/// glob counts per rule and per policy rather than per list alone.
const MAX_INLINE_GLOBS: usize = 64;
/// Named exclusion sets per policy, and globs listed in one set.
const MAX_SETS: usize = 64;
const MAX_SET_GLOBS: usize = 256;
/// Sets one rule or set may reference.
const MAX_SET_REFERENCES: usize = 16;
/// A rule's reference to a set is depth 1; each `include` adds one.
const MAX_SET_DEPTH: usize = 4;
/// Distinct exclusion globs of one rule after resolving its sets.
const MAX_RESOLVED_EXCLUDE: usize = 256;
/// Scope and exclusion globs summed over every rule of a policy after
/// resolution, which bounds total matching work at about 4096 globs per path.
const MAX_POLICY_GLOBS: usize = 4096;
/// The longest file prefix that `skipIfContent` reads.
const MAX_SKIP_PREFIX: usize = 64 * 1024;
const MAX_PROVENANCE_URL: usize = 2048;
/// Tags per rule and bytes per tag, kept equal to the `tags` and `tag` limits
/// in schema/policy-v1.json, tools/build.mjs and sdk/index.d.ts.
pub const MAX_TAGS: usize = 16;
pub const MAX_TAG: usize = 64;
pub type Result<T> = std::result::Result<T, String>;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Policy {
    version: u32,
    #[serde(default, deserialize_with = "present")]
    sets: Option<Vec<SetSource>>,
    rules: Vec<Rule>,
}

/// A named, reusable list of exclusion globs. The builder reads it as static
/// data from `.rulecheck/sets/<name>.set.ts` in the same worktree or commit as
/// the rules; nothing in it runs.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SetSource {
    name: String,
    version: u32,
    globs: Vec<String>,
    #[serde(default, deserialize_with = "present")]
    include: Option<Vec<String>>,
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
    #[serde(default, rename = "excludeSets", deserialize_with = "present")]
    exclude_sets: Option<Vec<String>>,
    #[serde(default, rename = "skipIfContent", deserialize_with = "present")]
    skip_if_content: Option<SkipIfContent>,
    remediation: String,
    #[serde(default, deserialize_with = "present")]
    message: Option<String>,
    #[serde(default, deserialize_with = "present")]
    pattern: Option<Pattern>,
    #[serde(default, deserialize_with = "present")]
    requires: Option<Pattern>,
    #[serde(default, deserialize_with = "present")]
    selector: Option<Selector>,
    #[serde(default, deserialize_with = "present")]
    structure: Option<Structure>,
    #[serde(default, deserialize_with = "present")]
    consistency: Option<consistency::Source>,
    #[serde(default, deserialize_with = "present")]
    provenance: Option<Provenance>,
    #[serde(default, deserialize_with = "present")]
    tags: Option<Vec<String>>,
    fixtures: Vec<FixtureSource>,
}

/// Where a rule came from. It is validated and then dropped: matchers, fixtures
/// and finding output ignore it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Provenance {
    finding: String,
}

/// Skips a file whose first `firstBytes` bytes contain a literal or match a
/// regex, such as a generated-file header.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SkipIfContent {
    kind: PatternKind,
    value: String,
    #[serde(rename = "firstBytes")]
    first_bytes: usize,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum PatternKind {
    Literal,
    Regex,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum Kind {
    Text,
    ForbiddenPath,
    Syntax,
    Structured,
    Consistency,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::Text => "text",
            Kind::ForbiddenPath => "forbidden-path",
            Kind::Syntax => "syntax",
            Kind::Structured => "structured",
            Kind::Consistency => "consistency",
        }
    }
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
    #[serde(default, deserialize_with = "present")]
    path: Option<String>,
    #[serde(default, deserialize_with = "present")]
    content: Option<String>,
    #[serde(default, deserialize_with = "present")]
    expect: Option<Option<Location>>,
    #[serde(default, rename = "expectAll", deserialize_with = "present")]
    expect_all: Option<Vec<Location>>,
    #[serde(default, deserialize_with = "present")]
    files: Option<Vec<FixtureFile>>,
    #[serde(default, rename = "expectFindings", deserialize_with = "present")]
    expect_findings: Option<Vec<FindingLocation>>,
}

/// One file of a consistency fixture's synthetic inventory.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FixtureFile {
    path: String,
    #[serde(default, deserialize_with = "present")]
    content: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FindingLocation {
    path: String,
    line: usize,
    column: usize,
}

pub struct Fixture {
    pub path: String,
    pub content: Option<String>,
    /// Consistency fixtures only: the inventory, sorted by path.
    pub files: Vec<(String, Option<String>)>,
    pub expect: Expect,
}

/// A fixture asserts either the first match (`expect`, the v1 format) or every
/// match in all-match order (`expectAll`, text rules only).
pub enum Expect {
    First(Option<Location>),
    All(Vec<Location>),
    /// Consistency fixtures: every finding as (path, location), sorted.
    Findings(Vec<(String, Location)>),
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
    /// The rule's `kind`, as written in the policy.
    pub kind: &'static str,
    pub remediation: String,
    /// Optional per-finding detail rendered from the regex's named captures.
    /// The fixed `remediation` is always reported beside it.
    pub message: Option<Template>,
    /// Metadata for `list --format json`. Matching, fixtures, findings and the
    /// matcher digest never read it. Tags are sorted and unique.
    pub tags: Vec<String>,
    pub provenance: Option<String>,
    pub scope: GlobSet,
    pub exclude: GlobSet,
    /// Forbidden-path rules only: globs for files that must exist for an
    /// absent forbidden path to count as clean. `None` keeps the v1 default,
    /// where no matching file is always a pass.
    pub anchors: Option<GlobSet>,
    pub skip: Option<ContentSkip>,
    pub matcher: Matcher,
    /// Digest of what decides the matches inside one file (see `matcher_digest`).
    /// A baseline entry recorded under another digest is incompatible.
    pub matcher_digest: String,
    pub fixtures: Vec<Fixture>,
    /// How JavaScript would read this rule's regex differently, shown when a
    /// fixture fails. Matching never uses it.
    pub dialect_note: Option<String>,
}

/// A compiled `skipIfContent`: a byte regex run on a bounded file prefix.
pub struct ContentSkip {
    regex: regex::bytes::Regex,
    first_bytes: usize,
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
    Structured(StructuredMatcher),
    /// A cross-file rule, judged once over its whole selected inventory.
    Consistency(Box<Consistency>),
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
/// `requires`, `selector` and `structure`, which decide what matches inside one file. The ID, scope
/// (including `excludeSets` and `skipIfContent`, which only take whole files out),
/// remediation, provenance, tags and fixtures are left out: none of them changes the
/// count in a file that stays in scope, and a file that enters the scope has
/// no baseline allowance. The `counting` version changes whenever Rulecheck's
/// all-match semantics do, so old baselines stop matching.
fn matcher_digest(rule: &Value) -> String {
    let mut semantics = serde_json::Map::new();
    semantics.insert("counting".into(), Value::from(1));
    for key in [
        "kind",
        "pattern",
        "requires",
        "selector",
        "structure",
        "consistency",
    ] {
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
    let sets = Sets::new(policy.sets.unwrap_or_default())?;
    let mut policy_globs = 0;

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
        let mut rule = rule
            .compile(&sets, &mut policy_globs)
            .map_err(|err| format!("[{id}] {err}"))?;
        rule.matcher_digest = matcher_digest(raw);
        compiled.push(rule);
    }
    compiled.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(compiled)
}

impl Rule {
    fn compile(self, sets: &Sets, policy_globs: &mut usize) -> Result<CompiledRule> {
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

        let tags = match self.tags {
            None => Vec::new(),
            Some(tags) => compile_tags(tags)?,
        };

        if !(1..=MAX_INLINE_GLOBS).contains(&self.paths.len()) {
            return Err(format!("provide 1 to {MAX_INLINE_GLOBS} path globs"));
        }
        let scope = globs("path", &self.paths)?;
        // Inline globs come first, then each set in the order listed, each
        // glob kept once. Exclusion has no precedence: a path matching any
        // resolved glob is out of scope.
        let mut excluded = Vec::new();
        match &self.exclude {
            None => {}
            Some(exclude) if (1..=MAX_INLINE_GLOBS).contains(&exclude.len()) => {
                excluded.extend(exclude.iter().map(String::as_str));
            }
            Some(_) => {
                return Err(format!(
                    "provide 1 to {MAX_INLINE_GLOBS} exclude globs or omit exclude"
                ));
            }
        }
        if let Some(names) = &self.exclude_sets {
            references("excludeSets", names)?;
            let mut visited = BTreeSet::new();
            for name in names {
                sets.resolve(name, &mut visited, &mut excluded)?;
            }
        }
        let mut seen = BTreeSet::new();
        excluded.retain(|glob| seen.insert(*glob));
        if excluded.len() > MAX_RESOLVED_EXCLUDE {
            return Err(format!(
                "excludes {} distinct globs after resolving excludeSets; use at most {MAX_RESOLVED_EXCLUDE}",
                excluded.len()
            ));
        }
        *policy_globs += self.paths.len() + excluded.len();
        if *policy_globs > MAX_POLICY_GLOBS {
            return Err(format!(
                "the policy uses more than {MAX_POLICY_GLOBS} path and exclusion globs across all rules; share fewer, broader globs"
            ));
        }
        let exclude = globs("exclude", &excluded)?;
        let anchors = match (&self.anchors, self.kind) {
            (None, _) => None,
            (Some(_), Kind::Consistency) => {
                return Err(
                    "anchors apply to forbidden-path rules only; a consistency rule already \
                     exits 2 when one of its scopes matches no file"
                        .into(),
                );
            }
            (Some(_), Kind::Text | Kind::Syntax | Kind::Structured) => {
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
        if self.consistency.is_some() != matches!(self.kind, Kind::Consistency) {
            return Err(
                "consistency rules require a consistency block, and only they take one".into(),
            );
        }
        if matches!(self.kind, Kind::Consistency) && self.skip_if_content.is_some() {
            return Err(
                "consistency rules take no skipIfContent; narrow their scopes with exclude".into(),
            );
        }
        let (matcher, dialect_note) = match (self.kind, self.pattern, self.selector, self.structure)
        {
            (Kind::Consistency, None, None, None) => {
                let (rule, count) = self.consistency.expect("checked above").compile()?;
                *policy_globs += count;
                if *policy_globs > MAX_POLICY_GLOBS {
                    return Err(format!(
                        "the policy uses more than {MAX_POLICY_GLOBS} path and exclusion globs across all rules; share fewer, broader globs"
                    ));
                }
                (Matcher::Consistency(Box::new(rule)), None)
            }
            (Kind::Text, Some(pattern), None, None) => {
                let (trigger, note) = Matcher::compile(pattern)?;
                match self.requires {
                    None => (trigger, note),
                    Some(requires) => {
                        let (requirement, required_note) = Matcher::requirement(requires)?;
                        let matcher = Matcher::Requires {
                            trigger: Box::new(trigger),
                            requirement: Box::new(requirement),
                        };
                        (matcher, note.or(required_note))
                    }
                }
            }
            (Kind::ForbiddenPath, None, None, None) => (Matcher::ForbiddenPath, None),
            (Kind::Syntax, None, Some(selector), None) => {
                let selector = selector.compile()?;
                let note = selector.dialect_note().map(str::to_owned);
                (Matcher::Syntax(selector), note)
            }
            (Kind::Structured, None, None, Some(structure)) => {
                (Matcher::Structured(structure.compile()?), None)
            }
            _ => {
                return Err(
                    "text rules require a pattern, syntax rules require a selector, \
                     structured rules require a structure, forbidden-path and consistency rules omit all three"
                        .into(),
                );
            }
        };

        let skip = match self.skip_if_content {
            None => None,
            Some(_) if !matcher.needs_text() => {
                return Err(
                    "skipIfContent needs a text, syntax or structured rule; forbidden-path rules never read content"
                        .into(),
                );
            }
            Some(skip) => Some(ContentSkip::compile(skip)?),
        };

        let message = match (self.message, &matcher) {
            (None, _) => None,
            (Some(template), Matcher::Regex(regex)) => Some(Template::parse(&template, regex)?),
            (Some(_), _) => {
                return Err(
                    "message requires a regex text pattern, whose named groups it references; \
                     drop message to report only the fixed remediation"
                        .into(),
                );
            }
        };

        if !(1..=64).contains(&self.fixtures.len()) {
            return Err("provide 1 to 64 fixtures".into());
        }
        let mut fixtures = Vec::with_capacity(self.fixtures.len());
        for (index, fixture) in self.fixtures.into_iter().enumerate() {
            if let Matcher::Consistency(_) = matcher {
                fixtures.push(consistency_fixture(index, fixture)?);
                continue;
            }
            if fixture.files.is_some() || fixture.expect_findings.is_some() {
                return Err(format!(
                    "fixture {}: files and expectFindings are for consistency rules; use path and expect",
                    index + 1
                ));
            }
            let Some(path) = fixture.path else {
                return Err(format!("fixture {}: provide a path", index + 1));
            };
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
                Expect::Findings(_) => unreachable!("consistency fixtures are compiled apart"),
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
                files: Vec::new(),
                expect,
            });
        }

        Ok(CompiledRule {
            id: self.id,
            kind: self.kind.name(),
            remediation: self.remediation,
            message,
            tags,
            provenance: self.provenance.map(|provenance| provenance.finding),
            matcher,
            matcher_digest: String::new(),
            scope,
            exclude,
            anchors,
            skip,
            fixtures,
            dialect_note,
        })
    }
}

/// A consistency fixture: a synthetic inventory of files and every finding
/// the rule reports on it.
fn consistency_fixture(index: usize, fixture: FixtureSource) -> Result<Fixture> {
    let label = format!("fixture {}", index + 1);
    let (Some(files), Some(expected)) = (fixture.files, fixture.expect_findings) else {
        return Err(format!(
            "{label}: consistency fixtures take files and expectFindings"
        ));
    };
    if fixture.path.is_some()
        || fixture.content.is_some()
        || fixture.expect.is_some()
        || fixture.expect_all.is_some()
    {
        return Err(format!(
            "{label}: consistency fixtures take only files and expectFindings"
        ));
    }
    if !(1..=64).contains(&files.len()) {
        return Err(format!("{label}: provide 1 to 64 files"));
    }
    let mut inventory = BTreeMap::new();
    for file in files {
        if !valid_path(&file.path) {
            return Err(format!("{label}: invalid file path {:?}", file.path));
        }
        if inventory.insert(file.path.clone(), file.content).is_some() {
            return Err(format!("{label}: lists {:?} twice", file.path));
        }
    }
    let mut findings = Vec::with_capacity(expected.len());
    for found in expected {
        if found.line == 0 || found.column == 0 {
            return Err(format!("{label}: expectFindings uses one-based locations"));
        }
        findings.push((
            found.path,
            Location {
                line: found.line,
                column: found.column,
            },
        ));
    }
    if findings.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err(format!(
            "{label}: list expectFindings by path, then location, without duplicates"
        ));
    }
    Ok(Fixture {
        path: inventory.keys().next().cloned().unwrap_or_default(),
        content: None,
        files: inventory.into_iter().collect(),
        expect: Expect::Findings(findings),
    })
}

/// Checks `tags`: 1 to 16 distinct tags of at most 64 bytes, each shaped like
/// a rule ID (`adr/0007`, `domain/billing`). Returns them sorted, so a reordered
/// list reads the same in the inventory.
fn compile_tags(mut tags: Vec<String>) -> Result<Vec<String>> {
    if !(1..=MAX_TAGS).contains(&tags.len()) {
        return Err(format!("provide 1 to {MAX_TAGS} tags or omit tags"));
    }
    for tag in &tags {
        if tag.len() > MAX_TAG || !valid_rule_id(tag) {
            return Err(format!(
                "invalid tag {tag:?}; use at most {MAX_TAG} lowercase letters, digits, hyphens and slashes, such as \"adr/0007\""
            ));
        }
    }
    tags.sort();
    if let Some(pair) = tags.windows(2).find(|pair| pair[0] == pair[1]) {
        return Err(format!("tags lists {:?} twice", pair[0]));
    }
    Ok(tags)
}

/// Mirrors the provenance `pattern` and `maxLength` in schema/policy-v1.json: an
/// https:// URL with no whitespace or control characters, counted in Unicode scalars.
fn valid_provenance_url(url: &str) -> bool {
    url.strip_prefix("https://").is_some_and(|rest| {
        !rest.is_empty() && !rest.chars().any(|c| c.is_whitespace() || c.is_control())
    }) && url.chars().count() <= MAX_PROVENANCE_URL
}

/// Checks a list of set names: 1 to 16 distinct, known-shape names.
fn references(field: &str, names: &[String]) -> Result<()> {
    if !(1..=MAX_SET_REFERENCES).contains(&names.len()) {
        return Err(format!(
            "provide 1 to {MAX_SET_REFERENCES} set names in {field} or omit it"
        ));
    }
    let mut seen = BTreeSet::new();
    for name in names {
        if !seen.insert(name) {
            return Err(format!("{field} lists set {name:?} twice"));
        }
    }
    Ok(())
}

pub(crate) fn valid_name(name: &str) -> bool {
    name.len() <= 128
        && name.split('/').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
        })
}

/// The policy's named exclusion sets, validated when the policy loads.
struct Sets(BTreeMap<String, SetSource>);

impl Sets {
    fn new(sources: Vec<SetSource>) -> Result<Self> {
        if sources.len() > MAX_SETS {
            return Err(format!("policy must have at most {MAX_SETS} sets"));
        }
        let mut sets = BTreeMap::new();
        for set in sources {
            let name = set.name.clone();
            let context = |err: String| format!("set {name:?}: {err}");
            if !valid_name(&name) {
                return Err(context(
                    "invalid set name; use lowercase letters, digits, hyphens and slashes".into(),
                ));
            }
            if set.version != 1 {
                return Err(context(format!(
                    "unsupported set version {}; use version 1",
                    set.version
                )));
            }
            if set.globs.len() > MAX_SET_GLOBS {
                return Err(context(format!("list at most {MAX_SET_GLOBS} globs")));
            }
            if set.globs.is_empty() && set.include.is_none() {
                return Err(context("list globs or include another set".into()));
            }
            // Compiling validates each glob before any rule uses the set.
            globs("exclude", &set.globs).map_err(context)?;
            if let Some(include) = &set.include {
                references("include", include).map_err(context)?;
            }
            if sets.insert(name.clone(), set).is_some() {
                return Err(context("duplicate set name".into()));
            }
        }
        let sets = Self(sets);
        let mut heights = BTreeMap::new();
        for name in sets.0.keys() {
            sets.height(name, &mut Vec::new(), &mut heights)?;
        }
        Ok(sets)
    }

    /// The longest include chain from `name`, counting `name` as 1. Every set
    /// is checked, used or not, so a cycle, an unknown include or a chain
    /// deeper than a rule may reference fails when the policy loads.
    fn height<'s>(
        &'s self,
        name: &'s str,
        chain: &mut Vec<&'s str>,
        heights: &mut BTreeMap<&'s str, usize>,
    ) -> Result<usize> {
        let path = |chain: &[&str]| {
            chain
                .iter()
                .chain([&name])
                .map(|name| format!("{name:?}"))
                .collect::<Vec<_>>()
                .join(" -> ")
        };
        if chain.contains(&name) {
            return Err(format!("set include cycle {}", path(chain)));
        }
        if let Some(height) = heights.get(name) {
            return Ok(*height);
        }
        let Some((key, set)) = self.0.get_key_value(name) else {
            let parent = chain.last().expect("top-level names come from the map");
            return Err(format!("set {parent:?} includes unknown set {name:?}"));
        };
        chain.push(key);
        let mut height = 1;
        for include in set.include.iter().flatten() {
            height = height.max(1 + self.height(include, chain, heights)?);
        }
        chain.pop();
        if height > MAX_SET_DEPTH {
            return Err(format!(
                "set {name:?} nests includes deeper than {MAX_SET_DEPTH} levels"
            ));
        }
        heights.insert(key, height);
        Ok(height)
    }

    /// Appends the globs of `name`, then of each set it includes, depth first.
    /// A set reached twice (a diamond) is expanded once.
    fn resolve<'s>(
        &'s self,
        name: &str,
        visited: &mut BTreeSet<&'s str>,
        out: &mut Vec<&'s str>,
    ) -> Result<()> {
        let Some((key, set)) = self.0.get_key_value(name) else {
            return Err(format!(
                "unknown set {name:?} in excludeSets; add .rulecheck/sets/{name}.set.ts or fix the name"
            ));
        };
        if !visited.insert(key) {
            return Ok(());
        }
        out.extend(set.globs.iter().map(String::as_str));
        for include in set.include.iter().flatten() {
            self.resolve(include, visited, out)?;
        }
        Ok(())
    }
}

impl ContentSkip {
    fn compile(skip: SkipIfContent) -> Result<Self> {
        if skip.value.is_empty() || skip.value.len() > 4096 {
            return Err("use a skipIfContent value of 1 to 4096 bytes".into());
        }
        if !(1..=MAX_SKIP_PREFIX).contains(&skip.first_bytes) {
            return Err(format!(
                "skipIfContent.firstBytes must be 1 to {MAX_SKIP_PREFIX}"
            ));
        }
        let pattern = match skip.kind {
            PatternKind::Literal => regex::escape(&skip.value),
            PatternKind::Regex => skip.value,
        };
        let regex = regex::bytes::RegexBuilder::new(&pattern)
            .nest_limit(64)
            .size_limit(256 * 1024)
            .dfa_size_limit(128 * 1024)
            .build()
            .map_err(|err| {
                format!(
                    "invalid skipIfContent regex: {}",
                    dialect::explain(&pattern, err)
                )
            })?;
        Ok(Self {
            regex,
            first_bytes: skip.first_bytes,
        })
    }

    /// Whether the first `firstBytes` bytes match. A literal or regex match
    /// must lie wholly inside the prefix, which may end inside a character.
    pub fn skips(&self, content: &str) -> bool {
        let bytes = content.as_bytes();
        self.regex
            .is_match(&bytes[..bytes.len().min(self.first_bytes)])
    }
}

pub(crate) fn globs(field: &str, patterns: &[impl AsRef<str>]) -> Result<GlobSet> {
    let mut set = GlobSetBuilder::new();
    for pattern in patterns {
        let pattern = pattern.as_ref();
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
    fn compile(pattern: Pattern) -> Result<(Self, Option<String>)> {
        let value = match &pattern {
            Pattern::Literal { value } | Pattern::Regex { value } => value,
        };
        if value.is_empty() || value.len() > 4096 {
            return Err("use a pattern of 1 to 4096 bytes".into());
        }
        match pattern {
            Pattern::Literal { value } => Ok((Self::Literal(value), None)),
            Pattern::Regex { value } => dialect::compile(&value)
                .map(|compiled| (Self::Regex(compiled.regex), compiled.note))
                .map_err(|err| format!("invalid regex: {err}")),
        }
    }

    /// A `requires` pattern. One that can match empty text would be met by
    /// every file, so the rule could never report and is refused.
    fn requirement(pattern: Pattern) -> Result<(Self, Option<String>)> {
        let (matcher, note) = Self::compile(pattern).map_err(|err| format!("requires: {err}"))?;
        if let Self::Regex(regex) = &matcher
            && regex.is_match("")
        {
            return Err(
                "requires: the regex can match empty text, so every file would meet the \
                 requirement and the rule could never report; match at least one character"
                    .into(),
            );
        }
        Ok((matcher, note))
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
        !matches!(self, Self::ForbiddenPath | Self::Consistency(_))
    }

    pub fn location(&self, path: &str, content: Option<&str>) -> Result<Option<Location>> {
        Ok(self
            .find(path, content, &mut Trees::default())?
            .map(|(location, _)| location))
    }

    /// The first match as a one-based line and Unicode scalar column, plus its
    /// zero-based, half-open UTF-8 byte range. Forbidden paths have no range.
    /// Only a syntax rule can fail, when its file does not parse. `trees`
    /// caches the file's parse for the other syntax rules that apply to it.
    pub fn find(
        &self,
        path: &str,
        content: Option<&str>,
        trees: &mut Trees,
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
            Self::Syntax(selector) => selector.find(path, content, trees)?,
            Self::Structured(structure) => structure.find_all(path, content, 1)?.into_iter().next(),
            Self::ForbiddenPath => return Ok(Some((Location { line: 1, column: 1 }, None))),
            Self::Consistency(_) => {
                return Err("consistency rules are judged across files, not per file".into());
            }
            Self::Requires {
                trigger,
                requirement,
            } => {
                // The requirement is searched only once the trigger matched.
                return Ok(trigger
                    .find(path, Some(content), trees)?
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
            Self::Literal(_) | Self::Regex(_) | Self::Structured(_) | Self::Consistency(_) => None,
        }
    }

    /// Every match in all-match mode, in ascending byte order, stopping after
    /// `limit` matches. A structured rule reports each failing node once. Literal matches are non-overlapping: the search resumes
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
            (Self::Structured(structure), Some(content)) => {
                structure.find_all(path, content, limit)?
            }
            (Self::Syntax(_), _) => {
                return Err(self.all_matches_unsupported().unwrap_or_default().into());
            }
            _ => {
                return Ok(self
                    .find(path, content, &mut Trees::default())?
                    .into_iter()
                    .collect());
            }
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
pub(crate) fn locate(content: &str, start: usize) -> Location {
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
    /// Whether a path is in scope: it matches `paths` and no inline or set
    /// exclusion glob. Path checks never read content.
    pub fn applies_to(&self, path: &str) -> bool {
        self.scope.is_match(path) && !self.exclude.is_match(path)
    }

    /// Whether a scan must read this in-scope file's text for the rule.
    pub fn reads(&self, path: &str) -> bool {
        match &self.matcher {
            Matcher::Consistency(rule) => rule.reads(path),
            matcher => matcher.needs_text(),
        }
    }

    /// Whether `path` is one of the files that show the guarded area exists.
    pub fn anchored_by(&self, path: &str) -> bool {
        self.anchors.as_ref().is_some_and(|set| set.is_match(path))
    }

    /// Whether `skipIfContent` drops a file that is in scope. It runs only on
    /// content Rulecheck has read and decoded, so a file it cannot read is
    /// never skipped: the scan still cannot judge it.
    pub fn skips(&self, content: Option<&str>) -> bool {
        matches!((&self.skip, content), (Some(skip), Some(content)) if skip.skips(content))
    }

    /// The rendered message of the match at `span`, or `None` when the rule
    /// has no message template. The regex runs again from the match start, so
    /// its captures are those of the reported match.
    pub fn message(&self, content: Option<&str>, span: Option<Span>) -> Option<String> {
        let template = self.message.as_ref()?;
        let captures = match (&self.matcher, content, span) {
            (Matcher::Regex(regex), Some(content), Some(span)) => {
                regex.captures_at(content, span.start).filter(|found| {
                    let whole = found.get(0).expect("group 0 is the whole match");
                    (whole.start(), whole.end()) == (span.start, span.end)
                })
            }
            _ => None,
        };
        Some(template.render(captures.as_ref()))
    }
}
