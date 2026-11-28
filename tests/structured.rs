//! Pins the `structured` rule kind over YAML: the cookbook recipes, match
//! semantics for missing paths, multiple documents, flow style and quoting,
//! the inputs that cannot be judged (malformed YAML, duplicate keys, aliases
//! and merge keys on the path, limits), loader and schema validation, exact
//! locations that agree between working-tree and exact-commit checks, and
//! bounded time on large and deeply nested files.
mod common;

use serde_json::{Value, json};
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

const ROOT: &str = env!("CARGO_MANIFEST_DIR");
const TIMEOUT: &str = include_str!("../docs/examples/rules/structured-yaml.rule.ts");
const VALUE: &str = include_str!("../docs/examples/rules/structured-yaml-value.rule.ts");

fn cli(args: &[&str], policy: &[u8], root: &Path) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_rulecheck-evaluator"))
        .current_dir(root)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("run rulecheck");
    child.stdin.take().unwrap().write_all(policy).unwrap();
    child.wait_with_output().unwrap()
}

fn run(output: Output, code: i32) -> String {
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert_eq!(
        output.status.code(),
        Some(code),
        "stdout: {stdout}\nstderr: {stderr}"
    );
    if code == 2 { stderr } else { stdout }
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
            "structured fixture",
        ],
    );
    git(root, &["rev-parse", "HEAD"])
}

fn write(root: &Path, path: &str, content: &str) {
    let path = root.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

fn repo() -> tempfile::TempDir {
    let repo = tempfile::tempdir().unwrap();
    git(repo.path(), &["init", "-q"]);
    repo
}

/// Builds `.rulecheck/rules` sources with the shipped builder.
fn built(root: &Path, rules: &[(&str, &str)]) -> Vec<u8> {
    for (name, source) in rules {
        write(root, &format!(".rulecheck/rules/{name}"), source);
    }
    let output = Command::new("node")
        .arg(Path::new(ROOT).join("tools/build.mjs"))
        .arg("--root")
        .arg(root)
        .output()
        .expect("run build; install Node.js and run npm ci");
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    output.stdout
}

fn policy(rules: &[Value]) -> Vec<u8> {
    json!({ "version": 1, "rules": rules })
        .to_string()
        .into_bytes()
}

/// A rule over `*.yml` with one passing empty-mapping fixture.
fn rule(id: &str, path: Value, assert: Value) -> Value {
    json!({
        "id": id, "kind": "structured", "paths": ["**/*.yml"],
        "structure": { "format": "yaml", "path": path, "assert": assert },
        "remediation": "Fix the configuration.",
        "fixtures": [{ "path": "a.yml", "content": "{}\n", "expectAll": [] }]
    })
}

fn findings(result: &str) -> Vec<(String, String, u64, u64, u64, u64)> {
    let result: Value = serde_json::from_str(result).unwrap();
    result["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|finding| {
            (
                finding["ruleId"].as_str().unwrap().to_owned(),
                finding["path"].as_str().unwrap().to_owned(),
                finding["line"].as_u64().unwrap(),
                finding["column"].as_u64().unwrap(),
                finding["byteStart"].as_u64().unwrap(),
                finding["byteEnd"].as_u64().unwrap(),
            )
        })
        .collect()
}

/// Runs `check` in the working tree and at `sha` with the same arguments
/// and asserts that both report the same exit code and output.
fn both_modes(root: &Path, policy: &[u8], sha: &str, extra: &[&str], code: i32) -> String {
    let mut tree = vec!["check"];
    tree.extend_from_slice(extra);
    let mut exact = vec!["check", "--commit", sha];
    exact.extend_from_slice(extra);
    let working = run(cli(&tree, policy, root), code);
    let committed = run(cli(&exact, policy, root), code);
    // JSON results differ only in where they say they read from.
    let comparable = |output: &str| match serde_json::from_str::<Value>(output) {
        Ok(mut value) => {
            value.as_object_mut().unwrap().remove("source");
            value.to_string()
        }
        Err(_) => output.to_owned(),
    };
    assert_eq!(comparable(&working), comparable(&committed));
    working
}

#[test]
fn cookbook_rules_pass_their_fixtures_and_match_the_schema() {
    let repo = repo();
    let policy = built(
        repo.path(),
        &[
            ("structured-yaml.rule.ts", TIMEOUT),
            ("structured-yaml-value.rule.ts", VALUE),
        ],
    );
    assert_eq!(
        run(cli(&["test"], &policy, repo.path()), 0),
        "16 fixtures passed\n"
    );
    let schema: Value = serde_json::from_str(include_str!("../schema/policy-v1.json")).unwrap();
    let value: Value = serde_json::from_slice(&policy).unwrap();
    assert_eq!(common::violations(&schema, &value), Vec::<String>::new());
    let timeout = value["rules"]
        .as_array()
        .unwrap()
        .iter()
        .find(|rule| rule["id"] == "ci/job-timeout")
        .unwrap();
    assert_eq!(
        timeout["structure"],
        json!({
            "format": "yaml",
            "path": [{ "key": "jobs" }, { "each": "mapping" }],
            "assert": { "kind": "has-key", "key": "timeout-minutes" }
        })
    );
}

const WORKFLOW: &str = "\
name: ci
on: [push]
permissions:
  contents: write
jobs:
  build:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: Test
        run: make test
  lint:
    runs-on: ubuntu-latest
    timeout-minutes: 5
  \"docs\": {runs-on: ubuntu-latest}
";

/// The same findings, locations and byte ranges in both modes, twice.
#[test]
fn working_tree_and_exact_commit_report_the_same_locations() {
    let repo = repo();
    let root = repo.path();
    let policy = built(
        root,
        &[
            ("structured-yaml.rule.ts", TIMEOUT),
            ("structured-yaml-value.rule.ts", VALUE),
        ],
    );
    write(root, ".github/workflows/ci.yml", WORKFLOW);
    write(
        root,
        ".github/workflows/multi.yaml",
        "jobs:\n  a:\n    timeout-minutes: 1\n---\npermissions:\n  contents: 'write'\njobs:\n  b: {}\n",
    );
    write(root, "deploy/app.yml", "jobs:\n  x: {}\n");
    let sha = commit(root);

    let first = both_modes(root, &policy, &sha, &["--format", "json"], 1);
    assert_eq!(
        first,
        both_modes(root, &policy, &sha, &["--format", "json"], 1)
    );
    let path = ".github/workflows/ci.yml".to_owned();
    let multi = ".github/workflows/multi.yaml".to_owned();
    let multi_text = fs::read_to_string(root.join(&multi)).unwrap();
    assert_eq!(&multi_text[63..70], "'write'");
    assert_eq!(&multi_text[79..80], "b");
    let timeout = "ci/job-timeout".to_owned();
    let write_access = "ci/no-contents-write".to_owned();
    assert_eq!(
        findings(&first),
        [
            (write_access.clone(), path.clone(), 4, 13, 45, 50),
            (timeout.clone(), path.clone(), 6, 3, 59, 64),
            (write_access.clone(), multi.clone(), 6, 13, 63, 70),
            (timeout.clone(), multi.clone(), 8, 3, 79, 80),
        ]
    );
    // The ranges cover the job key and the value, quotes included.
    assert_eq!(&WORKFLOW[59..64], "build");
    assert_eq!(&WORKFLOW[45..50], "write");

    // Every failing job is counted with --all-matches.
    let all = both_modes(root, &policy, &sha, &["--all-matches"], 1);
    assert_eq!(
        all,
        ".github/workflows/ci.yml:4:13: [ci/no-contents-write] Grant contents: write only to the job that needs it, or use read.\n\
         .github/workflows/ci.yml:6:3: [ci/job-timeout] Add timeout-minutes to the job.\n\
         .github/workflows/ci.yml:15:3: [ci/job-timeout] Add timeout-minutes to the job.\n\
         .github/workflows/multi.yaml:6:13: [ci/no-contents-write] Grant contents: write only to the job that needs it, or use read.\n\
         .github/workflows/multi.yaml:8:3: [ci/job-timeout] Add timeout-minutes to the job.\n"
    );

    // The repair makes both modes clean.
    write(
        root,
        ".github/workflows/ci.yml",
        &WORKFLOW
            .replace("contents: write", "contents: read")
            .replace("  build:\n", "  build:\n    timeout-minutes: 30\n")
            .replace("{runs-on: ubuntu-latest}", "{timeout-minutes: 2}"),
    );
    write(
        root,
        ".github/workflows/multi.yaml",
        "jobs:\n  a:\n    timeout-minutes: 1\n",
    );
    let sha = commit(root);
    both_modes(root, &policy, &sha, &[], 0);
}

/// Each assertion, a missing path, every document, and sequence selection.
#[test]
fn assertions_follow_the_documented_semantics() {
    let rules = [
        rule(
            "a/present",
            json!([{ "key": "on" }, { "key": "push" }]),
            json!({ "kind": "present" }),
        ),
        rule(
            "a/absent",
            json!([{ "key": "jobs" }, { "each": "mapping" }, { "key": "continue-on-error" }]),
            json!({ "kind": "absent" }),
        ),
        rule(
            "a/equals",
            json!([{ "key": "jobs" }, { "each": "mapping" }, { "key": "runs-on" }]),
            json!({ "kind": "equals", "value": "ubuntu-24.04" }),
        ),
        rule(
            "a/first-step",
            json!([{ "key": "jobs" }, { "each": "mapping" }, { "key": "steps" }, { "index": 0 }]),
            json!({ "kind": "has-key", "key": "uses" }),
        ),
        rule(
            "a/step-name",
            json!([{ "key": "jobs" }, { "each": "mapping" }, { "key": "steps" }, { "each": "sequence" }]),
            json!({ "kind": "has-key", "key": "name" }),
        ),
    ];
    let policy = policy(&rules);
    let repo = repo();
    let root = repo.path();
    let content = "\
on:
  pull_request: {}
jobs:
  a:
    runs-on: ubuntu-24.04
    continue-on-error: true
    steps:
      - run: make
      - name: Named
        uses: x/y@v1
  b:
    runs-on: [self-hosted]
    steps: not-a-list
---
on: {push: {}}
jobs:
  c: {runs-on: 'ubuntu-24.04', steps: [{uses: a, name: b}]}
";
    write(root, "w.yml", content);
    write(root, "empty.yml", "");
    write(root, "comment.yml", "# nothing here\n");
    let sha = commit(root);
    let result = both_modes(
        root,
        &policy,
        &sha,
        &["--all-matches", "--format", "json"],
        1,
    );
    let got: Vec<_> = findings(&result)
        .into_iter()
        .map(|(id, path, line, column, ..)| format!("{path}:{line}:{column} {id}"))
        .collect();
    assert_eq!(
        got,
        [
            // A file without documents has nothing present.
            "comment.yml:1:1 a/present",
            "empty.yml:1:1 a/present",
            // present reports at the deepest node the path reached.
            "w.yml:2:3 a/present",
            "w.yml:6:5 a/absent",
            // The first step of job a lacks uses; the run step has no name.
            "w.yml:8:9 a/first-step",
            "w.yml:8:9 a/step-name",
            // A sequence is not the scalar ubuntu-24.04.
            "w.yml:12:14 a/equals",
        ]
    );
}

/// Inputs that cannot be judged exit 2 in both modes, never pass silently.
#[test]
fn unjudgeable_yaml_fails_closed() {
    let cases = [
        (
            "dup.yml",
            "jobs:\n  a: {}\n  a: {}\n",
            "duplicate YAML key \"a\" at 3:3",
        ),
        ("bad.yml", "jobs: [\n", "cannot parse as YAML near 2:1"),
        ("tab.yml", "jobs:\n\ta: 1\n", "cannot parse as YAML near"),
        (
            "alias.yml",
            "shared: &s {timeout-minutes: 5}\njobs:\n  a: *s\n",
            "reaches a YAML alias at 3:6",
        ),
        (
            "merge.yml",
            "shared: &s {timeout-minutes: 5}\njobs:\n  a:\n    <<: *s\n",
            "reaches a mapping with a << merge key at 4:5",
        ),
        (
            "deep.yml",
            &format!("jobs: {}{}\n", "[".repeat(200), "]".repeat(200)),
            "nests deeper than 128 levels",
        ),
        ("binary.yml", "jobs: \u{0}\n", "cannot parse as YAML"),
        ("notes.txt", "jobs: {}\n", "apply to .yml and .yaml files"),
    ];
    for (path, content, message) in cases {
        let mut rule = rule(
            "a/timeout",
            json!([{ "key": "jobs" }, { "each": "mapping" }]),
            json!({ "kind": "has-key", "key": "timeout-minutes" }),
        );
        rule["paths"] = json!([path]);
        let bytes = policy(&[rule]);
        let repo = repo();
        let root = repo.path();
        write(root, path, content);
        let sha = commit(root);
        let error = both_modes(root, &bytes, &sha, &[], 2);
        assert!(
            error.contains(&format!("[a/timeout] {path}: ")) && error.contains(message),
            "{path}: {error}"
        );
    }

    // An alias off the path is fine: Rulecheck never expands it.
    let bytes = policy(&[rule(
        "a/timeout",
        json!([{ "key": "jobs" }, { "each": "mapping" }]),
        json!({ "kind": "has-key", "key": "timeout-minutes" }),
    )]);
    let repo = repo();
    let root = repo.path();
    write(
        root,
        "ok.yml",
        "defaults: &d {x: 1}\nother: *d\njobs:\n  a: {timeout-minutes: 1}\n",
    );
    let sha = commit(root);
    both_modes(root, &bytes, &sha, &[], 0);
}

/// The loader and the schema reject the same malformed structures.
#[test]
fn malformed_structures_are_rejected_by_schema_and_loader() {
    let schema: Value = serde_json::from_str(include_str!("../schema/policy-v1.json")).unwrap();
    let base = rule(
        "a/x",
        json!([{ "key": "jobs" }]),
        json!({ "kind": "present" }),
    );
    let with = |pointer: &str, value: Value| {
        let mut rule = base.clone();
        *rule.pointer_mut(pointer).unwrap() = value;
        rule
    };
    let mut missing = base.clone();
    missing.as_object_mut().unwrap().remove("structure");
    let mut extra = base.clone();
    extra["pattern"] = json!({ "kind": "literal", "value": "x" });
    let mut no_content = base.clone();
    no_content["fixtures"] = json!([{ "path": "a.yml", "expect": null }]);
    let cases = [
        (missing, "structured rules require a structure"),
        (extra, "structured rules require a structure"),
        (no_content, "content only for text fixtures"),
        (with("/structure/format", json!("json")), "unknown variant"),
        (
            with("/structure/path/0", json!({ "key": "a", "index": 0 })),
            "exactly one of key, index or each",
        ),
        (
            with("/structure/path/0", json!({})),
            "exactly one of key, index or each",
        ),
        (
            with("/structure/path/0", json!({ "each": "list" })),
            "unknown variant",
        ),
        (
            with("/structure/path/0", json!({ "index": -1 })),
            "invalid value",
        ),
        (
            with("/structure/path", json!([])),
            "need at least one path segment",
        ),
        (
            with(
                "/structure/path",
                json!(vec![json!({ "each": "mapping" }); 17]),
            ),
            "at most 16",
        ),
        (
            with("/structure/assert", json!({ "kind": "equals" })),
            "missing field `value`",
        ),
        (
            with(
                "/structure/assert",
                json!({ "kind": "present", "value": "x" }),
            ),
            "unknown field",
        ),
        (
            with("/structure/assert", json!({ "kind": "has-key", "key": "" })),
            "1 to 1024 bytes",
        ),
        (
            with(
                "/structure/assert",
                json!({ "kind": "matches", "value": "x" }),
            ),
            "unknown variant",
        ),
    ];
    for (rule, message) in cases {
        let bytes = policy(std::slice::from_ref(&rule));
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert_ne!(
            common::violations(&schema, &value),
            Vec::<String>::new(),
            "schema accepts {rule}"
        );
        let error = run(cli(&["validate"], &bytes, Path::new(ROOT)), 2);
        assert!(error.contains(message), "{rule}: {error}");
    }
    let bytes = policy(&[base]);
    run(cli(&["validate"], &bytes, Path::new(ROOT)), 0);
}

/// A changed structure is a new matcher digest, so baselines notice it.
#[test]
fn structured_findings_can_be_recorded_in_a_baseline() {
    let repo = repo();
    let root = repo.path();
    let policy = built(root, &[("structured-yaml.rule.ts", TIMEOUT)]);
    write(root, ".github/workflows/ci.yml", WORKFLOW);
    commit(root);
    run(
        cli(
            &["baseline", "--write", "--baseline", "b.json"],
            &policy,
            root,
        ),
        0,
    );
    let baseline: Value =
        serde_json::from_str(&fs::read_to_string(root.join("b.json")).unwrap()).unwrap();
    let text = baseline.to_string();
    assert!(text.contains("\"ci/job-timeout\""), "{text}");
    assert!(text.contains("\".github/workflows/ci.yml\":2"), "{text}");
    run(cli(&["check", "--baseline", "b.json"], &policy, root), 0);
}

/// A 1 MiB workflow with thousands of jobs, and maximal nesting, stay fast.
#[test]
fn large_and_nested_files_are_judged_in_bounded_time() {
    let mut big = String::from("jobs:\n");
    let mut job = 0;
    while big.len() < 1000 * 1024 {
        big.push_str(&format!(
            "  job-{job}:\n    runs-on: ubuntu-latest\n    steps:\n      - name: s\n        run: echo {job}\n"
        ));
        if job % 2 == 0 {
            big.push_str("    timeout-minutes: 5\n");
        }
        job += 1;
    }
    let nested = format!(
        "jobs:\n{}",
        (0..120)
            .map(|level| format!("{}k:\n", " ".repeat(2 + level * 2)))
            .collect::<String>()
    );
    let repo = repo();
    let root = repo.path();
    let policy = built(root, &[("structured-yaml.rule.ts", TIMEOUT)]);
    write(root, ".github/workflows/big.yml", &big);
    write(root, ".github/workflows/nested.yml", &nested);
    let sha = commit(root);
    let started = Instant::now();
    let all = both_modes(root, &policy, &sha, &["--all-matches"], 1);
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "{:?}",
        started.elapsed()
    );
    // Odd jobs lack the key, plus the nested file's only job.
    assert_eq!(all.lines().count(), job / 2 + 1);
}

/// The builder points at the offending structure property in the rule file.
#[test]
fn builder_reports_structure_mistakes_at_their_source() {
    let cases = [
        (
            r#"{ each: "mapping" }"#,
            r#"{ each: "list" }"#,
            "12:31: structure path 2 each must be \"mapping\" or \"sequence\".",
        ),
        (
            r#"{ each: "mapping" }"#,
            r#"{ key: "a", index: 1 }"#,
            "12:29: structure path 2 must be an object with exactly one of key, index or each.",
        ),
        (
            r#"kind: "has-key", key: "timeout-minutes""#,
            r#"kind: "has-key""#,
            "13:13: Missing property key in the structure assert. Add key.",
        ),
        (
            r#"format: "yaml""#,
            r#"format: "json""#,
            "10:5: structure format must be \"yaml\".",
        ),
    ];
    for (from, to, message) in cases {
        let repo = repo();
        let root = repo.path();
        write(
            root,
            ".rulecheck/rules/timeout.rule.ts",
            &TIMEOUT.replace(from, to),
        );
        let output = Command::new("node")
            .arg(Path::new(ROOT).join("tools/build.mjs"))
            .arg("--root")
            .arg(root)
            .output()
            .unwrap();
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert_eq!(output.status.code(), Some(2), "{stderr}");
        assert!(
            stderr.contains(&format!(".rulecheck/rules/timeout.rule.ts:{message}")),
            "{stderr}"
        );
    }
}
