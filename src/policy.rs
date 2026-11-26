use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Deserializer};
use std::collections::BTreeSet;
use std::io::Read;

pub const MAX_FILE: usize = 1024 * 1024;
const MAX_POLICY: usize = 1024 * 1024;
const MAX_RULES: usize = 128;
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
    remediation: String,
    #[serde(default, deserialize_with = "present")]
    pattern: Option<Pattern>,
    fixtures: Vec<Fixture>,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum Kind {
    Text,
    ForbiddenPath,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
enum Pattern {
    Literal { value: String },
    Regex { value: String },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fixture {
    pub path: String,
    #[serde(default, deserialize_with = "present")]
    pub content: Option<String>,
    #[serde(deserialize_with = "Deserialize::deserialize")]
    pub expect: Option<Location>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd)]
#[serde(deny_unknown_fields)]
pub struct Location {
    pub line: usize,
    pub column: usize,
}

pub struct CompiledRule {
    pub id: String,
    pub remediation: String,
    pub scope: GlobSet,
    pub exclude: GlobSet,
    pub matcher: Matcher,
    pub fixtures: Vec<Fixture>,
}

pub enum Matcher {
    Literal(String),
    Regex(Regex),
    ForbiddenPath,
}

fn present<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
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

pub fn load(reader: impl Read) -> Result<Vec<CompiledRule>> {
    let bytes = bounded_read(reader, MAX_POLICY).map_err(|err| format!("policy stdin: {err}"))?;
    let policy: Policy =
        serde_json::from_slice(&bytes).map_err(|err| format!("invalid policy JSON: {err}"))?;
    if policy.version != 1 {
        return Err(format!(
            "unsupported policy version {}; use version 1",
            policy.version
        ));
    }
    if !(1..=MAX_RULES).contains(&policy.rules.len()) {
        return Err(format!("policy must have 1 to {MAX_RULES} rules"));
    }

    let mut ids = BTreeSet::new();
    let mut compiled = Vec::with_capacity(policy.rules.len());
    for rule in policy.rules {
        if rule.id.is_empty()
            || rule.id.len() > 128
            || !rule.id.split('/').all(|part| {
                !part.is_empty()
                    && part
                        .bytes()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
            })
        {
            return Err(format!(
                "invalid rule ID {:?}; use lowercase letters, digits, hyphens and slashes",
                rule.id
            ));
        }
        if !ids.insert(rule.id.clone()) {
            return Err(format!("[{}] duplicate rule ID", rule.id));
        }
        let id = rule.id.clone();
        compiled.push(rule.compile().map_err(|err| format!("[{id}] {err}"))?);
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

        if !(1..=16).contains(&self.paths.len()) {
            return Err("provide 1 to 16 path globs".into());
        }
        let scope = globs("path", &self.paths)?;
        let exclude = match &self.exclude {
            None => GlobSet::empty(),
            Some(exclude) if (1..=16).contains(&exclude.len()) => globs("exclude", exclude)?,
            Some(_) => return Err("provide 1 to 16 exclude globs or omit exclude".into()),
        };

        let matcher = match (self.kind, self.pattern) {
            (Kind::Text, Some(pattern)) => Matcher::compile(pattern)?,
            (Kind::ForbiddenPath, None) => Matcher::ForbiddenPath,
            _ => return Err("text rules require a pattern; forbidden-path rules omit it".into()),
        };

        if !(1..=64).contains(&self.fixtures.len()) {
            return Err("provide 1 to 64 fixtures".into());
        }
        for fixture in &self.fixtures {
            if !valid_path(&fixture.path) {
                return Err(format!("invalid fixture path {:?}", fixture.path));
            }
            if matcher.needs_text() != fixture.content.is_some() {
                return Err(format!(
                    "{}: provide content only for text fixtures",
                    fixture.path
                ));
            }
            if let Some(location) = fixture.expect
                && (location.line == 0 || location.column == 0)
            {
                return Err(format!(
                    "{}: fixture uses one-based locations",
                    fixture.path
                ));
            }
        }

        Ok(CompiledRule {
            id: self.id,
            remediation: self.remediation,
            matcher,
            scope,
            exclude,
            fixtures: self.fixtures,
        })
    }
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

    pub fn needs_text(&self) -> bool {
        !matches!(self, Self::ForbiddenPath)
    }

    pub fn location(&self, content: Option<&str>) -> Option<Location> {
        let offset = match self {
            Self::Literal(value) => content?.find(value)?,
            Self::Regex(regex) => regex.find(content?)?.start(),
            Self::ForbiddenPath => return Some(Location { line: 1, column: 1 }),
        };
        let before = &content?[..offset];
        let line = before.bytes().filter(|byte| *byte == b'\n').count() + 1;
        let column = before.rsplit('\n').next()?.chars().count() + 1;
        Some(Location { line, column })
    }
}

impl CompiledRule {
    pub fn applies_to(&self, path: &str) -> bool {
        self.scope.is_match(path) && !self.exclude.is_match(path)
    }
}
