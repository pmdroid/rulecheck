//! Pins per-finding message templates: `{name}` references to a regex rule's
//! named groups, checked when the policy loads and rendered per match beside
//! the fixed remediation. Captured text is escaped, capped and never evaluated,
//! and rules without a template keep their v1 output.
mod common;

use serde_json::{Value, json};
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

const ROOT: &str = env!("CARGO_MANIFEST_DIR");
const REMEDIATION: &str = "Remove legacy_* settings.";

fn cli(args: &[&str], policy: &[u8], root: &Path) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_rulecheck-evaluator"))
        .current_dir(root)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("run rulecheck");
    let _ = child.stdin.take().unwrap().write_all(policy);
    child.wait_with_output().unwrap()
}

fn run(output: Output, code: i32) -> (String, String) {
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert_eq!(
        output.status.code(),
        Some(code),
        "stdout: {stdout}\nstderr: {stderr}"
    );
    (stdout, stderr)
}

fn git(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn commit(root: &Path) -> String {
    git(root, &["add", "-A"]);
    git(
        root,
        &[
            "-c",
            "user.name=Rulecheck Test",
            "-c",
            "user.email=rulecheck@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-qm",
            "Rulecheck message fixture",
        ],
    );
    git(root, &["rev-parse", "HEAD"])
}

fn write(root: &Path, path: &str, content: &str) {
    let path = root.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

/// A regex rule over `config/*.toml` with `key` and `value` groups.
fn rule(message: Option<&str>) -> Value {
    let mut rule = json!({
        "id": "config/no-legacy-settings",
        "kind": "text",
        "paths": ["config/*.toml"],
        "pattern": {
            "kind": "regex",
            "value": "(?m)^(?<key>legacy_[a-z_]+) = (?<value>[^\\n]*)(?<note> #old)?$",
        },
        "remediation": REMEDIATION,
        "fixtures": [{ "path": "config/app.toml", "content": "x = 1\n", "expect": null }],
    });
    if let Some(message) = message {
        rule["message"] = json!(message);
    }
    rule
}

fn policy(rules: &[Value]) -> Vec<u8> {
    serde_json::to_vec(&json!({ "version": 1, "rules": rules })).unwrap()
}

fn json_check(args: &[&str], policy: &[u8], root: &Path) -> Value {
    let mut args = args.to_vec();
    args.extend(["--format", "json"]);
    let (stdout, _) = run(cli(&args, policy, root), 1);
    let result: Value = serde_json::from_str(&stdout).unwrap();
    let schema: Value = serde_json::from_str(include_str!("../schema/result-v1.json")).unwrap();
    assert_eq!(common::violations(&schema, &result), Vec::<String>::new());
    result
}

fn messages(result: &Value) -> Vec<Value> {
    result["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|finding| finding["message"].clone())
        .collect()
}

#[test]
fn distinct_matches_render_distinct_messages_beside_the_remediation() {
    let repo = tempfile::tempdir().unwrap();
    let root = repo.path();
    git(root, &["init", "-q"]);
    write(
        root,
        "config/app.toml",
        "legacy_cache = true\nname = 1\nlegacy_port = 8080 #old\n",
    );
    let sha = commit(root);
    let templated = policy(&[rule(Some("{key} is set to {value}{note}."))]);

    let expected = format!(
        "config/app.toml:1:1: [config/no-legacy-settings] legacy_cache is set to true. {REMEDIATION}\n\
         config/app.toml:3:1: [config/no-legacy-settings] legacy_port is set to 8080 #old. {REMEDIATION}\n"
    );
    for args in [
        vec!["check", "--all-matches"],
        vec!["check", "--all-matches", "--commit", &sha],
    ] {
        assert_eq!(run(cli(&args, &templated, root), 1).0, expected);
        let result = json_check(&args, &templated, root);
        // An unset group (no ` #old` on line 1) renders as empty text.
        assert_eq!(
            messages(&result),
            [
                json!("legacy_cache is set to true."),
                json!("legacy_port is set to 8080 #old.")
            ]
        );
        for finding in result["findings"].as_array().unwrap() {
            assert_eq!(finding["remediation"], json!(REMEDIATION));
        }
    }

    // The first-match default renders the first match's captures.
    let (first, _) = run(cli(&["check"], &templated, root), 1);
    assert_eq!(first, expected.lines().next().unwrap().to_owned() + "\n");

    // Without a template the output keeps its v1 shape: no message key at all.
    let plain = policy(&[rule(None)]);
    let (human, _) = run(cli(&["check", "--all-matches"], &plain, root), 1);
    assert_eq!(
        human,
        format!(
            "config/app.toml:1:1: [config/no-legacy-settings] {REMEDIATION}\n\
             config/app.toml:3:1: [config/no-legacy-settings] {REMEDIATION}\n"
        )
    );
    let result = json_check(&["check", "--all-matches"], &plain, root);
    for finding in result["findings"].as_array().unwrap() {
        let mut keys: Vec<_> = finding.as_object().unwrap().keys().collect();
        keys.sort();
        assert_eq!(
            keys,
            [
                "byteEnd",
                "byteStart",
                "column",
                "line",
                "path",
                "remediation",
                "ruleId"
            ]
        );
    }
    // The template does not change which text matches, so the matcher digest
    // that baselines record is the same.
    let baseline = |policy: &[u8]| run(cli(&["baseline"], policy, root), 0).0;
    assert_eq!(baseline(&templated), baseline(&plain));
}

#[test]
fn captured_text_is_escaped_capped_and_never_evaluated() {
    let repo = tempfile::tempdir().unwrap();
    let root = repo.path();
    git(root, &["init", "-q"]);
    let long = "é".repeat(80);
    write(
        root,
        "config/app.toml",
        &format!(
            "legacy_a = {{key}} $1 ${{value}} .* (?<x>y)\n\
             legacy_b = \u{1b}[31mred\u{202e}txt\t\r\n\
             legacy_c = {long}\n"
        ),
    );
    git(root, &["add", "-A"]);
    let templated = policy(&[rule(Some("{{{key}}}: {value}"))]);
    let result = json_check(&["check", "--all-matches"], &templated, root);
    assert_eq!(
        messages(&result),
        [
            json!("{legacy_a}: {key} $1 ${value} .* (?<x>y)"),
            json!("{legacy_b}: \\u{1b}[31mred\\u{202e}txt\\t\\r"),
            json!(format!("{{legacy_c}}: {}…", "é".repeat(64))),
        ]
    );
    // Human output carries the same escaped text on one line per finding.
    let (human, _) = run(cli(&["check", "--all-matches"], &templated, root), 1);
    assert_eq!(human.lines().count(), 3, "{human}");
    assert!(!human.contains('\u{1b}') && !human.contains('\r') && !human.contains('\u{202e}'));
}

#[test]
fn invalid_templates_are_rejected_when_the_policy_loads() {
    let root = Path::new(ROOT);
    let schema: Value = serde_json::from_str(include_str!("../schema/policy-v1.json")).unwrap();
    for (message, error) in [
        (
            "{missing}",
            "message references {missing}, which is not a named group of the pattern; use one of key, note, value",
        ),
        ("plain text", "message references no capture group"),
        ("{key", "message has an unclosed {"),
        ("key}", "message has an unmatched }"),
        ("{0}", "message reference {0} must name a capture group"),
        ("{key}\n", "provide a single-line message up to 512 bytes"),
    ] {
        let (_, stderr) = run(cli(&["validate"], &policy(&[rule(Some(message))]), root), 2);
        assert!(
            stderr.contains(&format!("[config/no-legacy-settings] {error}")),
            "{message:?}: {stderr}"
        );
    }

    // Only regex text rules have named groups to reference.
    let refused = "message requires a regex text pattern";
    let mut literal = rule(Some("{key}"));
    literal["pattern"] = json!({ "kind": "literal", "value": "legacy_" });
    let path = json!({
        "id": "config/no-local", "kind": "forbidden-path", "paths": ["config/local.toml"],
        "remediation": "Remove it.", "message": "{key}",
        "fixtures": [{ "path": "config/local.toml", "expect": { "line": 1, "column": 1 } }],
    });
    for (rule, schema_rejects) in [(literal, false), (path, true)] {
        let bytes = policy(std::slice::from_ref(&rule));
        let (_, stderr) = run(cli(&["validate"], &bytes, root), 2);
        assert!(stderr.contains(refused), "{stderr}");
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            !common::violations(&schema, &value).is_empty(),
            schema_rejects
        );
    }

    let accepted = policy(&[rule(Some("{key} = {value}"))]);
    let value: Value = serde_json::from_slice(&accepted).unwrap();
    assert_eq!(common::violations(&schema, &value), Vec::<String>::new());
    run(cli(&["validate"], &accepted, root), 0);
}

#[test]
fn builder_passes_messages_through_and_rejects_them_off_regex_rules() {
    let repo = tempfile::tempdir().unwrap();
    let root = repo.path();
    git(root, &["init", "-q"]);
    let example = include_str!("../docs/examples/rules/message.rule.ts");
    let build = || {
        Command::new("node")
            .arg(Path::new(ROOT).join("tools/build.mjs"))
            .arg("--root")
            .arg(root)
            .output()
            .expect("run build; install Node.js and run npm ci")
    };
    write(root, ".rulecheck/rules/message.rule.ts", example);
    let (built, _) = run(build(), 0);
    let built: Value = serde_json::from_str(&built).unwrap();
    assert_eq!(
        built["rules"][0]["message"],
        json!("{key} is set to {value}.")
    );

    let literal = example.replace(
        "kind: \"regex\",\n    value: \"(?m)^[ \\\\t]*(?<key>",
        "kind: \"literal\",\n    value: \"(?m)^[ \\\\t]*(?<key>",
    );
    assert_ne!(literal, example);
    write(root, ".rulecheck/rules/message.rule.ts", &literal);
    let (_, stderr) = run(build(), 2);
    assert!(
        stderr.contains(
            "message.rule.ts:12:3: Only regex text rules have a message. \
             Use a regex pattern with named groups, or remove message."
        ),
        "{stderr}"
    );
}
