//! Pins `--strict-fixtures`: an opt-in check on `test` and `validate` that fails
//! any selected rule whose fixtures all expect no violation. The default `test`
//! and `validate` keep their v1 behaviour, missing fixtures stay a load error in
//! both modes, and baselines are not involved.
use serde_json::{Value, json};
use std::io::Write;
use std::process::{Command, Output, Stdio};

fn cli(args: &[&str], policy: &Value) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_rulecheck-evaluator"))
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("run rulecheck");
    // A usage error exits before reading stdin, so a broken pipe is expected there.
    let _ = child
        .stdin
        .take()
        .unwrap()
        .write_all(policy.to_string().as_bytes());
    child.wait_with_output().unwrap()
}

fn run(args: &[&str], policy: &Value) -> (i32, String, String) {
    let output = cli(args, policy);
    (
        output.status.code().unwrap(),
        String::from_utf8(output.stdout).unwrap(),
        String::from_utf8(output.stderr).unwrap(),
    )
}

fn text(id: &str, fixtures: Value) -> Value {
    json!({
        "id": id, "kind": "text", "paths": ["config/**"],
        "pattern": {"kind": "literal", "value": "legacy_mode = true"},
        "remediation": "Remove legacy_mode.",
        "fixtures": fixtures
    })
}

const NEGATIVE: &str =
    r#"{"path": "config/app.toml", "content": "legacy_mode = false\n", "expect": null}"#;
const POSITIVE: &str = r#"{"path": "config/app.toml", "content": "legacy_mode = true\n", "expect": {"line": 1, "column": 1}}"#;

fn fixture(source: &str) -> Value {
    serde_json::from_str(source).unwrap()
}

fn policy(rules: Vec<Value>) -> Value {
    json!({"version": 1, "rules": rules})
}

#[test]
fn a_positive_and_negative_pair_passes_strict_mode() {
    let policy = policy(vec![text(
        "config/pair",
        json!([fixture(NEGATIVE), fixture(POSITIVE)]),
    )]);
    assert_eq!(
        run(&["test", "--strict-fixtures"], &policy),
        (
            0,
            "2 fixtures passed\nevery selected rule has a positive fixture\n".into(),
            String::new()
        )
    );
    assert_eq!(
        run(&["validate", "--strict-fixtures"], &policy),
        (
            0,
            "every selected rule has a positive fixture\n".into(),
            String::new()
        )
    );
}

#[test]
fn a_positive_only_rule_passes_strict_mode() {
    let policy = policy(vec![text("config/positive", json!([fixture(POSITIVE)]))]);
    assert_eq!(run(&["test", "--strict-fixtures"], &policy).0, 0);
    assert_eq!(run(&["validate", "--strict-fixtures"], &policy).0, 0);
}

#[test]
fn a_negative_only_rule_fails_only_in_strict_mode() {
    // This matcher can never match the scoped file, yet its fixtures pass.
    let policy = policy(vec![
        text(
            "config/b-negative",
            json!([fixture(NEGATIVE), fixture(NEGATIVE)]),
        ),
        text("config/a-negative", json!([fixture(NEGATIVE)])),
        text("config/pair", json!([fixture(NEGATIVE), fixture(POSITIVE)])),
    ]);
    // Opt-in: the default commands keep their v1 results.
    assert_eq!(
        run(&["test"], &policy),
        (0, "5 fixtures passed\n".into(), String::new())
    );
    assert_eq!(
        run(&["validate"], &policy),
        (0, String::new(), String::new())
    );

    let report = concat!(
        "config/app.toml:1:1: [config/a-negative] has no positive fixture: its only fixture ",
        "expects no violation, so the rule would pass even if it never matched. Add a fixture ",
        "that expects a violation.\n",
        "config/app.toml:1:1: [config/b-negative] has no positive fixture: all 2 fixtures ",
        "expect no violation, so the rule would pass even if it never matched. Add a fixture ",
        "that expects a violation.\n",
    );
    assert_eq!(
        run(&["test", "--strict-fixtures"], &policy),
        (1, format!("5 fixtures passed\n{report}"), String::new())
    );
    assert_eq!(
        run(&["validate", "--strict-fixtures"], &policy),
        (1, report.into(), String::new())
    );
    // --id-prefix narrows the coverage check like the fixture run.
    assert_eq!(
        run(
            &["test", "--strict-fixtures", "--id-prefix", "config/pair"],
            &policy
        )
        .0,
        0
    );
}

#[test]
fn an_empty_expect_all_is_not_a_positive_fixture() {
    let negative = json!({"path": "config/app.toml", "content": "x\n", "expectAll": []});
    let positive = json!({
        "path": "config/app.toml", "content": "legacy_mode = true\n",
        "expectAll": [{"line": 1, "column": 1}]
    });
    let (code, stdout, _) = run(
        &["validate", "--strict-fixtures"],
        &policy(vec![text("config/all", json!([negative.clone()]))]),
    );
    assert_eq!(code, 1);
    assert!(
        stdout.contains("[config/all] has no positive fixture"),
        "{stdout}"
    );
    let pair = policy(vec![text("config/all", json!([negative, positive]))]);
    assert_eq!(run(&["test", "--strict-fixtures"], &pair).0, 0);
}

#[test]
fn missing_fixtures_cannot_be_judged_in_either_mode() {
    let policy = policy(vec![text("config/none", json!([]))]);
    for args in [
        &["test"][..],
        &["test", "--strict-fixtures"],
        &["validate", "--strict-fixtures"],
    ] {
        let (code, stdout, stderr) = run(args, &policy);
        assert_eq!(code, 2, "{args:?}");
        assert_eq!(stdout, "");
        assert!(stderr.contains("provide 1 to 64 fixtures"), "{stderr}");
    }
}

#[test]
fn forbidden_path_and_syntax_rules_need_a_positive_fixture_too() {
    let path_rule = |fixtures: Value| {
        json!({
            "id": "path/no-env", "kind": "forbidden-path", "paths": ["**/.env"],
            "remediation": "Remove the .env file.", "fixtures": fixtures
        })
    };
    let syntax_rule = |fixtures: Value| {
        json!({
            "id": "source/no-comments", "kind": "syntax", "paths": ["src/**"],
            "selector": {"language": "typescript", "node": "comment"},
            "remediation": "Remove the comment.", "fixtures": fixtures
        })
    };
    let path_negative = json!({"path": "app/env.example", "expect": null});
    let path_positive = json!({"path": "app/.env", "expect": {"line": 1, "column": 1}});
    let syntax_negative = json!({"path": "src/a.ts", "content": "const a = 1;\n", "expect": null});
    let syntax_positive =
        json!({"path": "src/a.ts", "content": "// c\n", "expect": {"line": 1, "column": 1}});

    let negative = policy(vec![
        path_rule(json!([path_negative.clone()])),
        syntax_rule(json!([syntax_negative.clone()])),
    ]);
    assert_eq!(run(&["test"], &negative).0, 0);
    let (code, stdout, _) = run(&["test", "--strict-fixtures"], &negative);
    assert_eq!(code, 1);
    assert!(
        stdout.contains("app/env.example:1:1: [path/no-env] has no positive fixture"),
        "{stdout}"
    );
    assert!(
        stdout.contains("src/a.ts:1:1: [source/no-comments] has no positive fixture"),
        "{stdout}"
    );

    let paired = policy(vec![
        path_rule(json!([path_negative, path_positive])),
        syntax_rule(json!([syntax_negative, syntax_positive])),
    ]);
    assert_eq!(run(&["test", "--strict-fixtures"], &paired).0, 0);
}

#[test]
fn strict_fixtures_is_accepted_only_by_test_and_validate() {
    let policy = policy(vec![text(
        "config/pair",
        json!([fixture(NEGATIVE), fixture(POSITIVE)]),
    )]);
    for command in ["list", "check", "baseline"] {
        let (code, _, stderr) = run(&[command, "--strict-fixtures"], &policy);
        assert_eq!(code, 2, "{command}");
        assert!(
            stderr.contains("unexpected argument \"--strict-fixtures\""),
            "{stderr}"
        );
    }
    let (code, _, stderr) = run(&["test", "--strict-fixtures", "--strict-fixtures"], &policy);
    assert_eq!(code, 2);
    assert!(stderr.contains("unexpected argument"), "{stderr}");
}
