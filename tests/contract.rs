//! Pins the published Rulecheck v1 scan and policy contract: path scope and
//! exclusion, case handling, working-tree inventory, rule sources as
//! candidates, first match per rule and file, non-regular inputs, and
//! schema/SDK/evaluator field parity.
use serde_json::{Value, json};
use std::fs;
use std::io::Write;
use std::os::unix::fs::symlink;
use std::path::Path;
use std::process::{Command, Output, Stdio};

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn cli(args: &[&str], policy: &[u8], root: &Path) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_rulecheck"))
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
            "Rulecheck contract fixture",
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

/// Runs a command and returns stdout after checking the exit code.
fn run(output: Output, code: i32) -> String {
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert_eq!(
        output.status.code(),
        Some(code),
        "stdout: {stdout}\nstderr: {stderr}"
    );
    if code == 2 {
        return stderr;
    }
    stdout
}

fn policy(rules: &[Value]) -> Vec<u8> {
    json!({ "version": 1, "rules": rules })
        .to_string()
        .into_bytes()
}

fn marker_rule(paths: &[&str], exclude: Option<&[&str]>) -> Value {
    let mut rule = json!({
        "id": "source/no-marker", "kind": "text", "paths": paths,
        "pattern": { "kind": "literal", "value": "legacyMarker" },
        "remediation": "Remove the marker.",
        "fixtures": [
            { "path": "src/app.ts", "content": "legacyMarker();\n", "expect": { "line": 1, "column": 1 } },
            { "path": "src/gen/app.ts", "content": "legacyMarker();\n", "expect": null }
        ]
    });
    if let Some(exclude) = exclude {
        rule["exclude"] = json!(exclude);
    }
    rule
}

#[test]
fn exclude_removes_files_from_checks_and_fixtures() {
    let repo = repo();
    let root = repo.path();
    write(root, "src/app.ts", "currentMarker();\n");
    write(root, "src/gen/app.ts", "legacyMarker();\n");
    write(root, "src/GEN/app.ts", "legacyMarker();\n");
    git(root, &["add", "-A"]);

    let excluded = policy(&[marker_rule(&["src/**/*.ts"], Some(&["src/gen/**"]))]);
    assert_eq!(run(cli(&["validate"], &excluded, root), 0), "");
    assert_eq!(
        run(cli(&["test"], &excluded, root), 0),
        "2 fixtures passed\n"
    );
    // Exclude globs are case-sensitive like paths: src/GEN stays in scope.
    assert_eq!(
        run(cli(&["check"], &excluded, root), 1),
        "src/GEN/app.ts:1:1: [source/no-marker] Remove the marker.\n"
    );

    // The same fixture fails without exclude, so fixtures honour exclusion.
    let unscoped = policy(&[marker_rule(&["src/**/*.ts"], None)]);
    let output = run(cli(&["test"], &unscoped, root), 1);
    assert!(output.starts_with("src/gen/app.ts:1:1: [source/no-marker] fixture 2 expected no violation, found a violation at 1:1."), "{output}");

    // Excluding every scoped file leaves an empty text scope, which cannot be judged.
    let all = policy(&[marker_rule(&["src/**/*.ts"], Some(&["src/**"]))]);
    let error = run(cli(&["check"], &all, root), 2);
    assert!(
        error.contains("[source/no-marker] empty text scope"),
        "{error}"
    );

    // Exclusion applies to forbidden-path rules too.
    let path_rule = json!({
        "id": "path/no-generated", "kind": "forbidden-path",
        "paths": ["src/**"], "exclude": ["src/*.ts"],
        "remediation": "Do not commit generated files.",
        "fixtures": [
            { "path": "src/app.ts", "expect": null },
            { "path": "src/gen/app.ts", "expect": { "line": 1, "column": 1 } }
        ]
    });
    let paths = policy(&[path_rule]);
    assert_eq!(run(cli(&["test"], &paths, root), 0), "2 fixtures passed\n");
    assert_eq!(
        run(cli(&["check"], &paths, root), 1),
        concat!(
            "src/GEN/app.ts:1:1: [path/no-generated] Do not commit generated files.\n",
            "src/gen/app.ts:1:1: [path/no-generated] Do not commit generated files.\n"
        )
    );
}

#[test]
fn exclude_is_validated_like_paths() {
    let root = Path::new(ROOT);
    for (exclude, message) in [
        (json!([]), "provide 1 to 16 exclude globs or omit exclude"),
        (
            json!(vec!["a"; 17]),
            "provide 1 to 16 exclude globs or omit exclude",
        ),
        (json!(null), "invalid policy JSON"),
        (json!("src/**"), "invalid policy JSON"),
        (json!(["../src/**"]), "invalid exclude glob \"../src/**\""),
        (json!(["src\\gen"]), "invalid exclude glob"),
        (json!(["/src/**"]), "invalid exclude glob"),
        (json!(["src/[gen"]), "invalid exclude glob"),
    ] {
        let mut rule = marker_rule(&["src/**/*.ts"], None);
        rule["exclude"] = exclude;
        let error = run(cli(&["validate"], &policy(&[rule]), root), 2);
        assert!(error.contains(message), "{error}");
    }
}

#[test]
fn paths_and_text_are_case_sensitive() {
    let repo = repo();
    let root = repo.path();
    write(root, "src/ok.ts", "currentMarker();\n");
    write(root, "SRC/upper-dir.ts", "legacyMarker();\n");
    write(root, "src/upper-ext.TS", "legacyMarker();\n");
    write(root, "src/lower-text.ts", "legacymarker();\n");
    git(root, &["add", "-A"]);

    let literal = policy(&[marker_rule(&["src/**/*.ts"], None)]);
    assert_eq!(run(cli(&["check"], &literal, root), 0), "");

    let mut insensitive = marker_rule(&["src/**/*.ts"], None);
    insensitive["pattern"] = json!({ "kind": "regex", "value": "(?i)legacymarker" });
    assert_eq!(
        run(cli(&["check"], &policy(&[insensitive]), root), 1),
        "src/lower-text.ts:1:1: [source/no-marker] Remove the marker.\n"
    );
}

#[test]
fn working_tree_inventory_is_tracked_regular_files_minus_deletions() {
    let repo = repo();
    let root = repo.path();
    write(root, ".gitignore", "ignored/\n");
    write(root, "src/app.ts", "currentMarker();\n");
    write(root, "src/staged-delete.ts", "currentMarker();\n");
    write(root, "src/unstaged-delete.ts", "currentMarker();\n");
    write(root, "ignored/forced.ts", "currentMarker();\n");
    git(root, &["add", "-A"]);
    git(root, &["add", "-f", "ignored/forced.ts"]);
    commit(root);
    let rule = policy(&[marker_rule(&["**/*.ts"], None)]);
    let banned = policy(&[json!({
        "id": "path/no-banned", "kind": "forbidden-path", "paths": ["**/banned.txt"],
        "remediation": "Remove banned.txt.",
        "fixtures": [{ "path": "banned.txt", "expect": { "line": 1, "column": 1 } }]
    })]);

    // Untracked files, ignored or not, are outside the v1 working-tree inventory.
    write(root, "src/untracked.ts", "legacyMarker();\n");
    write(root, "ignored/untracked.ts", "legacyMarker();\n");
    write(root, "banned.txt", "");
    assert_eq!(run(cli(&["check"], &rule, root), 0), "");
    assert_eq!(run(cli(&["check"], &banned, root), 0), "");
    git(root, &["add", "banned.txt"]);
    assert_eq!(
        run(cli(&["check"], &banned, root), 1),
        "banned.txt:1:1: [path/no-banned] Remove banned.txt.\n"
    );

    // Tracked files are scanned with working-tree contents, even when ignored.
    write(root, "ignored/forced.ts", "legacyMarker();\n");
    write(root, "src/staged-delete.ts", "legacyMarker();\n");
    write(root, "src/unstaged-delete.ts", "legacyMarker();\n");
    assert_eq!(
        run(cli(&["check"], &rule, root), 1),
        concat!(
            "ignored/forced.ts:1:1: [source/no-marker] Remove the marker.\n",
            "src/staged-delete.ts:1:1: [source/no-marker] Remove the marker.\n",
            "src/unstaged-delete.ts:1:1: [source/no-marker] Remove the marker.\n"
        )
    );

    // Deleted files are skipped, whether or not the deletion is staged.
    git(root, &["rm", "-qf", "src/staged-delete.ts"]);
    fs::remove_file(root.join("src/unstaged-delete.ts")).unwrap();
    fs::remove_file(root.join("banned.txt")).unwrap();
    assert_eq!(
        run(cli(&["check"], &rule, root), 1),
        "ignored/forced.ts:1:1: [source/no-marker] Remove the marker.\n"
    );
    assert_eq!(run(cli(&["check"], &banned, root), 0), "");

    // A text scope whose only tracked file was deleted cannot be judged.
    let only = policy(&[marker_rule(&["src/unstaged-delete.ts"], None)]);
    let error = run(cli(&["check"], &only, root), 2);
    assert!(error.contains("empty text scope"), "{error}");
}

#[test]
fn commit_mode_reads_only_the_commit_tree() {
    let repo = repo();
    let root = repo.path();
    write(root, "src/app.ts", "currentMarker();\n");
    write(root, "src/gen/app.ts", "legacyMarker();\n");
    git(root, &["add", "-A"]);
    let sha = commit(root);
    let rule = policy(&[marker_rule(&["src/**/*.ts"], Some(&["src/gen/**"]))]);

    write(root, "src/app.ts", "legacyMarker();\n");
    write(root, "src/staged.ts", "legacyMarker();\n");
    git(root, &["add", "src/staged.ts"]);
    assert_eq!(run(cli(&["check", "--commit", &sha], &rule, root), 0), "");
    assert_eq!(
        run(cli(&["check"], &rule, root), 1),
        concat!(
            "src/app.ts:1:1: [source/no-marker] Remove the marker.\n",
            "src/staged.ts:1:1: [source/no-marker] Remove the marker.\n"
        )
    );
}

#[test]
fn rule_sources_are_candidates_unless_excluded() {
    let repo = repo();
    let root = repo.path();
    let source = include_str!("../.rulecheck/rules/no-legacy-mode.rule.ts")
        .replace("[\"config/*.toml\"]", "[\"config/*.toml\", \"**/*.ts\"]");
    assert!(source.contains("\"**/*.ts\""));
    write(root, ".rulecheck/rules/demo.rule.ts", &source);
    write(root, "src/app.ts", "export {};\n");
    git(root, &["add", "-A"]);
    let built = Command::new("node")
        .arg(Path::new(ROOT).join("tools/build.mjs"))
        .arg("--root")
        .arg(root)
        .output()
        .expect("run build; install Node.js and run npm ci");
    let built = run(built, 0);

    // The rule's own fixture content is tracked text in scope.
    assert_eq!(
        run(cli(&["test"], built.as_bytes(), root), 0),
        "2 fixtures passed\n"
    );
    assert_eq!(
        run(cli(&["check"], built.as_bytes(), root), 1),
        ".rulecheck/rules/demo.rule.ts:3:24: [config/no-legacy-mode] Remove legacy_mode or set it to false.\n"
    );

    let mut excluded: Value = serde_json::from_str(&built).unwrap();
    excluded["rules"][0]["exclude"] = json!([".rulecheck/**"]);
    let excluded = excluded.to_string();
    assert_eq!(run(cli(&["check"], excluded.as_bytes(), root), 0), "");
}

#[test]
fn each_rule_reports_its_first_match_per_file() {
    let repo = repo();
    let root = repo.path();
    write(
        root,
        "src/app.ts",
        "ok();\nlegacyMarker(); legacyMarker();\nlegacyMarker();\n",
    );
    git(root, &["add", "-A"]);
    let mut second = marker_rule(&["src/*.ts"], None);
    second["id"] = json!("source/no-marker-regex");
    second["pattern"] = json!({ "kind": "regex", "value": "Marker" });
    second["fixtures"] = json!([{ "path": "src/app.ts", "content": "legacyMarker", "expect": { "line": 1, "column": 7 } }]);
    let rules = policy(&[marker_rule(&["src/*.ts"], None), second]);
    assert_eq!(
        run(cli(&["check"], &rules, root), 1),
        concat!(
            "src/app.ts:2:1: [source/no-marker] Remove the marker.\n",
            "src/app.ts:2:7: [source/no-marker-regex] Remove the marker.\n"
        )
    );
}

#[test]
fn scoped_non_regular_working_tree_files_cannot_be_judged() {
    let repo = repo();
    let root = repo.path();
    write(root, "src/app.ts", "currentMarker();\n");
    write(root, "src/other.ts", "currentMarker();\n");
    write(root, "elsewhere/app.ts", "currentMarker();\n");
    git(root, &["add", "-A"]);
    let rule = policy(&[marker_rule(&["src/**/*.ts"], None)]);
    let paths = policy(&[json!({
        "id": "path/no-app", "kind": "forbidden-path", "paths": ["src/app.ts"],
        "remediation": "Remove src/app.ts.",
        "fixtures": [{ "path": "src/app.ts", "expect": { "line": 1, "column": 1 } }]
    })]);

    // A directory or FIFO in place of a tracked file is rejected without blocking.
    fs::remove_file(root.join("src/app.ts")).unwrap();
    fs::create_dir(root.join("src/app.ts")).unwrap();
    for policy in [&rule, &paths] {
        let error = run(cli(&["check"], policy, root), 2);
        assert!(
            error.contains("src/app.ts: expected a regular file"),
            "{error}"
        );
    }
    fs::remove_dir(root.join("src/app.ts")).unwrap();
    let status = Command::new("mkfifo")
        .arg(root.join("src/app.ts"))
        .status()
        .unwrap();
    assert!(status.success());
    let error = run(cli(&["check"], &rule, root), 2);
    assert!(
        error.contains("src/app.ts: expected a regular file"),
        "{error}"
    );

    // A symlinked parent directory is not followed.
    fs::remove_file(root.join("src/app.ts")).unwrap();
    fs::rename(root.join("src"), root.join("real-src")).unwrap();
    symlink("elsewhere", root.join("src")).unwrap();
    let error = run(cli(&["check"], &rule, root), 2);
    assert!(error.contains("without following symlinks"), "{error}");
}

#[test]
fn schema_sdk_and_evaluator_accept_the_same_rule_fields() {
    let schema: Value = serde_json::from_str(include_str!("../schema/policy-v1.json")).unwrap();
    let rule = &schema["$defs"]["rule"];
    let mut fields: Vec<_> = rule["properties"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    fields.sort();
    assert_eq!(
        fields,
        [
            "exclude",
            "fixtures",
            "id",
            "kind",
            "paths",
            "pattern",
            "remediation"
        ]
    );
    assert_eq!(
        rule["required"],
        json!(["id", "kind", "paths", "remediation", "fixtures"])
    );
    let fixture = &schema["$defs"]["fixture"]["properties"];
    let mut fixture: Vec<_> = fixture.as_object().unwrap().keys().cloned().collect();
    fixture.sort();
    assert_eq!(fixture, ["content", "expect", "path"]);

    let sdk = include_str!("../sdk/index.d.ts");
    for field in [
        "id: string;",
        "paths: readonly string[];",
        "exclude?: readonly string[];",
        "remediation: string;",
        "kind: \"text\";",
        "kind: \"forbidden-path\";",
        "pattern: { kind: \"literal\" | \"regex\"; value: string };",
        "path: string; content: string; expect: Location | null",
        "path: string; expect: Location | null",
    ] {
        assert!(sdk.contains(field), "SDK is missing {field}");
    }

    // Fields outside the schema, such as provenance metadata, are rejected.
    let root = Path::new(ROOT);
    for (key, value) in [
        (
            "provenance",
            json!({ "finding": "https://example.invalid/1" }),
        ),
        ("include", json!(["src/**"])),
        ("severity", json!("error")),
    ] {
        let mut extra = marker_rule(&["src/**/*.ts"], None);
        extra[key] = value;
        let error = run(cli(&["validate"], &policy(&[extra]), root), 2);
        assert!(error.contains(&format!("unknown field `{key}`")), "{error}");
    }
    let mut fixture_extra = marker_rule(&["src/**/*.ts"], None);
    fixture_extra["fixtures"][0]["note"] = json!("x");
    let error = run(cli(&["validate"], &policy(&[fixture_extra]), root), 2);
    assert!(error.contains("unknown field `note`"), "{error}");
}
