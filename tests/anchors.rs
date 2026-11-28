//! Forbidden-path anchors: an absent forbidden file stays clean, while a rule
//! whose guarded directory moved or vanished cannot judge and exits 2, with the
//! same result in a working-tree check and an exact-commit check.
mod common;

use serde_json::{Value, json};
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

const ROOT: &str = env!("CARGO_MANIFEST_DIR");
const ID: &str = "config/no-local-overrides";
const MISSING: &str = "[config/no-local-overrides] missing anchor: no file matches the rule's \
anchors, so the area it guards moved or vanished and an absent forbidden path proves nothing; \
restore the anchor or update anchors and paths together\n\
rulecheck: one or more forbidden-path rules found none of their anchors\n";

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

/// Checks the exit code and returns (stdout, stderr).
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
            "Anchor fixture",
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

fn rule(anchors: Option<Value>) -> Value {
    let mut rule = json!({
        "id": ID,
        "kind": "forbidden-path",
        "paths": ["services/api/config/*.local.toml"],
        "remediation": "Keep local overrides out of Git.",
        "fixtures": [
            { "path": "services/api/config/app.toml", "expect": null },
            { "path": "services/api/config/dev.local.toml", "expect": { "line": 1, "column": 1 } },
        ],
    });
    if let Some(anchors) = anchors {
        rule["anchors"] = anchors;
    }
    rule
}

fn policy(rules: &[Value]) -> Vec<u8> {
    json!({ "version": 1, "rules": rules })
        .to_string()
        .into_bytes()
}

fn anchored() -> Vec<u8> {
    policy(&[rule(Some(json!(["services/api/config/app.toml"])))])
}

/// Runs `check` in the working tree and at `sha`, asserts both exit `code`
/// with identical output, and returns (stdout, stderr).
fn both_modes(policy: &[u8], root: &Path, sha: &str, code: i32) -> (String, String) {
    let tree = run(cli(&["check"], policy, root), code);
    let exact = run(cli(&["check", "--commit", sha], policy, root), code);
    assert_eq!(tree, exact, "working tree and --commit {sha} disagree");
    tree
}

#[test]
fn an_absent_forbidden_file_is_clean_when_its_anchor_exists() {
    let repo = repo();
    let root = repo.path();
    write(root, "services/api/config/app.toml", "port = 8080\n");
    git(root, &["add", "."]);
    let sha = commit(root);
    let policy = anchored();
    assert_eq!(run(cli(&["validate"], &policy, root), 0).0, "");
    assert_eq!(
        run(cli(&["test"], &policy, root), 0).0,
        "2 fixtures passed\n"
    );
    assert_eq!(
        both_modes(&policy, root, &sha, 0),
        (String::new(), String::new())
    );

    write(root, "services/api/config/dev.local.toml", "");
    git(root, &["add", "."]);
    let sha = commit(root);
    assert_eq!(
        both_modes(&policy, root, &sha, 1).0,
        "services/api/config/dev.local.toml:1:1: [config/no-local-overrides] Keep local overrides out of Git.\n"
    );
}

#[test]
fn a_moved_directory_exits_2_instead_of_passing() {
    let repo = repo();
    let root = repo.path();
    write(root, "services/api/config/app.toml", "port = 8080\n");
    git(root, &["add", "."]);
    commit(root);
    // The service is renamed and a local override lands at its new path.
    git(root, &["mv", "services/api", "services/gateway"]);
    write(root, "services/gateway/config/dev.local.toml", "");
    git(root, &["add", "."]);
    let sha = commit(root);

    // Without anchors the stale rule passes: the v1 default is unchanged.
    let unanchored = policy(&[rule(None)]);
    assert_eq!(
        both_modes(&unanchored, root, &sha, 0),
        (String::new(), String::new())
    );

    let (stdout, stderr) = both_modes(&anchored(), root, &sha, 2);
    assert_eq!(stdout, "");
    assert_eq!(stderr, MISSING);
}

#[test]
fn a_vanished_directory_exits_2_and_a_deleted_anchor_counts_as_missing() {
    let repo = repo();
    let root = repo.path();
    write(root, "services/api/config/app.toml", "port = 8080\n");
    write(root, "services/web/index.ts", "export {};\n");
    git(root, &["add", "."]);
    commit(root);
    // Deleted on disk but still in the index: the working tree skips the
    // deletion, as it does for scope candidates, so the anchor is missing.
    fs::remove_dir_all(root.join("services/api")).unwrap();
    let policy = anchored();
    assert_eq!(
        run(cli(&["check"], &policy, root), 2),
        (String::new(), MISSING.into())
    );
    git(root, &["add", "-A"]);
    let sha = commit(root);
    assert_eq!(
        both_modes(&policy, root, &sha, 2),
        (String::new(), MISSING.into())
    );
}

#[test]
fn any_matching_anchor_is_enough_and_findings_still_print() {
    let repo = repo();
    let root = repo.path();
    write(root, "services/api/config/app.toml", "");
    write(root, "services/api/config/dev.local.toml", "");
    git(root, &["add", "."]);
    let sha = commit(root);
    // A glob that matches the anchor, alongside one that matches nothing.
    let policy = policy(&[rule(Some(json!([
        "services/gone/config/app.toml",
        "services/*/config/app.toml"
    ])))]);
    let (stdout, _) = both_modes(&policy, root, &sha, 1);
    assert!(
        stdout.starts_with("services/api/config/dev.local.toml:1:1:"),
        "{stdout}"
    );

    // With the anchor gone the finding is still printed, but the check exits 2.
    git(root, &["rm", "-q", "services/api/config/app.toml"]);
    let sha = commit(root);
    let (stdout, stderr) = both_modes(&policy, root, &sha, 2);
    assert!(
        stdout.starts_with("services/api/config/dev.local.toml:1:1:"),
        "{stdout}"
    );
    assert_eq!(stderr, MISSING);
}

#[test]
fn json_reports_the_missing_anchor_per_rule() {
    let repo = repo();
    let root = repo.path();
    write(root, "README.md", "synthetic\n");
    git(root, &["add", "."]);
    let sha = commit(root);
    let policy = anchored();
    for args in [
        vec!["check", "--format", "json"],
        vec!["check", "--format", "json", "--commit", &sha],
    ] {
        let (stdout, stderr) = run(cli(&args, &policy, root), 2);
        assert_eq!(stderr, MISSING);
        let result: Value = serde_json::from_str(&stdout).unwrap();
        let schema: Value = serde_json::from_str(include_str!("../schema/result-v1.json")).unwrap();
        assert_eq!(common::violations(&schema, &result), Vec::<String>::new());
        assert_eq!(result["outcome"], "error");
        assert_eq!(result["findings"], json!([]));
        let diagnostics = result["diagnostics"].as_array().unwrap();
        assert_eq!(diagnostics.len(), 2);
        assert_eq!(diagnostics[0]["ruleId"], ID);
        assert!(
            diagnostics[0]["message"]
                .as_str()
                .unwrap()
                .starts_with("missing anchor:")
        );
        assert_eq!(diagnostics[1]["ruleId"], Value::Null);
    }
}

#[test]
fn baselines_need_anchors_and_ignore_them_in_the_matcher_digest() {
    let repo = repo();
    let root = repo.path();
    write(root, "services/api/config/app.toml", "");
    write(root, "services/api/config/dev.local.toml", "");
    git(root, &["add", "."]);
    commit(root);
    // A baseline recorded before anchors were added still applies.
    let (recorded, _) = run(cli(&["baseline"], &policy(&[rule(None)]), root), 0);
    let file = root.join("baseline.json");
    fs::write(&file, &recorded).unwrap();
    let baseline = file.to_str().unwrap();
    let policy = anchored();
    assert_eq!(run(cli(&["baseline"], &policy, root), 0).0, recorded);
    assert_eq!(
        run(cli(&["check", "--baseline", baseline], &policy, root), 0).0,
        ""
    );

    fs::remove_file(root.join("services/api/config/app.toml")).unwrap();
    let (stdout, stderr) = run(cli(&["baseline"], &policy, root), 2);
    assert_eq!(stdout, "");
    assert!(
        stderr.ends_with(
            "rulecheck: one or more forbidden-path rules found none of their anchors; a baseline \
             is recorded only from a complete scan, so nothing was written\n"
        ),
        "{stderr}"
    );
    run(cli(&["check", "--baseline", baseline], &policy, root), 2);
}

#[test]
fn anchors_are_validated() {
    let root = Path::new(ROOT);
    let text = json!({
        "id": "source/no-marker",
        "kind": "text",
        "paths": ["src/**"],
        "anchors": ["src/main.rs"],
        "pattern": { "kind": "literal", "value": "MARKER" },
        "remediation": "Remove the marker.",
        "fixtures": [{ "path": "src/a.rs", "content": "MARKER", "expect": { "line": 1, "column": 1 } }],
    });
    for (rule, message) in [
        (
            text,
            "anchors apply to forbidden-path rules only; text and syntax rules already exit 2 \
             when no file is in scope",
        ),
        (
            rule(Some(json!([]))),
            "provide 1 to 16 anchor globs or omit anchors",
        ),
        (rule(Some(json!(["../app.toml"]))), "invalid anchor glob"),
        (rule(Some(json!(["services/[api"]))), "invalid anchor glob"),
        (
            rule(Some(json!(["services/api/config/*.toml"]))),
            "services/api/config/dev.local.toml: a forbidden path must not match the rule's anchors",
        ),
    ] {
        let (_, stderr) = run(cli(&["validate"], &policy(&[rule]), root), 2);
        assert!(stderr.contains(message), "{stderr}");
    }

    let schema: Value = serde_json::from_str(include_str!("../schema/policy-v1.json")).unwrap();
    let accepted: Value = serde_json::from_slice(&anchored()).unwrap();
    assert_eq!(common::violations(&schema, &accepted), Vec::<String>::new());
    let text_rule = json!({
        "id": "config/no-marker",
        "kind": "text",
        "paths": ["config/**"],
        "anchors": ["config/app.toml"],
        "pattern": { "kind": "literal", "value": "MARKER" },
        "remediation": "Remove the marker.",
        "fixtures": [{ "path": "config/a.toml", "content": "", "expect": null }],
    });
    let rejected = json!({ "version": 1, "rules": [text_rule] });
    assert_ne!(common::violations(&schema, &rejected), Vec::<String>::new());
}

#[test]
fn the_builder_accepts_anchors_on_forbidden_path_rules_only() {
    let repo = repo();
    let root = repo.path();
    let rules = root.join(".rulecheck/rules");
    fs::create_dir_all(&rules).unwrap();
    let source = |kind: &str, extra: &str| {
        format!(
            "import {{ defineRule }} from \"@pmdroid/rulecheck\";\n\n\
             export default defineRule({{\n  id: \"config/no-local\",\n  kind: \"{kind}\",\n  \
             paths: [\"config/*.local.toml\"],\n  anchors: [\"config/app.toml\"],\n{extra}  \
             remediation: \"Keep local files out of Git.\",\n  fixtures: [{{ path: \"config/a.local.toml\", \
             expect: {{ line: 1, column: 1 }} }}],\n}});\n"
        )
    };
    let file = rules.join("local.rule.ts");
    fs::write(&file, source("forbidden-path", "")).unwrap();
    let build = |root: &Path| {
        Command::new("node")
            .arg(Path::new(ROOT).join("tools/build.mjs"))
            .arg("--root")
            .arg(root)
            .output()
            .unwrap()
    };
    let output = build(root);
    assert!(output.status.success(), "{output:?}");
    let built: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(built["rules"][0]["anchors"], json!(["config/app.toml"]));

    fs::write(
        &file,
        source("text", "  pattern: { kind: \"literal\", value: \"x\" },\n"),
    )
    .unwrap();
    let output = build(root);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("7:3: Only forbidden-path rules have anchors"),
        "{stderr}"
    );
}

#[test]
fn anchors_are_checked_after_a_text_budget_stop() {
    let repo = repo();
    let root = repo.path();
    let chunk = "x".repeat(700 * 1024);
    write(root, "docs/a.txt", &chunk);
    write(root, "docs/b.txt", &chunk);
    // Sorted after the file that stops the scan.
    write(root, "services/api/config/app.toml", "");
    git(root, &["add", "."]);
    let sha = commit(root);
    let text = json!({
        "id": "docs/no-marker",
        "kind": "text",
        // The anchor is in this rule's scope too, so it is a candidate that
        // the stop leaves unread.
        "paths": ["docs/*.txt", "services/**"],
        "pattern": { "kind": "literal", "value": "MARKER" },
        "remediation": "Remove the marker.",
        "fixtures": [{ "path": "docs/a.txt", "content": "MARKER", "expect": { "line": 1, "column": 1 } }],
    });
    let policy = policy(&[text, rule(Some(json!(["services/api/config/app.toml"])))]);
    for sha in [None, Some(sha.as_str())] {
        let mut args = vec!["check", "--text-budget", "1"];
        args.extend(sha.iter().flat_map(|sha| ["--commit", *sha]));
        let (_, stderr) = run(cli(&args, &policy, root), 2);
        assert!(
            stderr.starts_with("[docs/no-marker] not evaluated on every file in scope"),
            "{stderr}"
        );
        assert!(!stderr.contains("missing anchor"), "{stderr}");
    }
}
