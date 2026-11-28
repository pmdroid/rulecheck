//! Pins named exclusion sets (`excludeSets`, `.rulecheck/sets/*.set.ts`) and the
//! opt-in `skipIfContent` predicate: static resolution from the same worktree or
//! commit as the rules, glob, reference, depth and cycle limits, the order of
//! path and content checks, fixture parity, and cannot-judge behavior when a
//! file cannot be read.
mod common;

use serde_json::{Value, json};
use std::fs;
use std::io::Write;
use std::os::unix::fs::symlink;
use std::path::Path;
use std::process::{Command, Output, Stdio};

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

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
            "Rulecheck exclusion-set fixture",
        ],
    );
    git(root, &["rev-parse", "HEAD"])
}

fn write(root: &Path, path: &str, content: impl AsRef<[u8]>) {
    let path = root.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

fn repo() -> tempfile::TempDir {
    let repo = tempfile::tempdir().unwrap();
    git(repo.path(), &["init", "-q"]);
    repo
}

fn build(root: &Path, args: &[&str]) -> Output {
    Command::new("node")
        .arg(Path::new(ROOT).join("tools/build.mjs"))
        .arg("--root")
        .arg(root)
        .args(args)
        .output()
        .expect("run build; install Node.js and run npm ci")
}

const GENERATED_SET: &str = r#"import { defineGlobSet } from "@pmdroid/rulecheck";

export default defineGlobSet({ version: 1, globs: ["src/gen/**"], include: ["third-party"] });
"#;

const THIRD_PARTY_SET: &str = r#"import { defineGlobSet } from "@pmdroid/rulecheck";

export default defineGlobSet({ version: 1, globs: ["vendor/**"] });
"#;

const RULE: &str = r#"import { defineRule } from "@pmdroid/rulecheck";

export default defineRule({
  id: "demo/no-todo",
  kind: "text",
  paths: ["**/*.ts"],
  exclude: [".rulecheck/**"],
  excludeSets: ["generated"],
  skipIfContent: { kind: "literal", value: "@generated", firstBytes: 64 },
  pattern: { kind: "literal", value: "TODO" },
  remediation: "Resolve the TODO.",
  fixtures: [
    { path: "src/a.ts", content: "// TODO\n", expect: { line: 1, column: 4 } },
    { path: "src/gen/a.ts", content: "// TODO\n", expect: null },
    { path: "vendor/a.ts", content: "// TODO\n", expect: null },
    { path: "src/b.ts", content: "// @generated\n// TODO\n", expect: null },
  ],
});
"#;

/// A synthetic checkout with one rule, two chained sets and a file for each
/// way out of scope.
fn sample() -> tempfile::TempDir {
    let repo = repo();
    let root = repo.path();
    write(root, ".rulecheck/sets/generated.set.ts", GENERATED_SET);
    write(root, ".rulecheck/sets/third-party.set.ts", THIRD_PARTY_SET);
    write(root, ".rulecheck/rules/todo.rule.ts", RULE);
    write(root, "src/a.ts", "// TODO a\n");
    write(root, "src/gen/x.ts", "// TODO\n");
    write(root, "vendor/v.ts", "// TODO\n");
    write(root, "src/b.ts", "/* @generated */\n// TODO\n");
    git(root, &["add", "-A"]);
    repo
}

#[test]
fn builder_resolves_sets_and_scan_and_fixtures_agree() {
    let repo = sample();
    let root = repo.path();
    let (policy, _) = run(build(root, &[]), 0);
    let document: Value = serde_json::from_str(&policy).unwrap();
    assert_eq!(
        document["sets"],
        json!([
            { "name": "generated", "version": 1, "globs": ["src/gen/**"], "include": ["third-party"] },
            { "name": "third-party", "version": 1, "globs": ["vendor/**"] },
        ])
    );
    let schema: Value = serde_json::from_str(include_str!("../schema/policy-v1.json")).unwrap();
    assert_eq!(common::violations(&schema, &document), Vec::<String>::new());

    let (stdout, _) = run(cli(&["test"], policy.as_bytes(), root), 0);
    assert_eq!(stdout, "4 fixtures passed\n");
    let (stdout, _) = run(cli(&["check"], policy.as_bytes(), root), 1);
    assert_eq!(stdout, "src/a.ts:1:4: [demo/no-todo] Resolve the TODO.\n");

    // A fixture that disagrees with the skip fails like any other fixture.
    let wrong = policy.replace(
        r#""content":"// @generated\n// TODO\n","expect":null"#,
        r#""content":"// @generated\n// TODO\n","expect":{"line":2,"column":4}"#,
    );
    assert_ne!(wrong, policy);
    let (stdout, _) = run(cli(&["test"], wrong.as_bytes(), root), 1);
    assert!(
        stdout.starts_with("src/b.ts:2:4: [demo/no-todo] fixture 4 expected a violation at 2:4, found no violation."),
        "{stdout}"
    );
}

#[test]
fn policies_without_sets_are_unchanged() {
    let repo = repo();
    let root = repo.path();
    write(
        root,
        ".rulecheck/rules/todo.rule.ts",
        RULE.replace("  excludeSets: [\"generated\"],\n", "")
            .replace(
                "  skipIfContent: { kind: \"literal\", value: \"@generated\", firstBytes: 64 },\n",
                "",
            ),
    );
    let (policy, _) = run(build(root, &[]), 0);
    assert!(policy.starts_with(r#"{"version":1,"rules":["#), "{policy}");
}

#[test]
fn commit_builds_read_sets_from_that_commit() {
    let repo = sample();
    let root = repo.path();
    let sha = commit(root);
    // The worktree drops vendor/** from the included set; the commit keeps it.
    write(
        root,
        ".rulecheck/sets/third-party.set.ts",
        THIRD_PARTY_SET.replace("vendor/**", "third_party/**"),
    );
    let (worktree, _) = run(build(root, &[]), 0);
    let (committed, _) = run(build(root, &["--commit", &sha]), 0);
    assert!(
        committed.contains(r#""globs":["vendor/**"]"#),
        "{committed}"
    );
    assert!(
        worktree.contains(r#""globs":["third_party/**"]"#),
        "{worktree}"
    );

    let (stdout, _) = run(
        cli(&["check", "--commit", &sha], committed.as_bytes(), root),
        1,
    );
    assert_eq!(stdout, "src/a.ts:1:4: [demo/no-todo] Resolve the TODO.\n");
    let (stdout, _) = run(cli(&["check"], worktree.as_bytes(), root), 1);
    assert_eq!(
        stdout,
        "src/a.ts:1:4: [demo/no-todo] Resolve the TODO.\n\
         vendor/v.ts:1:4: [demo/no-todo] Resolve the TODO.\n"
    );

    // A set added only to the worktree is unknown to the commit's rules.
    write(
        root,
        ".rulecheck/rules/todo.rule.ts",
        RULE.replace("\"generated\"]", "\"generated\", \"local\"]"),
    );
    let partial = commit(root);
    write(root, ".rulecheck/sets/local.set.ts", THIRD_PARTY_SET);
    run(build(root, &[]), 0);
    let (_, stderr) = run(build(root, &["--commit", &partial]), 2);
    assert!(
        stderr.contains("todo.rule.ts:8:30: Unknown set \"local\" in excludeSets."),
        "{stderr}"
    );
}

#[test]
fn builder_reports_set_mistakes_in_the_source_file() {
    for (path, content, message) in [
        (
            ".rulecheck/rules/todo.rule.ts",
            RULE.replace("[\"generated\"]", "[\"generatd\"]"),
            ".rulecheck/rules/todo.rule.ts:8:17: Unknown set \"generatd\" in excludeSets. \
             Add .rulecheck/sets/generatd.set.ts or use one of: generated, third-party.",
        ),
        (
            ".rulecheck/rules/todo.rule.ts",
            RULE.replace("firstBytes: 64", "firstBytes: 65537"),
            ".rulecheck/rules/todo.rule.ts:9:58: skipIfContent firstBytes must be an integer from 1 to 65536.",
        ),
        (
            ".rulecheck/sets/generated.set.ts",
            GENERATED_SET.replace("version: 1", "version: 2"),
            ".rulecheck/sets/generated.set.ts:3:32: set version must be 1. Change version to 1.",
        ),
        (
            ".rulecheck/sets/generated.set.ts",
            GENERATED_SET.replace("include:", "includes:"),
            ".rulecheck/sets/generated.set.ts:3:67: Unknown property includes in the set. Use version, globs and include.",
        ),
        (
            ".rulecheck/sets/generated.set.ts",
            GENERATED_SET.replace("[\"src/gen/**\"]", "[\"src/gen/**\"].concat([])"),
            ".rulecheck/sets/generated.set.ts:3:51: Unsupported value.",
        ),
        (
            ".rulecheck/sets/generated.set.ts",
            GENERATED_SET.replace("defineGlobSet }", "defineRule }"),
            ".rulecheck/sets/generated.set.ts:1:1: Expected the SDK marker import. \
             Start with import { defineGlobSet } from \"@pmdroid/rulecheck\".",
        ),
        (
            ".rulecheck/sets/Generated.set.ts",
            GENERATED_SET.to_owned(),
            ".rulecheck/sets/Generated.set.ts:1:1: Invalid set name \"Generated\".",
        ),
    ] {
        let repo = sample();
        let root = repo.path();
        write(root, path, content);
        let (_, stderr) = run(build(root, &[]), 2);
        assert!(stderr.contains(message), "want {message}\ngot {stderr}");
    }

    // A forbidden-path rule never reads content, so it cannot skip on it.
    let repo = sample();
    let root = repo.path();
    write(
        root,
        ".rulecheck/rules/todo.rule.ts",
        r#"import { defineRule } from "@pmdroid/rulecheck";

export default defineRule({
  id: "demo/no-env",
  kind: "forbidden-path",
  paths: ["**/.env"],
  skipIfContent: { kind: "literal", value: "x", firstBytes: 1 },
  remediation: "Remove the file.",
  fixtures: [{ path: ".env", expect: { line: 1, column: 1 } }],
});
"#,
    );
    let (_, stderr) = run(build(root, &[]), 2);
    assert!(
        stderr.contains("todo.rule.ts:7:3: Forbidden-path rules have no skipIfContent"),
        "{stderr}"
    );
}

#[test]
fn a_symlinked_sets_directory_is_rejected() {
    let repo = sample();
    let root = repo.path();
    fs::rename(root.join(".rulecheck/sets"), root.join("elsewhere")).unwrap();
    symlink("../elsewhere", root.join(".rulecheck/sets")).unwrap();
    let (_, stderr) = run(build(root, &[]), 2);
    assert!(
        stderr.contains(".rulecheck/sets: use a regular directory inside the checkout"),
        "{stderr}"
    );
}

fn rule(extra: Value) -> Value {
    let mut rule = json!({
        "id": "demo/no-todo",
        "kind": "text",
        "paths": ["**/*.ts"],
        "pattern": { "kind": "literal", "value": "TODO" },
        "remediation": "Resolve the TODO.",
        "fixtures": [{ "path": "a.ts", "content": "TODO", "expect": { "line": 1, "column": 1 } }],
    });
    rule.as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    rule
}

fn set(name: &str, globs: Value, include: Option<Value>) -> Value {
    let mut set = json!({ "name": name, "version": 1, "globs": globs });
    if let Some(include) = include {
        set["include"] = include;
    }
    set
}

fn rejects(policy: Value, message: &str) {
    let dir = tempfile::tempdir().unwrap();
    let (_, stderr) = run(cli(&["test"], policy.to_string().as_bytes(), dir.path()), 2);
    assert!(stderr.contains(message), "want {message}\ngot {stderr}");
}

fn accepts(policy: Value) {
    let dir = tempfile::tempdir().unwrap();
    run(cli(&["test"], policy.to_string().as_bytes(), dir.path()), 0);
}

fn globs(prefix: &str, count: usize) -> Value {
    json!(
        (0..count)
            .map(|i| format!("{prefix}{i}/**"))
            .collect::<Vec<_>>()
    )
}

#[test]
fn evaluator_bounds_globs_references_depth_and_cycles() {
    // 64 inline globs per list replace the old cap of 16.
    let mut paths = globs("p", 63);
    paths.as_array_mut().unwrap().push(json!("*.ts"));
    accepts(
        json!({ "version": 1, "rules": [rule(json!({ "paths": paths, "exclude": globs("x", 64) }))] }),
    );
    rejects(
        json!({ "version": 1, "rules": [rule(json!({ "paths": globs("p", 65) }))] }),
        "[demo/no-todo] provide 1 to 64 path globs",
    );
    rejects(
        json!({ "version": 1, "rules": [rule(json!({ "exclude": globs("x", 65) }))] }),
        "provide 1 to 64 exclude globs or omit exclude",
    );

    // A set holds up to 256 globs; a rule resolves up to 256 distinct ones.
    let big = |name: &str, count| set(name, globs(name, count), None);
    accepts(
        json!({ "version": 1, "sets": [big("a", 256)], "rules": [rule(json!({ "excludeSets": ["a"] }))] }),
    );
    rejects(
        json!({ "version": 1, "sets": [big("a", 257)], "rules": [rule(json!({}))] }),
        "set \"a\": list at most 256 globs",
    );
    rejects(
        json!({ "version": 1, "sets": [big("a", 200), big("b", 57)], "rules": [rule(json!({ "excludeSets": ["a", "b"], "exclude": ["z/**"] }))] }),
        "[demo/no-todo] excludes 258 distinct globs after resolving excludeSets; use at most 256",
    );
    // Shared globs and diamond includes count once.
    accepts(json!({
        "version": 1,
        "sets": [
            set("a", globs("s", 200), None),
            set("b", globs("s", 200), Some(json!(["a"]))),
            set("c", json!([]), Some(json!(["a", "b"]))),
        ],
        "rules": [rule(json!({ "excludeSets": ["c", "a"], "exclude": ["s0/**"] }))],
    }));

    // Across the policy, scope and resolved exclusion globs add up to 4096.
    let rules: Vec<Value> = (0..17)
        .map(|i| {
            let mut rule = rule(json!({ "excludeSets": ["a"] }));
            rule["id"] = json!(format!("demo/rule-{i}"));
            rule
        })
        .collect();
    rejects(
        json!({ "version": 1, "sets": [big("a", 240)], "rules": rules }),
        "[demo/rule-16] the policy uses more than 4096 path and exclusion globs across all rules",
    );

    // References: known, distinct, at most 16, no cycles, at most 4 levels.
    rejects(
        json!({ "version": 1, "rules": [rule(json!({ "excludeSets": ["missing"] }))] }),
        "[demo/no-todo] unknown set \"missing\" in excludeSets",
    );
    rejects(
        json!({ "version": 1, "sets": [big("a", 1)], "rules": [rule(json!({ "excludeSets": ["a", "a"] }))] }),
        "excludeSets lists set \"a\" twice",
    );
    rejects(
        json!({ "version": 1, "sets": [set("a", json!([]), Some(json!(["b"])))], "rules": [rule(json!({}))] }),
        "set \"a\" includes unknown set \"b\"",
    );
    rejects(
        json!({
            "version": 1,
            "sets": [
                set("a", globs("a", 1), Some(json!(["b"]))),
                set("b", globs("b", 1), Some(json!(["c"]))),
                set("c", globs("c", 1), Some(json!(["a"]))),
            ],
            "rules": [rule(json!({}))],
        }),
        "set include cycle \"a\" -> \"b\" -> \"c\" -> \"a\"",
    );
    let chain = |depth: usize| -> Vec<Value> {
        (0..depth)
            .map(|i| {
                let include = (i + 1 < depth).then(|| json!([format!("s{}", i + 1)]));
                set(&format!("s{i}"), globs("g", 1), include)
            })
            .collect()
    };
    accepts(
        json!({ "version": 1, "sets": chain(4), "rules": [rule(json!({ "excludeSets": ["s0"] }))] }),
    );
    rejects(
        json!({ "version": 1, "sets": chain(5), "rules": [rule(json!({}))] }),
        "set \"s0\" nests includes deeper than 4 levels",
    );

    // Set shape.
    rejects(
        json!({ "version": 1, "sets": [{ "name": "a", "version": 2, "globs": ["a/**"] }], "rules": [rule(json!({}))] }),
        "set \"a\": unsupported set version 2; use version 1",
    );
    rejects(
        json!({ "version": 1, "sets": [big("a", 1), big("a", 1)], "rules": [rule(json!({}))] }),
        "set \"a\": duplicate set name",
    );
    rejects(
        json!({ "version": 1, "sets": [set("a", json!([]), None)], "rules": [rule(json!({}))] }),
        "set \"a\": list globs or include another set",
    );
    rejects(
        json!({ "version": 1, "sets": [set("a", json!(["../x"]), None)], "rules": [rule(json!({}))] }),
        "set \"a\": invalid exclude glob \"../x\"",
    );
    let many: Vec<Value> = (0..65).map(|i| big(&format!("s{i}"), 1)).collect();
    rejects(
        json!({ "version": 1, "sets": many, "rules": [rule(json!({}))] }),
        "policy must have at most 64 sets",
    );

    // skipIfContent bounds.
    for (skip, message) in [
        (
            json!({ "kind": "literal", "value": "x", "firstBytes": 0 }),
            "skipIfContent.firstBytes must be 1 to 65536",
        ),
        (
            json!({ "kind": "literal", "value": "x", "firstBytes": 65537 }),
            "skipIfContent.firstBytes must be 1 to 65536",
        ),
        (
            json!({ "kind": "literal", "value": "", "firstBytes": 8 }),
            "use a skipIfContent value of 1 to 4096 bytes",
        ),
        (
            json!({ "kind": "regex", "value": "(", "firstBytes": 8 }),
            "invalid skipIfContent regex",
        ),
    ] {
        rejects(
            json!({ "version": 1, "rules": [rule(json!({ "skipIfContent": skip }))] }),
            message,
        );
    }
    rejects(
        json!({ "version": 1, "rules": [{
            "id": "demo/no-env", "kind": "forbidden-path", "paths": ["**/.env"],
            "skipIfContent": { "kind": "literal", "value": "x", "firstBytes": 1 },
            "remediation": "Remove it.", "fixtures": [{ "path": ".env", "expect": { "line": 1, "column": 1 } }],
        }] }),
        "skipIfContent needs a text, syntax or structured rule",
    );
}

fn skip_policy(skip: Value) -> String {
    json!({ "version": 1, "rules": [rule(json!({ "skipIfContent": skip }))] }).to_string()
}

#[test]
fn content_skips_read_only_a_bounded_prefix() {
    let repo = repo();
    let root = repo.path();
    // The marker ends at byte 12 in in.ts, straddles byte 8 in edge.ts, and
    // starts after byte 8 in late.ts.
    write(root, "in.ts", "// @gen TODO\n");
    write(root, "edge.ts", "//   @gen TODO\n");
    write(root, "late.ts", "//      @gen TODO\n");
    git(root, &["add", "-A"]);
    let policy = skip_policy(json!({ "kind": "literal", "value": "@gen", "firstBytes": 8 }));
    let (stdout, _) = run(cli(&["check"], policy.as_bytes(), root), 1);
    assert_eq!(
        stdout,
        "edge.ts:1:11: [demo/no-todo] Resolve the TODO.\n\
         late.ts:1:14: [demo/no-todo] Resolve the TODO.\n"
    );
    let policy = skip_policy(json!({ "kind": "regex", "value": "^// +@gen", "firstBytes": 11 }));
    let (stdout, _) = run(cli(&["check"], policy.as_bytes(), root), 1);
    assert_eq!(stdout, "late.ts:1:14: [demo/no-todo] Resolve the TODO.\n");

    // A prefix may end inside a multi-byte character.
    write(root, "in.ts", "é@gen TODO\n");
    let policy = skip_policy(json!({ "kind": "literal", "value": "@gen", "firstBytes": 1 }));
    run(cli(&["check"], policy.as_bytes(), root), 1);
}

#[test]
fn content_skips_never_hide_unreadable_files() {
    let repo = repo();
    let root = repo.path();
    write(root, "gen.ts", "// @generated\n// TODO\n");
    git(root, &["add", "-A"]);
    let policy = skip_policy(json!({ "kind": "literal", "value": "@generated", "firstBytes": 64 }));

    // Every in-scope file skipped: the rule judged nothing, so it cannot pass.
    let (_, stderr) = run(cli(&["check"], policy.as_bytes(), root), 2);
    assert!(
        stderr.contains("[demo/no-todo] empty text scope"),
        "{stderr}"
    );

    // Invalid UTF-8 after the marker still cannot be judged.
    write(root, "bad.ts", b"// @generated\n\xff\n".as_slice());
    git(root, &["add", "-A"]);
    let (_, stderr) = run(cli(&["check"], policy.as_bytes(), root), 2);
    assert!(
        stderr.contains("[demo/no-todo] bad.ts: expected UTF-8 text"),
        "{stderr}"
    );

    // Neither can a file over the 1 MiB read limit, or a symlink in its place.
    let mut big = b"// @generated\n".to_vec();
    big.resize(1024 * 1024 + 1, b' ');
    write(root, "bad.ts", big);
    let (_, stderr) = run(cli(&["check"], policy.as_bytes(), root), 2);
    assert!(
        stderr.contains("[demo/no-todo] bad.ts: cannot read text"),
        "{stderr}"
    );
    fs::remove_file(root.join("bad.ts")).unwrap();
    symlink("gen.ts", root.join("bad.ts")).unwrap();
    let (_, stderr) = run(cli(&["check"], policy.as_bytes(), root), 2);
    assert!(
        stderr.contains("[demo/no-todo] bad.ts: cannot open"),
        "{stderr}"
    );
}

#[test]
fn path_exclusions_win_before_content_is_read() {
    let repo = repo();
    let root = repo.path();
    write(root, "src/a.ts", "// TODO\n");
    write(root, "vendor/bad.ts", b"\xff".as_slice());
    git(root, &["add", "-A"]);
    // The excluded file is never opened, so its bytes cannot fail the scan.
    let policy = json!({
        "version": 1,
        "sets": [set("vendor", json!(["vendor/**"]), None)],
        "rules": [rule(json!({
            "excludeSets": ["vendor"],
            "skipIfContent": { "kind": "literal", "value": "@generated", "firstBytes": 64 },
        }))],
    })
    .to_string();
    let (stdout, _) = run(cli(&["check"], policy.as_bytes(), root), 1);
    assert_eq!(stdout, "src/a.ts:1:4: [demo/no-todo] Resolve the TODO.\n");
}

#[test]
fn syntax_rules_skip_before_parsing() {
    let repo = repo();
    let root = repo.path();
    // Generated code that does not parse is skipped instead of failing.
    write(root, "gen.ts", "// @generated\nconst = ;\n");
    write(root, "src.ts", "// keep\nconst a = 1;\n");
    git(root, &["add", "-A"]);
    let policy = json!({ "version": 1, "rules": [{
        "id": "demo/no-comments",
        "kind": "syntax",
        "paths": ["*.ts"],
        "selector": { "language": "typescript", "node": "comment" },
        "skipIfContent": { "kind": "literal", "value": "@generated", "firstBytes": 32 },
        "remediation": "Remove the comment.",
        "fixtures": [
            { "path": "a.ts", "content": "// @generated\nconst = ;\n", "expect": null },
            { "path": "b.ts", "content": "// c\n", "expect": { "line": 1, "column": 1 } },
        ],
    }] })
    .to_string();
    let (stdout, _) = run(cli(&["test"], policy.as_bytes(), root), 0);
    assert_eq!(stdout, "2 fixtures passed\n");
    let (stdout, _) = run(cli(&["check"], policy.as_bytes(), root), 1);
    assert_eq!(
        stdout,
        "src.ts:1:1: [demo/no-comments] Remove the comment.\n"
    );
}

#[test]
fn exact_commit_evidence_keeps_sets_and_diffs_their_effect() {
    let repo = sample();
    let root = repo.path();
    write(root, "src/a.ts", "// done\n");
    let base = commit(root);
    // Only a set changes: vendor/ comes back into scope, with a TODO in it.
    write(
        root,
        ".rulecheck/sets/third-party.set.ts",
        THIRD_PARTY_SET.replace("vendor/**", "third_party/**"),
    );
    let candidate = commit(root);
    let reviews = tempfile::NamedTempFile::new().unwrap();
    let output = Command::new("node")
        .current_dir(root)
        .arg(Path::new(ROOT).join("tools/check-ci.mjs"))
        .arg(root)
        .args([&base, &candidate, env!("CARGO_BIN_EXE_rulecheck-evaluator")])
        .env("RULECHECK_TOOL_SHA", &base)
        .env("RULECHECK_POLICY_REVIEWS", reviews.path())
        .output()
        .unwrap();
    let evidence: Value = serde_json::from_slice(&output.stdout).unwrap();
    let phases = &evidence["phases"];
    for phase in ["base-validate", "base-check", "proposed-validate"] {
        assert_eq!(phases[phase]["outcome"], "pass", "{phase}: {evidence}");
    }
    // Fixtures resolve the proposed sets too, so the rule's own vendor fixture fails.
    assert_eq!(
        phases["proposed-test"]["stdout"],
        "vendor/a.ts:1:4: [demo/no-todo] fixture 3 expected no violation, found a violation at 1:4. \
         Resolve the TODO. Update the rule or fixture.\n"
    );
    // The base policy still excludes vendor/; the proposed one reports it.
    assert_eq!(
        phases["proposed-check"]["outcome"], "violation",
        "{evidence}"
    );
    // A set edit needs the policy owner's review like a rule edit.
    assert_eq!(phases["proposed-owner-review"]["required"], true);
    assert_eq!(phases["proposed-owner-review"]["outcome"], "violation");
    let changes = &evidence["effectivePolicyDiff"]["changes"];
    assert_eq!(changes.as_array().unwrap().len(), 1, "{changes}");
    assert_eq!(changes[0]["id"], "demo/no-todo");
    assert_eq!(
        changes[0]["before"]["resolvedExclude"],
        json!([".rulecheck/**", "src/gen/**", "vendor/**"])
    );
    assert_eq!(
        changes[0]["after"]["resolvedExclude"],
        json!([".rulecheck/**", "src/gen/**", "third_party/**"])
    );
}

#[test]
fn skipped_files_still_count_toward_the_text_budget() {
    let repo = repo();
    let root = repo.path();
    let generated = format!("// @generated\n{}\n", " ".repeat(700_000));
    write(root, "a.ts", &generated);
    write(root, "b.ts", &generated);
    git(root, &["add", "-A"]);
    let policy = skip_policy(json!({ "kind": "literal", "value": "@generated", "firstBytes": 64 }));
    // The second file would pass the budget before its header is checked, so
    // the rule is reported as not evaluated rather than silently skipped.
    let (_, stderr) = run(
        cli(&["check", "--text-budget", "1"], policy.as_bytes(), root),
        2,
    );
    assert!(
        stderr.contains("[demo/no-todo] not evaluated on every file in scope"),
        "{stderr}"
    );
    assert!(stderr.contains("scan stopped at b.ts"), "{stderr}");
}
