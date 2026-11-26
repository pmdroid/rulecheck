mod policy;
mod repository;

use policy::{CompiledRule, Location, Result};
use repository::Report;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// Version of the `check --format json` document, independent of the policy version.
const RESULT_VERSION: u32 = 1;

const HELP: &str = "\
rulecheck: deterministic repository checks

  npm run --silent build -- [--root DIR]    Emit a policy from TypeScript rules
  rulecheck schema                          Print the version 1 JSON schema
  rulecheck help                            Print this help
  rulecheck validate                        Validate a policy from stdin
  rulecheck list [--id-prefix PREFIX]        List policy rule IDs
  rulecheck test [--id-prefix PREFIX]        Check exact fixture expectations
  rulecheck check [--root DIR] [--commit SHA | --untracked] [--id-prefix PREFIX] [--format human|json]
                                            Scan a checkout or exact Git commit

--untracked adds non-ignored untracked files to a working-tree check.

Exit codes: 0 clean, 1 violation or fixture mismatch, 2 cannot judge.";

const COMMIT_HINT: &str = "--commit requires a full 40- or 64-character Git commit SHA; \
resolve it first, e.g. --commit \"$(git rev-parse HEAD)\"";

const UNTRACKED_COMMIT: &str = "--untracked applies to working-tree checks only; \
a commit has no untracked files, so drop --untracked or --commit";

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

fn run(mut args: impl Iterator<Item = OsString>) -> Result<u8> {
    let Some(command) = args.next() else {
        eprintln!("{HELP}");
        return Err("missing command; run rulecheck help".into());
    };
    let command = command.to_str().ok_or("expected a UTF-8 command")?;
    let mut root = None;
    let mut commit = None;
    let mut prefix = None;
    let mut format = None;
    let mut untracked = false;
    while let Some(flag) = args.next() {
        match flag.to_str() {
            Some("--root") if command == "check" && root.is_none() => {
                root = Some(PathBuf::from(
                    args.next().ok_or("--root requires a directory")?,
                ));
            }
            Some("--commit") if command == "check" && commit.is_none() => {
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
            Some("--untracked") if command == "check" && !untracked => untracked = true,
            Some("--id-prefix")
                if matches!(command, "list" | "test" | "check") && prefix.is_none() =>
            {
                let value = args
                    .next()
                    .and_then(|value| value.into_string().ok())
                    .filter(|value| !value.is_empty() && !value.starts_with("--"))
                    .ok_or("--id-prefix requires a nonempty UTF-8 prefix")?;
                prefix = Some(value);
            }
            Some("--format") if command == "check" && format.is_none() => {
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

    match command {
        "help" | "-h" | "--help" => println!("{HELP}"),
        "-V" | "--version" => println!("rulecheck {}", env!("CARGO_PKG_VERSION")),
        "schema" => print!("{}", include_str!("../schema/policy-v1.json")),
        "check" => {
            let root = root.as_deref().unwrap_or(Path::new("."));
            return check(
                root,
                commit.as_deref(),
                untracked,
                prefix.as_deref(),
                format.unwrap_or(Format::Human),
            );
        }
        "validate" | "list" | "test" => {
            let rules = policy::load(&policy::read(io::stdin().lock())?)?;
            let rules = select(&rules, prefix.as_deref())?;
            match command {
                "list" => rules.iter().for_each(|rule| println!("{}", rule.id)),
                "test" => return Ok(u8::from(!test(&rules))),
                _ => {}
            }
        }
        _ => return Err(format!("unknown command {command:?}; run rulecheck help")),
    }
    Ok(0)
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

/// Reads policy from stdin and scans the checkout or commit. Human output keeps
/// the line format `path:line:column: [id] remediation`. JSON output always
/// writes one result document to stdout, including when the scan cannot judge,
/// and repeats diagnostics on stderr in the human form.
fn check(
    root: &Path,
    commit: Option<&str>,
    untracked: bool,
    prefix: Option<&str>,
    format: Format,
) -> Result<u8> {
    let bytes = policy::read(io::stdin().lock());
    let digest = bytes
        .as_ref()
        .ok()
        .map(|bytes| format!("sha256:{:x}", Sha256::digest(bytes)));
    let rules = bytes.and_then(|bytes| policy::load(&bytes));
    let selected = rules
        .as_ref()
        .map_err(Clone::clone)
        .and_then(|rules| select(rules, prefix));
    let report = selected
        .as_ref()
        .map_err(Clone::clone)
        .and_then(|rules| match commit {
            Some(sha) => repository::check_commit(root, sha, rules),
            None => repository::check(root, rules, untracked),
        });

    if format == Format::Human {
        let report = report?;
        for finding in &report.findings {
            let Location { line, column } = finding.location;
            println!(
                "{}:{line}:{column}: [{}] {}",
                finding.path, finding.id, finding.remediation
            );
        }
        for id in &report.empty_scopes {
            eprintln!("[{id}] {EMPTY_SCOPE}");
        }
        if !report.empty_scopes.is_empty() {
            return Err(EMPTY_SCOPES.into());
        }
        return Ok(u8::from(!report.findings.is_empty()));
    }

    let mut diagnostics = Vec::new();
    let mut findings = Vec::new();
    match &report {
        Ok(Report {
            findings: found,
            empty_scopes,
        }) => {
            findings = found.iter().map(finding_json).collect();
            for id in empty_scopes {
                eprintln!("[{id}] {EMPTY_SCOPE}");
                diagnostics.push(json!({ "ruleId": id, "message": EMPTY_SCOPE }));
            }
            if !empty_scopes.is_empty() {
                eprintln!("rulecheck: {EMPTY_SCOPES}");
                diagnostics.push(json!({ "ruleId": null, "message": EMPTY_SCOPES }));
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
        u8::from(!findings.is_empty())
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
    if untracked {
        source["untracked"] = json!(true);
    }
    let result = json!({
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
    println!(
        "{}",
        serde_json::to_string_pretty(&result).expect("serialize result")
    );
    Ok(code)
}

fn finding_json(finding: &repository::Finding) -> Value {
    json!({
        "ruleId": finding.id,
        "path": finding.path,
        "line": finding.location.line,
        "column": finding.location.column,
        "byteStart": finding.span.map(|span| span.start),
        "byteEnd": finding.span.map(|span| span.end),
        "remediation": finding.remediation,
    })
}

fn test(rules: &[&CompiledRule]) -> bool {
    let mut failures = Vec::new();
    let mut count = 0;
    for rule in rules {
        for (index, fixture) in rule.fixtures.iter().enumerate() {
            count += 1;
            let actual = if rule.applies_to(&fixture.path) {
                rule.matcher.location(fixture.content.as_deref())
            } else {
                None
            };
            if actual != fixture.expect {
                failures.push((
                    &fixture.path,
                    &rule.id,
                    index,
                    fixture.expect,
                    actual,
                    &rule.remediation,
                ));
            }
        }
    }
    failures.sort_unstable();
    for (path, id, index, expected, actual, remediation) in &failures {
        let Location { line, column } = actual.or(*expected).expect("at least one location");
        println!(
            "{path}:{line}:{column}: [{id}] fixture {} expected {}, found {}. {remediation} Update the rule or fixture.",
            index + 1,
            describe(*expected),
            describe(*actual)
        );
    }
    if failures.is_empty() {
        println!("{count} fixtures passed");
    }
    failures.is_empty()
}

fn describe(location: Option<Location>) -> String {
    match location {
        Some(Location { line, column }) => format!("a violation at {line}:{column}"),
        None => "no violation".into(),
    }
}
