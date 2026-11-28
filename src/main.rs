mod baseline;
mod consistency;
mod dialect;
mod message;
mod policy;
mod repository;
mod structured;
mod syntax;

use baseline::Comparison;
use policy::{CompiledRule, Expect, Location, Result};
use repository::{DEFAULT_TEXT_BUDGET_MIB, Finding, MAX_TEXT_BUDGET_MIB, Matches, Partial, Report};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::ffi::OsString;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use syntax::Trees;

/// Version of the `check --format json` document, independent of the policy version.
const RESULT_VERSION: u32 = 1;
/// Version of the `list --format json` rule inventory (schema/inventory-v1.json).
const INVENTORY_VERSION: u32 = 1;

const HELP: &str = "\
rulecheck: deterministic repository checks

  npm run --silent build -- [--root DIR]    Emit a policy from TypeScript rules
  rulecheck schema                          Print the version 1 JSON schema
  rulecheck help                            Print this help
  rulecheck validate [--strict-fixtures]    Validate a policy from stdin
  rulecheck list [--id-prefix PREFIX] [--format human|json]
                                            List policy rule IDs, or with json
                                            the rule inventory with tags
  rulecheck test [--id-prefix PREFIX] [--strict-fixtures]
                                            Check exact fixture expectations
  rulecheck check [--root DIR] [--commit SHA | --untracked] [--id-prefix PREFIX] [--all-matches]
                  [--text-budget MIB] [--baseline FILE [--strict-baseline]] [--format human|json]
                                            Scan a checkout or exact Git commit
  rulecheck baseline [--root DIR] [--commit SHA | --untracked] [--id-prefix PREFIX]
                  [--text-budget MIB] [--baseline FILE [--write]]
                                            Record existing finding counts

--untracked adds non-ignored untracked files to a working-tree check.
--all-matches reports every text match instead of the first per rule and file;
syntax rules cannot be counted yet, so selecting one exits 2.
--text-budget sets how much scoped text one check reads, in MiB: 1 to 1024,
default 64. A check that reaches it reports partial findings and exits 2.
--baseline FILE counts every match and reports only rule and path counts above
the baseline; --strict-baseline also fails counts below it. baseline records
the counts of a complete scan to stdout, or with --write replaces FILE; with
--id-prefix only those rules' entries change.
--strict-fixtures also fails any selected rule whose fixtures all expect no
violation: such a rule passes even if it can never match. It is opt-in.

Exit codes: 0 clean, 1 violation or fixture mismatch, 2 cannot judge.";

const COMMIT_HINT: &str = "--commit requires a full 40- or 64-character Git commit SHA; \
resolve it first, e.g. --commit \"$(git rev-parse HEAD)\"";

const UNTRACKED_COMMIT: &str = "--untracked applies to working-tree checks only; \
a commit has no untracked files, so drop --untracked or --commit";

const TEXT_BUDGET_HINT: &str =
    "--text-budget requires a whole number of MiB from 1 to 1024 (default 64)";

/// Parses `--text-budget` as decimal MiB in 1..=1024 and returns bytes.
/// Digits only, so signs, units, fractions and values past u64 are refused.
fn text_budget(value: Option<OsString>) -> Result<usize> {
    value
        .and_then(|value| value.into_string().ok())
        .filter(|text| !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit()))
        .and_then(|text| text.parse::<usize>().ok())
        .filter(|mib| (1..=MAX_TEXT_BUDGET_MIB).contains(mib))
        .map(|mib| mib * 1024 * 1024)
        .ok_or_else(|| TEXT_BUDGET_HINT.into())
}

#[derive(Clone, Copy, PartialEq)]
enum Format {
    Human,
    Json,
}

fn main() -> ExitCode {
    match run(std::env::args_os().skip(1)) {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!("rulecheck: {error}");
            ExitCode::from(2)
        }
    }
}

/// Parsed command-line flags. Each command accepts only its own.
struct Options {
    root: PathBuf,
    commit: Option<String>,
    untracked: bool,
    matches: Matches,
    budget: Option<usize>,
    prefix: Option<String>,
    format: Format,
    baseline: Option<PathBuf>,
    strict_baseline: bool,
    write: bool,
    strict_fixtures: bool,
}

fn run(mut args: impl Iterator<Item = OsString>) -> Result<u8> {
    let Some(command) = args.next() else {
        eprintln!("{HELP}");
        return Err("missing command; run rulecheck help".into());
    };
    let command = command.to_str().ok_or("expected a UTF-8 command")?;
    let scans = matches!(command, "check" | "baseline");
    let mut root = None;
    let mut commit = None;
    let mut prefix = None;
    let mut format = None;
    let mut untracked = false;
    let mut matches = Matches::First;
    let mut budget = None;
    let mut baseline = None;
    let mut strict_baseline = false;
    let mut write = false;
    let mut strict_fixtures = false;
    while let Some(flag) = args.next() {
        match flag.to_str() {
            Some("--root") if scans && root.is_none() => {
                root = Some(PathBuf::from(
                    args.next().ok_or("--root requires a directory")?,
                ));
            }
            Some("--commit") if scans && commit.is_none() => {
                commit = Some(
                    args.next()
                        .and_then(|value| value.into_string().ok())
                        .filter(|sha| {
                            matches!(sha.len(), 40 | 64)
                                && sha.bytes().all(|byte| byte.is_ascii_hexdigit())
                        })
                        .ok_or(COMMIT_HINT)?,
                );
            }
            Some("--untracked") if scans && !untracked => untracked = true,
            Some("--all-matches") if command == "check" && matches == Matches::First => {
                matches = Matches::All;
            }
            Some("--text-budget") if scans && budget.is_none() => {
                budget = Some(text_budget(args.next())?);
            }
            Some("--baseline") if scans && baseline.is_none() => {
                baseline = Some(PathBuf::from(
                    args.next()
                        .filter(|value| !value.is_empty())
                        .ok_or("--baseline requires a file")?,
                ));
            }
            Some("--strict-baseline") if command == "check" && !strict_baseline => {
                strict_baseline = true;
            }
            Some("--write") if command == "baseline" && !write => write = true,
            Some("--strict-fixtures")
                if matches!(command, "validate" | "test") && !strict_fixtures =>
            {
                strict_fixtures = true;
            }
            Some("--id-prefix")
                if matches!(command, "list" | "test" | "check" | "baseline")
                    && prefix.is_none() =>
            {
                let value = args
                    .next()
                    .and_then(|value| value.into_string().ok())
                    .filter(|value| !value.is_empty() && !value.starts_with("--"))
                    .ok_or("--id-prefix requires a nonempty UTF-8 prefix")?;
                prefix = Some(value);
            }
            Some("--format") if matches!(command, "check" | "list") && format.is_none() => {
                format = match args.next().as_ref().and_then(|value| value.to_str()) {
                    Some("human") => Some(Format::Human),
                    Some("json") => Some(Format::Json),
                    _ => return Err("--format requires human or json".into()),
                };
            }
            _ => {
                return Err(format!("unexpected argument {flag:?}; run rulecheck help"));
            }
        }
    }

    if untracked && commit.is_some() {
        return Err(UNTRACKED_COMMIT.into());
    }
    if strict_baseline && baseline.is_none() {
        return Err("--strict-baseline requires --baseline FILE".into());
    }
    if write && baseline.is_none() {
        return Err("--write requires --baseline FILE, the file to regenerate".into());
    }
    let options = Options {
        root: root.unwrap_or_else(|| PathBuf::from(".")),
        commit,
        untracked,
        matches,
        budget,
        prefix,
        format: format.unwrap_or(Format::Human),
        baseline,
        strict_baseline,
        write,
        strict_fixtures,
    };

    match command {
        "help" | "-h" | "--help" => println!("{HELP}"),
        "-V" | "--version" => println!("rulecheck {}", env!("CARGO_PKG_VERSION")),
        "schema" => print!("{}", include_str!("../schema/policy-v1.json")),
        "check" => return check(&options),
        "baseline" => return regenerate(&options),
        "validate" | "list" | "test" => {
            let bytes = policy::read(io::stdin().lock())?;
            let rules = policy::load(&bytes)?;
            let rules = select(&rules, options.prefix.as_deref())?;
            let mut passed = match command {
                "list" if options.format == Format::Json => {
                    let document = inventory(&rules, &bytes, options.prefix.as_deref());
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&document).expect("serialize inventory")
                    );
                    true
                }
                "list" => {
                    rules.iter().for_each(|rule| println!("{}", rule.id));
                    true
                }
                "test" => test(&rules),
                _ => true,
            };
            if options.strict_fixtures {
                passed &= fixture_coverage(&rules);
            }
            return Ok(u8::from(!passed));
        }
        _ => return Err(format!("unknown command {command:?}; run rulecheck help")),
    }
    Ok(0)
}

/// The `list --format json` document: every selected rule in ID order with its
/// kind, sorted tags, provenance and remediation. The same policy bytes always
/// give the same document, so consumers can diff or cache it by `policy.sha256`.
fn inventory(rules: &[&CompiledRule], bytes: &[u8], prefix: Option<&str>) -> Value {
    let rules: Vec<_> = rules
        .iter()
        .map(|rule| {
            json!({
                "id": rule.id,
                "kind": rule.kind,
                "tags": rule.tags,
                "provenance": rule.provenance.as_ref().map(|finding| json!({ "finding": finding })),
                "remediation": rule.remediation,
            })
        })
        .collect();
    json!({
        "inventoryVersion": INVENTORY_VERSION,
        "tools": { "evaluator": format!("rulecheck {}", env!("CARGO_PKG_VERSION")) },
        "policy": { "version": 1, "sha256": sha256(bytes), "idPrefix": prefix },
        "rules": rules,
    })
}

fn select<'a>(rules: &'a [CompiledRule], prefix: Option<&str>) -> Result<Vec<&'a CompiledRule>> {
    let selected: Vec<_> = rules
        .iter()
        .filter(|rule| prefix.is_none_or(|prefix| rule.id.starts_with(prefix)))
        .collect();
    if selected.is_empty() {
        return Err(format!(
            "no rule IDs match prefix {:?}",
            prefix.unwrap_or_default()
        ));
    }
    Ok(selected)
}

const EMPTY_SCOPE: &str = "empty text scope: track a matching file or update the rule's path globs";
const EMPTY_SCOPES: &str = "one or more text rules matched no repository files";
const EMPTY_CONSISTENCY: &str =
    "one or more consistency rules have a scope that matched no repository files";

/// A consistency scope that matched no file of its rule's inventory.
fn empty_consistency_scope(scope: &str) -> String {
    format!(
        "empty consistency scope {scope:?}: no file of the rule's inventory matches it; track a \
         matching file or update the scope's path globs"
    )
}
const MISSING_ANCHOR: &str = "missing anchor: no file matches the rule's anchors, so the area it \
guards moved or vanished and an absent forbidden path proves nothing; restore the anchor or \
update anchors and paths together";
const MISSING_ANCHORS: &str = "one or more forbidden-path rules found none of their anchors";
const UNEVALUATED: &str =
    "not evaluated on every file in scope: the scan stopped at the text budget";

/// Explains a budget stop: where, by how much, and what to change.
fn budget_message(partial: &Partial) -> String {
    let count = partial.unevaluated.len();
    format!(
        "scan stopped at {}: its text would bring the scan to {} bytes, over the {} MiB \
         text budget, after {} bytes were evaluated; findings are partial and {count} {} not \
         evaluated on every file in scope. Narrow the rule path scopes or select fewer rules \
         with --id-prefix; raise --text-budget (at most {MAX_TEXT_BUDGET_MIB}) only if the \
         scopes are already tight",
        partial.path,
        partial.needed,
        partial.budget / (1024 * 1024),
        partial.read,
        if count == 1 { "rule was" } else { "rules were" },
    )
}

/// The summary lines of a scan that cannot judge, in output order: empty text
/// scopes, missing anchors, then a budget stop. Empty when the scan judged.
fn cannot_judge(report: &Report) -> Vec<String> {
    let mut reasons = Vec::new();
    if !report.empty_scopes.is_empty() {
        reasons.push(EMPTY_SCOPES.to_owned());
    }
    if !report.empty_consistency.is_empty() {
        reasons.push(EMPTY_CONSISTENCY.to_owned());
    }
    if !report.missing_anchors.is_empty() {
        reasons.push(MISSING_ANCHORS.to_owned());
    }
    if let Some(partial) = &report.partial {
        reasons.push(budget_message(partial));
    }
    reasons
}

fn scan<'a>(options: &Options, rules: &[&'a CompiledRule], matches: Matches) -> Result<Report<'a>> {
    let budget = options
        .budget
        .unwrap_or(DEFAULT_TEXT_BUDGET_MIB * 1024 * 1024);
    match &options.commit {
        Some(sha) => repository::check_commit(&options.root, sha, rules, matches, budget),
        None => repository::check(&options.root, rules, options.untracked, matches, budget),
    }
}

/// The findings to report: with a baseline, only those of a rule and path over
/// their allowance.
fn shown<'r, 'a>(report: &'r Report<'a>, comparison: &Option<Comparison>) -> Vec<&'r Finding<'a>> {
    report
        .findings
        .iter()
        .filter(|finding| comparison.as_ref().is_none_or(|c| c.is_exceeded(finding)))
        .collect()
}

fn sha256(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

/// Reads policy from stdin and scans the checkout or commit. Human output keeps
/// the line format `path:line:column: [id] remediation`. JSON output always
/// writes one result document to stdout, including when the scan cannot judge,
/// and repeats diagnostics on stderr in the human form.
///
/// With `--baseline`, every match is counted and only the findings of a rule
/// and path over their allowance are reported. The baseline is read and
/// validated before any file is scanned, and a partial scan is never compared.
fn check(options: &Options) -> Result<u8> {
    let prefix = options.prefix.as_deref();
    let commit = options.commit.as_deref();
    let strict = options.strict_baseline;
    let bytes = policy::read(io::stdin().lock());
    let digest = bytes.as_ref().ok().map(|bytes| sha256(bytes));
    let rules = bytes.and_then(|bytes| policy::load(&bytes));
    let selected = rules
        .as_ref()
        .map_err(Clone::clone)
        .and_then(|rules| select(rules, prefix));
    let baseline_bytes = options.baseline.as_deref().map(|path| {
        baseline::read(path, false).map(|bytes| bytes.expect("a check requires its baseline"))
    });
    let baseline_digest = match &baseline_bytes {
        Some(Ok(bytes)) => Some(sha256(bytes)),
        _ => None,
    };
    let baseline_input =
        baseline_bytes.map(|bytes| bytes.and_then(|bytes| baseline::parse(&bytes)));
    let matches = if options.baseline.is_some() {
        Matches::All
    } else {
        options.matches
    };
    let judged = selected
        .as_ref()
        .map_err(Clone::clone)
        .and_then(|selected| {
            let baseline = baseline_input.transpose()?;
            if matches == Matches::All && baseline.is_none() {
                all_matches_supported(selected)?;
            }
            let report = scan(options, selected, matches)?;
            let comparison = match (&baseline, &report.partial) {
                (Some(baseline), None) => Some(baseline::compare(
                    baseline,
                    rules.as_deref().unwrap_or_default(),
                    selected,
                    prefix,
                    &report.findings,
                )?),
                _ => None,
            };
            Ok((report, comparison))
        });
    // Lines about counts that differ from the baseline. Human output prints
    // violations on stdout with the findings; JSON keeps stdout for the
    // document, so they all go to stderr. A strict check also fails on a
    // baseline looser than the scan; otherwise those lines are notes.
    let tighten = |comparison: &Option<Comparison>, human: bool| {
        let Some(comparison) = comparison else {
            return false;
        };
        let report = |line: String| match human {
            true => println!("{line}"),
            false => eprintln!("{line}"),
        };
        comparison
            .exceeded
            .iter()
            .for_each(|group| report(baseline::exceeded_message(group)));
        let lines = comparison.loose.iter().map(baseline::loose_message).chain(
            comparison
                .stale
                .iter()
                .map(|id| baseline::stale_message(id)),
        );
        for line in lines {
            match strict {
                true => report(line),
                false => eprintln!("rulecheck: note: {line}"),
            }
        }
        strict && comparison.needs_tightening()
    };

    if options.format == Format::Human {
        let (report, comparison) = judged?;
        let shown = shown(&report, &comparison);
        for finding in &shown {
            let Location { line, column } = finding.location;
            match &finding.message {
                None => println!(
                    "{}:{line}:{column}: [{}] {}",
                    finding.path, finding.id, finding.remediation
                ),
                Some(message) => println!(
                    "{}:{line}:{column}: [{}] {message} {}",
                    finding.path, finding.id, finding.remediation
                ),
            }
        }
        let tightened = tighten(&comparison, true);
        for id in &report.empty_scopes {
            eprintln!("[{id}] {EMPTY_SCOPE}");
        }
        for (id, scope) in &report.empty_consistency {
            eprintln!("[{id}] {}", empty_consistency_scope(scope));
        }
        for id in &report.missing_anchors {
            eprintln!("[{id}] {MISSING_ANCHOR}");
        }
        if let Some(partial) = &report.partial {
            for id in &partial.unevaluated {
                eprintln!("[{id}] {UNEVALUATED}");
            }
        }
        // Every reason but the last is printed here; main prints the last.
        let mut reasons = cannot_judge(&report);
        let Some(last) = reasons.pop() else {
            return Ok(u8::from(!shown.is_empty() || tightened));
        };
        reasons
            .iter()
            .for_each(|reason| eprintln!("rulecheck: {reason}"));
        return Err(last);
    }

    let mut diagnostics = Vec::new();
    let mut findings = Vec::new();
    let mut partial_json = None;
    let mut baseline_json = None;
    let mut tightened = false;
    match &judged {
        Ok((report, comparison)) => {
            findings = shown(report, comparison)
                .into_iter()
                .map(finding_json)
                .collect();
            tightened = tighten(comparison, false);
            if let Some(comparison) = comparison {
                let groups = |groups: &[baseline::Group]| -> Vec<Value> {
                    groups
                        .iter()
                        .map(|group| {
                            json!({
                                "ruleId": group.id,
                                "path": group.path,
                                "count": group.count,
                                "allowed": group.allowed,
                            })
                        })
                        .collect()
                };
                baseline_json = Some(json!({
                    "strict": strict,
                    "allowedFindings": comparison.allowed_findings,
                    "exceeded": groups(&comparison.exceeded),
                    "loose": groups(&comparison.loose),
                    "staleRuleIds": comparison.stale,
                }));
            }
            for id in &report.empty_scopes {
                eprintln!("[{id}] {EMPTY_SCOPE}");
                diagnostics.push(json!({ "ruleId": id, "message": EMPTY_SCOPE }));
            }
            for (id, scope) in &report.empty_consistency {
                let message = empty_consistency_scope(scope);
                eprintln!("[{id}] {message}");
                diagnostics.push(json!({ "ruleId": id, "message": message }));
            }
            for id in &report.missing_anchors {
                eprintln!("[{id}] {MISSING_ANCHOR}");
                diagnostics.push(json!({ "ruleId": id, "message": MISSING_ANCHOR }));
            }
            if let Some(partial) = &report.partial {
                for id in &partial.unevaluated {
                    eprintln!("[{id}] {UNEVALUATED}");
                    diagnostics.push(json!({ "ruleId": id, "message": UNEVALUATED }));
                }
            }
            for message in cannot_judge(report) {
                eprintln!("rulecheck: {message}");
                diagnostics.push(json!({ "ruleId": null, "message": message }));
            }
            if let Some(partial) = &report.partial {
                partial_json = Some(json!({
                    "reason": "text-budget",
                    "textBudgetBytes": partial.budget,
                    "textBytesEvaluated": partial.read,
                    "stoppedAt": partial.path,
                    "unevaluatedRuleIds": partial.unevaluated,
                }));
            }
        }
        Err(error) => {
            eprintln!("rulecheck: {error}");
            diagnostics.push(json!({ "ruleId": null, "message": error }));
        }
    }
    let code = if !diagnostics.is_empty() {
        2
    } else {
        u8::from(!findings.is_empty() || tightened)
    };
    let ids: Vec<_> = selected
        .as_deref()
        .unwrap_or_default()
        .iter()
        .map(|rule| rule.id.as_str())
        .collect();
    let outcome = ["pass", "violation", "error"][usize::from(code)];
    let mut source = json!({
        "mode": if commit.is_some() { "commit" } else { "working-tree" },
        "commit": commit,
    });
    // Present only when requested, so default results keep their v1 shape.
    if options.untracked {
        source["untracked"] = json!(true);
    }
    if matches == Matches::All {
        source["matches"] = json!("all");
    }
    if let Some(bytes) = options.budget {
        source["textBudgetMiB"] = json!(bytes / (1024 * 1024));
    }
    if options.baseline.is_some() {
        source["baselineSha256"] = json!(baseline_digest);
    }
    if strict {
        source["strictBaseline"] = json!(true);
    }
    let mut result = json!({
        "resultVersion": RESULT_VERSION,
        "command": "check",
        "outcome": outcome,
        "exitCode": code,
        "tools": { "evaluator": format!("rulecheck {}", env!("CARGO_PKG_VERSION")), "builder": null },
        "policy": {
            "version": rules.as_ref().ok().map(|_| 1),
            "sha256": digest,
            "idPrefix": prefix,
            "ruleIds": ids,
        },
        "source": source,
        "findings": findings,
        "diagnostics": diagnostics,
    });
    // Present only after a budget stop, so finished scans keep their v1 shape.
    if let Some(partial) = partial_json {
        result["partial"] = partial;
    }
    // Present only when a complete scan was compared with a baseline.
    if let Some(baseline) = baseline_json {
        result["baseline"] = baseline;
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&result).expect("serialize result")
    );
    Ok(code)
}

/// `baseline`: records the complete all-match counts of the selected rules and
/// writes the canonical baseline to stdout, or with `--write` replaces the
/// `--baseline` file. With `--id-prefix` only those rules' entries change; the
/// rest are kept from the existing file, which must then be valid. A partial
/// scan, an empty text scope, a findings cap or a syntax-rule finding records
/// nothing and exits 2.
fn regenerate(options: &Options) -> Result<u8> {
    let prefix = options.prefix.as_deref();
    let rules = policy::load(&policy::read(io::stdin().lock())?)?;
    let selected = select(&rules, prefix)?;
    let existing = match (&options.baseline, prefix) {
        (Some(path), Some(_)) => baseline::read(path, true)?
            .map(|bytes| baseline::parse(&bytes))
            .transpose()
            .map_err(|err| format!("{err}; scoped regeneration keeps the other rules' entries"))?,
        _ => None,
    };
    let report = scan(options, &selected, Matches::All)?;
    for id in &report.empty_scopes {
        eprintln!("[{id}] {EMPTY_SCOPE}");
    }
    for (id, scope) in &report.empty_consistency {
        eprintln!("[{id}] {}", empty_consistency_scope(scope));
    }
    for id in &report.missing_anchors {
        eprintln!("[{id}] {MISSING_ANCHOR}");
    }
    if let Some(partial) = &report.partial {
        for id in &partial.unevaluated {
            eprintln!("[{id}] {UNEVALUATED}");
        }
        return Err(format!(
            "cannot record a baseline from a partial scan, so nothing was written: {}",
            budget_message(partial)
        ));
    }
    let reasons = cannot_judge(&report);
    if !reasons.is_empty() {
        return Err(format!(
            "{}; a baseline is recorded only from a complete scan, so nothing was written",
            reasons.join("; ")
        ));
    }
    let baseline = baseline::generate(existing, &selected, prefix, &report.findings)?;
    let text = baseline::render(&baseline);
    match &options.baseline {
        Some(path) if options.write => write_file(path, &text)?,
        _ => print!("{text}"),
    }
    let (rules, paths, findings) = baseline.totals();
    eprintln!(
        "rulecheck: the baseline records {findings} findings in {paths} paths for {rules} rules"
    );
    Ok(0)
}

/// Replaces `path` in one rename, so a failed run leaves the old file.
fn write_file(path: &Path, text: &str) -> Result<()> {
    let context = |err: std::io::Error| format!("cannot write baseline {}: {err}", path.display());
    let parent = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    let mut file = tempfile::NamedTempFile::new_in(parent).map_err(context)?;
    file.write_all(text.as_bytes()).map_err(context)?;
    file.persist(path).map_err(|err| context(err.error))?;
    Ok(())
}

/// All-match counts must be exact, so a selected syntax rule, whose node count
/// is not defined yet, stops the check before any file is read.
fn all_matches_supported(rules: &[&CompiledRule]) -> Result<()> {
    for rule in rules {
        if let policy::Matcher::Syntax(_) = rule.matcher {
            return Err(format!(
                "[{}] --all-matches does not support syntax rules: they do not count matching \
                 nodes yet, and reporting only their first match would undercount; select text \
                 and forbidden-path rules with --id-prefix or drop --all-matches",
                rule.id
            ));
        }
    }
    Ok(())
}

fn finding_json(finding: &repository::Finding) -> Value {
    let mut value = json!({
        "ruleId": finding.id,
        "path": finding.path,
        "line": finding.location.line,
        "column": finding.location.column,
        "byteStart": finding.span.map(|span| span.start),
        "byteEnd": finding.span.map(|span| span.end),
        "remediation": finding.remediation,
    });
    // Present only for a rule with a message template, so other findings keep their v1 shape.
    if let Some(message) = &finding.message {
        value["message"] = json!(message);
    }
    value
}

fn test(rules: &[&CompiledRule]) -> bool {
    let mut failures = Vec::new();
    let mut errors = Vec::new();
    let mut count = 0;
    for rule in rules {
        for (index, fixture) in rule.fixtures.iter().enumerate() {
            count += 1;
            // The same path and content checks as a scan, in the same order.
            let content = fixture.content.as_deref();
            let applies = rule.applies_to(&fixture.path) && !rule.skips(content);
            let mismatch = match &fixture.expect {
                Expect::Findings(expected) => match consistency_fixture(rule, &fixture.files) {
                    Ok(actual) if actual == *expected => Ok(None),
                    Ok(actual) => {
                        let (path, at) = actual
                            .iter()
                            .zip(expected)
                            .find(|(found, wanted)| found != wanted)
                            .map(|(found, _)| found)
                            .or_else(|| actual.get(expected.len()))
                            .or_else(|| expected.get(actual.len()))
                            .expect("the lists differ");
                        failures.push((
                            path.clone(),
                            &rule.id,
                            index,
                            *at,
                            describe_findings(expected),
                            describe_findings(&actual),
                            &rule.remediation,
                            None,
                        ));
                        continue;
                    }
                    Err(error) => {
                        errors.push((fixture.path.clone(), &rule.id, index, error));
                        continue;
                    }
                },
                Expect::First(expected) => {
                    let actual = match applies {
                        true => rule.matcher.location(&fixture.path, content),
                        false => Ok(None),
                    };
                    actual.map(|actual| {
                        (actual != *expected).then(|| {
                            let at = actual.or(*expected).expect("at least one location");
                            (at, describe(*expected), describe(actual))
                        })
                    })
                }
                Expect::All(expected) => {
                    let actual = match applies {
                        true => rule
                            .matcher
                            .find_all(&fixture.path, content, usize::MAX, &mut Trees::default())
                            .map(|found| found.into_iter().map(|(at, _)| at).collect()),
                        false => Ok(Vec::new()),
                    };
                    actual.map(|actual: Vec<Location>| {
                        (actual != *expected).then(|| {
                            // Point at the first position where the lists differ.
                            let at = actual
                                .iter()
                                .zip(expected)
                                .find(|(found, wanted)| found != wanted)
                                .map(|(found, _)| *found)
                                .or_else(|| actual.get(expected.len()).copied())
                                .or_else(|| expected.get(actual.len()).copied())
                                .expect("at least one location");
                            (at, describe_all(expected), describe_all(&actual))
                        })
                    })
                }
            };
            match mismatch {
                Ok(None) => {}
                Ok(Some((at, expected, actual))) => {
                    failures.push((
                        fixture.path.clone(),
                        &rule.id,
                        index,
                        at,
                        expected,
                        actual,
                        &rule.remediation,
                        rule.dialect_note.as_deref(),
                    ));
                }
                Err(error) => errors.push((fixture.path.clone(), &rule.id, index, error)),
            }
        }
    }
    failures.sort_unstable();
    for (path, id, index, at, expected, actual, remediation, note) in &failures {
        let Location { line, column } = at;
        let note = note.map(|note| format!(" {note}.")).unwrap_or_default();
        println!(
            "{path}:{line}:{column}: [{id}] fixture {} expected {expected}, found {actual}. {remediation} Update the rule or fixture.{note}",
            index + 1,
        );
    }
    errors.sort_unstable();
    for (path, id, index, error) in &errors {
        println!(
            "{path}:1:1: [{id}] fixture {} cannot be judged: {error}. Fix the fixture content.",
            index + 1
        );
    }
    if failures.is_empty() && errors.is_empty() {
        println!("{count} fixtures passed");
    }
    failures.is_empty() && errors.is_empty()
}

/// `--strict-fixtures`: every selected rule needs at least one positive fixture,
/// one that expects a violation. A rule whose fixtures all expect nothing still
/// passes `test` when its matcher can never match, so the fixtures prove nothing
/// about what it catches. Missing fixtures are already rejected by the loader.
fn fixture_coverage(rules: &[&CompiledRule]) -> bool {
    let mut uncovered: Vec<_> = rules
        .iter()
        .filter(|rule| {
            !rule.fixtures.iter().any(|fixture| match &fixture.expect {
                Expect::First(expected) => expected.is_some(),
                Expect::All(expected) => !expected.is_empty(),
                Expect::Findings(expected) => !expected.is_empty(),
            })
        })
        .collect();
    uncovered.sort_unstable_by(|a, b| a.id.cmp(&b.id));
    for rule in &uncovered {
        let path = &rule.fixtures[0].path;
        println!(
            "{path}:1:1: [{}] has no positive fixture: {}, so the rule would pass even if \
             it never matched. Add a fixture that expects a violation.",
            rule.id,
            match rule.fixtures.len() {
                1 => "its only fixture expects no violation".to_string(),
                n => format!("all {n} fixtures expect no violation"),
            },
        );
    }
    if uncovered.is_empty() {
        println!("every selected rule has a positive fixture");
    }
    uncovered.is_empty()
}

/// Runs a consistency rule over a fixture's inventory, with the same scope
/// checks as a scan, and returns its findings in output order.
fn consistency_fixture(
    rule: &CompiledRule,
    files: &[(String, Option<String>)],
) -> Result<Vec<(String, Location)>> {
    let policy::Matcher::Consistency(consistency) = &rule.matcher else {
        unreachable!("only consistency rules have file-list fixtures");
    };
    let mut accumulator = consistency::Accumulator::new(consistency);
    for (path, content) in files {
        if rule.applies_to(path) {
            accumulator.add(path, content.as_deref().filter(|_| rule.reads(path)))?;
        }
    }
    if let Some(name) = accumulator.empty_scopes().first() {
        return Err(format!(
            "scope {name:?} matches no file in the fixture, so the rule cannot be judged"
        ));
    }
    let mut found: Vec<_> = accumulator
        .finish()
        .into_iter()
        .map(|hit| (hit.path, hit.location))
        .collect();
    found.sort_unstable();
    Ok(found)
}

fn describe_findings(findings: &[(String, Location)]) -> String {
    if findings.is_empty() {
        return "no violations".into();
    }
    let shown: Vec<_> = findings
        .iter()
        .take(16)
        .map(|(path, Location { line, column })| format!("{path}:{line}:{column}"))
        .collect();
    let more = match findings.len() {
        n if n > 16 => format!(" and {} more", n - 16),
        _ => String::new(),
    };
    format!("violations at {}{more}", shown.join(", "))
}

fn describe(location: Option<Location>) -> String {
    match location {
        Some(Location { line, column }) => format!("a violation at {line}:{column}"),
        None => "no violation".into(),
    }
}

/// An `expectAll` list. Only the first 16 positions are written out, so a
/// runaway regex cannot flood the output.
fn describe_all(locations: &[Location]) -> String {
    if locations.is_empty() {
        return "no violations".into();
    }
    let shown: Vec<_> = locations
        .iter()
        .take(16)
        .map(|Location { line, column }| format!("{line}:{column}"))
        .collect();
    let more = match locations.len() {
        n if n > 16 => format!(" and {} more", n - 16),
        _ => String::new(),
    };
    let noun = if locations.len() == 1 {
        "violation"
    } else {
        "violations"
    };
    format!("{} {noun} at {}{more}", locations.len(), shown.join(", "))
}
