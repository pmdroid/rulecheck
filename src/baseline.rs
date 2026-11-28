//! A count-only baseline of existing findings: for each rule ID and normalized
//! repository path, how many all-match findings were accepted when the rule was
//! adopted. A check passes a (rule, path) pair whose count is at or below its
//! allowance, and fails one above it, including any path the baseline does not
//! list. The file is canonical JSON, so the same counts always give the same
//! bytes, and any other form is rejected rather than guessed at.
use crate::policy::{CompiledRule, Matcher, Result, bounded_read, valid_path, valid_rule_id};
use crate::repository::Finding;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::ErrorKind;
use std::path::Path;

pub const VERSION: u32 = 1;
const MAX_BASELINE: usize = 4 * 1024 * 1024;
/// No (rule, path) pair can hold more findings than one scan reports.
const MAX_COUNT: u64 = 10_000;
const REGENERATE: &str = "regenerate it with rulecheck baseline";

#[derive(Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Baseline {
    baseline_version: u32,
    matches: String,
    rules: BTreeMap<String, Entry>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    /// The rule's matcher digest when the counts were recorded.
    matcher: String,
    paths: BTreeMap<String, u64>,
}

/// One rule and path whose count differs from its allowance.
pub struct Group {
    pub id: String,
    pub path: String,
    pub count: u64,
    pub allowed: u64,
}

pub struct Comparison {
    /// More findings than allowed, including paths the baseline does not list.
    pub exceeded: Vec<Group>,
    /// Fewer findings than allowed, including listed paths that are gone, renamed
    /// or out of scope. Strict checks fail until the baseline is tightened.
    pub loose: Vec<Group>,
    /// Baseline rules that the policy no longer has.
    pub stale: Vec<String>,
    /// Findings within their allowance, which the check does not report.
    pub allowed_findings: u64,
}

impl Comparison {
    /// Whether a strict check fails on a baseline that is looser than the scan.
    pub fn needs_tightening(&self) -> bool {
        !self.loose.is_empty() || !self.stale.is_empty()
    }

    pub fn is_exceeded(&self, finding: &Finding) -> bool {
        self.exceeded
            .binary_search_by(|group| {
                (group.id.as_str(), group.path.as_str()).cmp(&(finding.id, finding.path.as_str()))
            })
            .is_ok()
    }
}

/// Reads a baseline file. `Ok(None)` only when it does not exist and
/// `missing_ok`; a check always needs it.
pub fn read(path: &Path, missing_ok: bool) -> Result<Option<Vec<u8>>> {
    let context = |err: String| format!("baseline {}: {err}", path.display());
    let file = match File::open(path) {
        Ok(file) => file,
        Err(err) if missing_ok && err.kind() == ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(context(err.to_string())),
    };
    let metadata = file.metadata().map_err(|err| context(err.to_string()))?;
    if !metadata.is_file() {
        return Err(context("expected a regular file".into()));
    }
    bounded_read(file, MAX_BASELINE)
        .map(Some)
        .map_err(|err| context(err.to_string()))
}

/// Parses and validates a baseline. Unknown or missing fields, another
/// version, an invalid rule ID or path, a zero or oversized count, and any
/// bytes other than the canonical form are errors.
pub fn parse(bytes: &[u8]) -> Result<Baseline> {
    let baseline: Baseline =
        serde_json::from_slice(bytes).map_err(|err| format!("invalid baseline JSON: {err}"))?;
    if baseline.baseline_version != VERSION {
        return Err(format!(
            "unsupported baselineVersion {}; this Rulecheck reads version {VERSION}, so {REGENERATE}",
            baseline.baseline_version
        ));
    }
    if baseline.matches != "all" {
        return Err(format!(
            "unsupported baseline matches {:?}; baselines count every match (\"all\")",
            baseline.matches
        ));
    }
    for (id, entry) in &baseline.rules {
        if !valid_rule_id(id) {
            return Err(format!("invalid baseline rule ID {id:?}"));
        }
        let digest = entry.matcher.strip_prefix("sha256:").unwrap_or_default();
        if digest.len() != 64
            || !digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(format!(
                "[{id}] baseline matcher must be sha256: and 64 lowercase hex digits"
            ));
        }
        if entry.paths.is_empty() {
            return Err(format!(
                "[{id}] baseline lists no paths; omit a rule without findings"
            ));
        }
        for (path, count) in &entry.paths {
            if !valid_path(path) {
                return Err(format!(
                    "[{id}] invalid baseline path {path:?}; use a normalized repository-relative path"
                ));
            }
            if !(1..=MAX_COUNT).contains(count) {
                return Err(format!(
                    "[{id}] {path}: baseline count must be 1 to {MAX_COUNT}; omit a path without findings"
                ));
            }
        }
    }
    if render(&baseline).as_bytes() != bytes {
        return Err(format!(
            "baseline is not in canonical form (sorted keys, two-space indent, one trailing \
             newline, no duplicate keys); {REGENERATE}"
        ));
    }
    Ok(baseline)
}

pub fn render(baseline: &Baseline) -> String {
    let mut text = serde_json::to_string_pretty(baseline).expect("serialize baseline");
    text.push('\n');
    text
}

fn selected(prefix: Option<&str>, id: &str) -> bool {
    prefix.is_none_or(|prefix| id.starts_with(prefix))
}

/// Findings per (rule, path).
fn counts<'a>(findings: &'a [Finding]) -> BTreeMap<(&'a str, &'a str), u64> {
    let mut counts = BTreeMap::new();
    for finding in findings {
        *counts
            .entry((finding.id, finding.path.as_str()))
            .or_default() += 1;
    }
    counts
}

/// Compares complete all-match findings of the `selected` rules with the
/// baseline. An entry recorded for other match semantics, or for a syntax rule,
/// cannot be judged. Entries of unselected rules are ignored; entries of rules
/// missing from the whole policy are stale.
pub fn compare(
    baseline: &Baseline,
    policy: &[CompiledRule],
    selected_rules: &[&CompiledRule],
    prefix: Option<&str>,
    findings: &[Finding],
) -> Result<Comparison> {
    let mut allowances = BTreeMap::new();
    for rule in selected_rules {
        let Some(entry) = baseline.rules.get(&rule.id) else {
            continue;
        };
        check_entry(rule, entry)?;
        for (path, count) in &entry.paths {
            allowances.insert((rule.id.as_str(), path.as_str()), *count);
        }
    }
    let counts = counts(findings);
    let keys: BTreeSet<_> = counts.keys().chain(allowances.keys()).copied().collect();
    let mut comparison = Comparison {
        exceeded: Vec::new(),
        loose: Vec::new(),
        stale: Vec::new(),
        allowed_findings: 0,
    };
    for key in keys {
        let count = counts.get(&key).copied().unwrap_or(0);
        let allowed = allowances.get(&key).copied().unwrap_or(0);
        let group = || Group {
            id: key.0.to_owned(),
            path: key.1.to_owned(),
            count,
            allowed,
        };
        if count > allowed {
            comparison.exceeded.push(group());
        } else {
            comparison.allowed_findings += count;
            if count < allowed {
                comparison.loose.push(group());
            }
        }
    }
    comparison.stale = baseline
        .rules
        .keys()
        .filter(|id| selected(prefix, id) && !policy.iter().any(|rule| &rule.id == *id))
        .cloned()
        .collect();
    Ok(comparison)
}

fn check_entry(rule: &CompiledRule, entry: &Entry) -> Result<()> {
    if let Matcher::Syntax(_) = rule.matcher {
        return Err(format!(
            "[{}] the baseline lists counts for a syntax rule, which Rulecheck does not count; \
             remove the entry and fix the findings",
            rule.id
        ));
    }
    if entry.matcher != rule.matcher_digest {
        return Err(format!(
            "[{}] the baseline was recorded for other match semantics ({}, now {}); the old \
             counts cannot be applied to the changed rule, so regenerate its entry with \
             rulecheck baseline --id-prefix {} in a reviewed pull request",
            rule.id, entry.matcher, rule.matcher_digest, rule.id
        ));
    }
    Ok(())
}

/// Records the complete all-match findings of the `selected` rules. With a
/// prefix, entries of other rules are kept from `existing` unchanged; without
/// one the baseline is rebuilt. A syntax rule with findings cannot be recorded.
pub fn generate(
    existing: Option<Baseline>,
    selected_rules: &[&CompiledRule],
    prefix: Option<&str>,
    findings: &[Finding],
) -> Result<Baseline> {
    if let Some(finding) = findings.iter().find(|finding| {
        selected_rules
            .iter()
            .any(|rule| rule.id == finding.id && matches!(rule.matcher, Matcher::Syntax(_)))
    }) {
        return Err(format!(
            "[{}] {}: syntax rules cannot be recorded in a baseline because their matches are not \
             counted; fix the finding or narrow the rule's scope",
            finding.id, finding.path
        ));
    }
    let mut rules: BTreeMap<String, Entry> = match prefix {
        Some(prefix) => existing
            .map(|baseline| baseline.rules)
            .unwrap_or_default()
            .into_iter()
            .filter(|(id, _)| !id.starts_with(prefix))
            .collect(),
        None => BTreeMap::new(),
    };
    for ((id, path), count) in counts(findings) {
        let rule = selected_rules
            .iter()
            .find(|rule| rule.id == id)
            .expect("findings come from selected rules");
        rules
            .entry(id.to_owned())
            .or_insert_with(|| Entry {
                matcher: rule.matcher_digest.clone(),
                paths: BTreeMap::new(),
            })
            .paths
            .insert(path.to_owned(), count);
    }
    Ok(Baseline {
        baseline_version: VERSION,
        matches: "all".into(),
        rules,
    })
}

impl Baseline {
    /// Rules, paths and findings recorded.
    pub fn totals(&self) -> (usize, usize, u64) {
        let paths = self.rules.values().map(|entry| entry.paths.len()).sum();
        let findings = self
            .rules
            .values()
            .flat_map(|entry| entry.paths.values())
            .sum();
        (self.rules.len(), paths, findings)
    }
}

fn plural(count: u64) -> &'static str {
    if count == 1 { "finding" } else { "findings" }
}

/// A (rule, path) over its allowance.
pub fn exceeded_message(group: &Group) -> String {
    format!(
        "{}: [{}] {} {}, baseline allows {}",
        group.path,
        group.id,
        group.count,
        plural(group.count),
        group.allowed
    )
}

/// A (rule, path) under its allowance: a violation in strict mode, else a note.
pub fn loose_message(group: &Group) -> String {
    format!(
        "{}: [{}] {} {}, baseline allows {}; tighten the baseline: {REGENERATE}",
        group.path,
        group.id,
        group.count,
        plural(group.count),
        group.allowed
    )
}

pub fn stale_message(id: &str) -> String {
    format!("[{id}] the baseline lists a rule that is not in the policy; {REGENERATE} to remove it")
}
