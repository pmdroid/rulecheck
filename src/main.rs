mod policy;
mod repository;

use policy::{CompiledRule, Location, Result};
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const HELP: &str = "\
rulecheck: deterministic repository checks

  npm run --silent build -- [--root DIR]    Emit a policy from TypeScript rules
  rulecheck schema                          Print the version 1 JSON schema
  rulecheck validate                        Validate a policy from stdin
  rulecheck list [--id-prefix PREFIX]        List policy rule IDs
  rulecheck test [--id-prefix PREFIX]        Check exact fixture expectations
  rulecheck check [--root DIR] [--commit SHA] [--id-prefix PREFIX]
                                            Scan a checkout or exact Git commit

Exit codes: 0 clean, 1 violation or fixture mismatch, 2 cannot judge.";

fn main() -> ExitCode {
    match run(std::env::args_os().skip(1)) {
        Ok(clean) => ExitCode::from(u8::from(!clean)),
        Err(error) => {
            eprintln!("rulecheck: {error}");
            ExitCode::from(2)
        }
    }
}

fn run(mut args: impl Iterator<Item = OsString>) -> Result<bool> {
    let command = args.next().unwrap_or_else(|| "--help".into());
    let command = command.to_str().ok_or("expected a UTF-8 command")?;
    let mut root = None;
    let mut commit = None;
    let mut prefix = None;
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
                        .ok_or("--commit requires a full Git commit SHA")?,
                );
            }
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
            _ => {
                return Err(format!(
                    "unexpected argument {flag:?}; run rulecheck --help"
                ));
            }
        }
    }

    match command {
        "-h" | "--help" => println!("{HELP}"),
        "-V" | "--version" => println!("rulecheck {}", env!("CARGO_PKG_VERSION")),
        "schema" => print!("{}", include_str!("../schema/policy-v1.json")),
        "validate" | "list" | "test" | "check" => {
            let rules = policy::load(io::stdin().lock())?;
            let rules: Vec<_> = rules
                .iter()
                .filter(|rule| {
                    prefix
                        .as_ref()
                        .is_none_or(|prefix| rule.id.starts_with(prefix))
                })
                .collect();
            if rules.is_empty() {
                return Err(format!(
                    "no rule IDs match prefix {:?}",
                    prefix.unwrap_or_default()
                ));
            }
            match command {
                "list" => rules.iter().for_each(|rule| println!("{}", rule.id)),
                "test" => return Ok(test(&rules)),
                "check" => {
                    let root = root.as_deref().unwrap_or(Path::new("."));
                    return match commit {
                        Some(sha) => repository::check_commit(root, &sha, &rules),
                        None => repository::check(root, &rules),
                    };
                }
                _ => {}
            }
        }
        _ => return Err(format!("unknown command {command:?}; run rulecheck --help")),
    }
    Ok(true)
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
