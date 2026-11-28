//! Pins opt-in all-match reporting (`check --all-matches`) and `expectAll`
//! fixtures: literal and regex match semantics, UTF-8 byte spans, output order,
//! the findings cap, working-tree and commit agreement, the syntax-rule refusal,
//! and the same flag and fields through the builder and the installed wrapper.
//! Without the flag every rule still reports its first match per file.
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
            "Rulecheck all-match fixture",
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

fn schema(name: &str) -> Value {
    let text = fs::read_to_string(Path::new(ROOT).join("schema").join(name)).unwrap();
    serde_json::from_str(&text).unwrap()
}

/// A policy that the published schema accepts.
fn policy(rules: &[Value]) -> Vec<u8> {
    let policy = json!({ "version": 1, "rules": rules });
    let errors = common::violations(&schema("policy-v1.json"), &policy);
    assert!(errors.is_empty(), "{errors:?}");
    serde_json::to_vec(&policy).unwrap()
}

fn text_rule(id: &str, kind: &str, value: &str, paths: &[&str]) -> Value {
    json!({
        "id": id, "kind": "text", "paths": paths,
        "pattern": { "kind": kind, "value": value },
        "remediation": "Remove the marker.",
        "fixtures": [{ "path": paths[0].replace("**/", "").replace('*', "x"), "content": "", "expect": null }]
    })
}

fn json_check(args: &[&str], policy: &[u8], root: &Path, code: i32) -> Value {
    let (stdout, _) = run(cli(args, policy, root), code);
    let result: Value = serde_json::from_str(&stdout).unwrap();
    let errors = common::violations(&schema("result-v1.json"), &result);
    assert!(errors.is_empty(), "{errors:?}");
    result
}

/// `(line, column, byteStart, byteEnd)` for each finding.
fn spans(result: &Value) -> Vec<(u64, u64, Option<u64>, Option<u64>)> {
    result["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|finding| {
            (
                finding["line"].as_u64().unwrap(),
                finding["column"].as_u64().unwrap(),
                finding["byteStart"].as_u64(),
                finding["byteEnd"].as_u64(),
            )
        })
        .collect()
}

/// Every finding of one text rule over one file, with and without the flag.
fn scan(kind: &str, value: &str, content: &str) -> (Value, Value) {
    let repo = repo();
    let root = repo.path();
    write(root, "notes/a.txt", content);
    git(root, &["add", "-A"]);
    let rules = policy(&[text_rule("notes/marker", kind, value, &["notes/*.txt"])]);
    let first = json_check(&["check", "--format", "json"], &rules, root, 1);
    let all = json_check(
        &["check", "--all-matches", "--format", "json"],
        &rules,
        root,
        1,
    );
    (first, all)
}

#[test]
fn default_reports_the_first_match_and_all_matches_reports_every_one() {
    let repo = repo();
    let root = repo.path();
    write(root, "notes/a.txt", "TODO one TODO\nfine\n  TODO\n");
    git(root, &["add", "-A"]);
    let rules = policy(&[text_rule(
        "notes/no-todo",
        "literal",
        "TODO",
        &["notes/*.txt"],
    )]);

    let (first, _) = run(cli(&["check"], &rules, root), 1);
    assert_eq!(
        first,
        "notes/a.txt:1:1: [notes/no-todo] Remove the marker.\n"
    );
    let (all, stderr) = run(cli(&["check", "--all-matches"], &rules, root), 1);
    assert_eq!(
        all,
        "notes/a.txt:1:1: [notes/no-todo] Remove the marker.\n\
         notes/a.txt:1:10: [notes/no-todo] Remove the marker.\n\
         notes/a.txt:3:3: [notes/no-todo] Remove the marker.\n"
    );
    assert!(stderr.is_empty());

    // The default JSON result keeps its v1 shape; the flag adds source.matches.
    let first = json_check(&["check", "--format", "json"], &rules, root, 1);
    assert_eq!(
        first["source"],
        json!({ "mode": "working-tree", "commit": null })
    );
    assert_eq!(spans(&first), [(1, 1, Some(0), Some(4))]);
    let all = json_check(
        &["check", "--format", "json", "--all-matches"],
        &rules,
        root,
        1,
    );
    assert_eq!(
        all["source"],
        json!({ "mode": "working-tree", "commit": null, "matches": "all" })
    );
    assert_eq!(
        spans(&all),
        [
            (1, 1, Some(0), Some(4)),
            (1, 10, Some(9), Some(13)),
            (3, 3, Some(21), Some(25))
        ]
    );
    // A clean file is clean in both modes.
    write(root, "notes/a.txt", "done\n");
    run(cli(&["check", "--all-matches"], &rules, root), 0);
}

#[test]
fn spans_are_utf8_bytes_and_columns_are_unicode_scalars() {
    let content = "π TODO é\n🦀🦀TODO TODO\n";
    let (first, all) = scan("literal", "TODO", content);
    assert_eq!(spans(&first), [(1, 1 + 2, Some(3), Some(7))]);
    let found = spans(&all);
    assert_eq!(
        found,
        [
            (1, 3, Some(3), Some(7)),
            (2, 3, Some(19), Some(23)),
            (2, 8, Some(24), Some(28))
        ]
    );
    for (_, _, start, end) in found {
        assert_eq!(
            &content[start.unwrap() as usize..end.unwrap() as usize],
            "TODO"
        );
    }
}

#[test]
fn matches_do_not_overlap() {
    // A literal search resumes at the end of each match.
    let (first, all) = scan("literal", "aa", "aaaaa\n");
    assert_eq!(spans(&first), [(1, 1, Some(0), Some(2))]);
    assert_eq!(
        spans(&all),
        [(1, 1, Some(0), Some(2)), (1, 3, Some(2), Some(4))]
    );
    let (_, all) = scan("literal", "aba", "ababa aba\n");
    assert_eq!(
        spans(&all),
        [(1, 1, Some(0), Some(3)), (1, 7, Some(6), Some(9))]
    );
    // A regex reports leftmost-first matches, not the longest alternative.
    let (_, all) = scan("regex", "a|ab", "abab\n");
    assert_eq!(
        spans(&all),
        [(1, 1, Some(0), Some(1)), (1, 3, Some(2), Some(3))]
    );
    let (_, all) = scan("regex", "a+", "aaa baa\n");
    assert_eq!(
        spans(&all),
        [(1, 1, Some(0), Some(3)), (1, 6, Some(5), Some(7))]
    );
}

#[test]
fn zero_width_regex_matches_are_reported_at_character_boundaries() {
    // One empty match per line start, including the end of text after the last newline.
    let (first, all) = scan("regex", "(?m)^", "a\nb\n");
    assert_eq!(spans(&first), [(1, 1, Some(0), Some(0))]);
    assert_eq!(
        spans(&all),
        [
            (1, 1, Some(0), Some(0)),
            (2, 1, Some(2), Some(2)),
            (3, 1, Some(4), Some(4))
        ]
    );
    // An empty match that starts where the previous match ended is skipped.
    let (_, all) = scan("regex", "a*", "baaab");
    assert_eq!(
        spans(&all),
        [
            (1, 1, Some(0), Some(0)),
            (1, 2, Some(1), Some(4)),
            (1, 6, Some(5), Some(5))
        ]
    );
    // Empty matches never split a UTF-8 sequence.
    let (_, all) = scan("regex", "x*", "πé");
    assert_eq!(
        spans(&all),
        [
            (1, 1, Some(0), Some(0)),
            (1, 2, Some(2), Some(2)),
            (1, 3, Some(4), Some(4))
        ]
    );
}

#[test]
fn findings_sort_by_path_location_and_rule_and_paths_report_once() {
    let repo = repo();
    let root = repo.path();
    write(root, "notes/b.txt", "xx\n");
    write(root, "notes/a.txt", "x x\n");
    write(root, "notes/local.env", "x x x\n");
    git(root, &["add", "-A"]);
    let mut path_rule = json!({
        "id": "notes/no-env", "kind": "forbidden-path", "paths": ["notes/*.env"],
        "remediation": "Remove the env file.",
        "fixtures": [{ "path": "notes/local.env", "expect": { "line": 1, "column": 1 } }]
    });
    let rules = policy(&[
        text_rule("notes/z-literal", "literal", "x", &["notes/*.txt"]),
        text_rule("notes/a-regex", "regex", "x", &["notes/*"]),
        path_rule.clone(),
    ]);
    let (stdout, _) = run(cli(&["check", "--all-matches"], &rules, root), 1);
    assert_eq!(
        stdout,
        "notes/a.txt:1:1: [notes/a-regex] Remove the marker.\n\
         notes/a.txt:1:1: [notes/z-literal] Remove the marker.\n\
         notes/a.txt:1:3: [notes/a-regex] Remove the marker.\n\
         notes/a.txt:1:3: [notes/z-literal] Remove the marker.\n\
         notes/b.txt:1:1: [notes/a-regex] Remove the marker.\n\
         notes/b.txt:1:1: [notes/z-literal] Remove the marker.\n\
         notes/b.txt:1:2: [notes/a-regex] Remove the marker.\n\
         notes/b.txt:1:2: [notes/z-literal] Remove the marker.\n\
         notes/local.env:1:1: [notes/a-regex] Remove the marker.\n\
         notes/local.env:1:1: [notes/no-env] Remove the env file.\n\
         notes/local.env:1:3: [notes/a-regex] Remove the marker.\n\
         notes/local.env:1:5: [notes/a-regex] Remove the marker.\n"
    );
    let result = json_check(
        &["check", "--all-matches", "--format", "json"],
        &rules,
        root,
        1,
    );
    let env = &result["findings"][9];
    assert_eq!(env["ruleId"], "notes/no-env");
    assert_eq!(
        (env["byteStart"].clone(), env["byteEnd"].clone()),
        (Value::Null, Value::Null)
    );

    // A forbidden-path fixture cannot ask for more than one finding.
    path_rule["fixtures"][0] =
        json!({ "path": "notes/local.env", "expectAll": [{ "line": 1, "column": 1 }] });
    let raw = serde_json::to_vec(&json!({ "version": 1, "rules": [path_rule] })).unwrap();
    let (_, stderr) = run(cli(&["validate"], &raw, root), 2);
    assert_eq!(
        stderr,
        "rulecheck: [notes/no-env] notes/local.env: expectAll is not available because \
         forbidden-path rules report at most one finding per path; use expect\n"
    );
}

#[test]
fn working_tree_and_exact_commit_report_the_same_matches() {
    let repo = repo();
    let root = repo.path();
    write(root, "notes/a.txt", "TODO é TODO\n");
    write(root, "notes/b.txt", "\nTODOTODO\n");
    let sha = commit(root);
    let rules = policy(&[text_rule(
        "notes/no-todo",
        "literal",
        "TODO",
        &["notes/*.txt"],
    )]);
    let tree = json_check(
        &["check", "--all-matches", "--format", "json"],
        &rules,
        root,
        1,
    );
    let exact = json_check(
        &[
            "check",
            "--all-matches",
            "--commit",
            &sha,
            "--format",
            "json",
        ],
        &rules,
        root,
        1,
    );
    assert_eq!(tree["findings"].as_array().unwrap().len(), 4);
    assert_eq!(tree["findings"], exact["findings"]);
    assert_eq!(
        exact["source"],
        json!({ "mode": "commit", "commit": sha, "matches": "all" })
    );
    let (tree, _) = run(cli(&["check", "--all-matches"], &rules, root), 1);
    let (exact, _) = run(
        cli(&["check", "--all-matches", "--commit", &sha], &rules, root),
        1,
    );
    assert_eq!(tree, exact);
    // --untracked adds files to the same all-match scan.
    write(root, "notes/c.txt", "TODO\n");
    let (untracked, _) = run(
        cli(&["check", "--all-matches", "--untracked"], &rules, root),
        1,
    );
    assert_eq!(untracked.lines().count(), 5);
}

#[test]
fn all_matches_are_capped_at_ten_thousand_findings() {
    let repo = repo();
    let root = repo.path();
    let rules = policy(&[text_rule("notes/no-x", "literal", "x", &["notes/*.txt"])]);
    write(root, "notes/a.txt", &"x".repeat(10_000));
    git(root, &["add", "-A"]);
    let (stdout, _) = run(cli(&["check", "--all-matches"], &rules, root), 1);
    assert_eq!(stdout.lines().count(), 10_000);
    assert!(stdout.ends_with("notes/a.txt:1:10000: [notes/no-x] Remove the marker.\n"));

    // One more match, here in a second file, fails closed with no partial findings.
    write(root, "notes/b.txt", "x");
    git(root, &["add", "-A"]);
    let (stdout, stderr) = run(cli(&["check", "--all-matches"], &rules, root), 2);
    assert!(stdout.is_empty());
    assert_eq!(
        stderr,
        "rulecheck: [notes/no-x] notes/b.txt: more than 10,000 findings; narrow the path scope\n"
    );
    let result = json_check(
        &["check", "--all-matches", "--format", "json"],
        &rules,
        root,
        2,
    );
    assert_eq!(result["findings"], json!([]));
    // The first-match default still reports one finding per file.
    let (stdout, _) = run(cli(&["check"], &rules, root), 1);
    assert_eq!(stdout.lines().count(), 2);
}

fn syntax_rule() -> Value {
    json!({
        "id": "source/no-comments", "kind": "syntax", "paths": ["src/**/*.ts"],
        "selector": { "language": "typescript", "node": "comment" },
        "remediation": "Remove the comment.",
        "fixtures": [{ "path": "src/a.ts", "content": "// a\n", "expect": { "line": 1, "column": 1 } }]
    })
}

#[test]
fn syntax_rules_are_not_counted() {
    let repo = repo();
    let root = repo.path();
    write(root, "src/a.ts", "// one\n// two\n");
    write(root, "notes/a.txt", "TODO TODO\n");
    git(root, &["add", "-A"]);
    let rules = policy(&[
        syntax_rule(),
        text_rule("notes/no-todo", "literal", "TODO", &["notes/*.txt"]),
    ]);
    let refusal = "rulecheck: [source/no-comments] --all-matches does not support syntax rules: \
                   they do not count matching nodes yet, and reporting only their first match \
                   would undercount; select text and forbidden-path rules with --id-prefix or \
                   drop --all-matches\n";
    let (stdout, stderr) = run(cli(&["check", "--all-matches"], &rules, root), 2);
    assert!(stdout.is_empty());
    assert_eq!(stderr, refusal);
    // The refusal holds even when no file is in the syntax rule's scope.
    let (_, stderr) = run(
        cli(
            &["check", "--all-matches"],
            &policy(&[syntax_rule()]),
            Path::new(ROOT),
        ),
        2,
    );
    assert_eq!(stderr, refusal);
    let result = json_check(
        &["check", "--all-matches", "--format", "json"],
        &rules,
        root,
        2,
    );
    assert_eq!(result["findings"], json!([]));
    assert_eq!(
        result["diagnostics"][0]["message"],
        refusal["rulecheck: ".len()..].trim_end()
    );
    assert_eq!(result["source"]["matches"], "all");

    // Selecting only text rules counts them; the default still reports both kinds.
    let (stdout, _) = run(
        cli(
            &["check", "--all-matches", "--id-prefix", "notes/"],
            &rules,
            root,
        ),
        1,
    );
    assert_eq!(stdout.lines().count(), 2);
    let (stdout, _) = run(cli(&["check"], &rules, root), 1);
    assert_eq!(
        stdout,
        "notes/a.txt:1:1: [notes/no-todo] Remove the marker.\n\
         src/a.ts:1:1: [source/no-comments] Remove the comment.\n"
    );

    // A syntax fixture cannot assert a count either.
    let mut counted = syntax_rule();
    counted["fixtures"][0] = json!({ "path": "src/a.ts", "content": "// a\n", "expectAll": [] });
    let raw = serde_json::to_vec(&json!({ "version": 1, "rules": [counted.clone()] })).unwrap();
    let (_, stderr) = run(cli(&["validate"], &raw, root), 2);
    assert!(
        stderr.contains("src/a.ts: expectAll is not available because syntax rules do not count"),
        "{stderr}"
    );
    let errors = common::violations(
        &schema("policy-v1.json"),
        &json!({ "version": 1, "rules": [counted] }),
    );
    assert!(!errors.is_empty());
}

#[test]
fn the_flag_applies_to_check_once() {
    for args in [
        &["check", "--all-matches", "--all-matches"][..],
        &["test", "--all-matches"],
        &["list", "--all-matches"],
        &["validate", "--all-matches"],
    ] {
        let (_, stderr) = run(cli(args, b"", Path::new(ROOT)), 2);
        assert!(
            stderr.contains("unexpected argument \"--all-matches\""),
            "{stderr}"
        );
    }
    let (help, _) = run(cli(&["help"], b"", Path::new(ROOT)), 0);
    assert!(help.contains("[--all-matches]"), "{help}");
}

fn counted_rule(fixtures: Value) -> Value {
    json!({
        "id": "notes/no-todo", "kind": "text", "paths": ["notes/*.txt"],
        "pattern": { "kind": "regex", "value": "TODO|FIXME" },
        "remediation": "Resolve the marker.",
        "fixtures": fixtures
    })
}

#[test]
fn expect_all_fixtures_assert_every_position() {
    let root = Path::new(ROOT);
    let content = "TODO é FIXME\nTODO\n";
    let passing = policy(&[counted_rule(json!([
        { "path": "notes/a.txt", "content": content, "expect": { "line": 1, "column": 1 } },
        { "path": "notes/a.txt", "content": content, "expectAll": [
            { "line": 1, "column": 1 }, { "line": 1, "column": 8 }, { "line": 2, "column": 1 }
        ] },
        { "path": "notes/a.txt", "content": "done\n", "expectAll": [] },
        { "path": "other/a.txt", "content": content, "expectAll": [] }
    ]))]);
    let (stdout, _) = run(cli(&["test"], &passing, root), 0);
    assert_eq!(stdout, "4 fixtures passed\n");

    // A missing, an extra and a moved position each fail at the first difference.
    let failing = policy(&[counted_rule(json!([
        { "path": "notes/a.txt", "content": content, "expectAll": [
            { "line": 1, "column": 1 }, { "line": 2, "column": 1 }
        ] },
        { "path": "notes/a.txt", "content": "TODO\n", "expectAll": [
            { "line": 1, "column": 1 }, { "line": 3, "column": 1 }
        ] },
        { "path": "notes/a.txt", "content": "done\n", "expectAll": [{ "line": 1, "column": 1 }] },
        { "path": "notes/a.txt", "content": "TODO\n", "expectAll": [] }
    ]))]);
    let (stdout, _) = run(cli(&["test"], &failing, root), 1);
    assert_eq!(
        stdout,
        "notes/a.txt:1:8: [notes/no-todo] fixture 1 expected 2 violations at 1:1, 2:1, \
         found 3 violations at 1:1, 1:8, 2:1. Resolve the marker. Update the rule or fixture.\n\
         notes/a.txt:3:1: [notes/no-todo] fixture 2 expected 2 violations at 1:1, 3:1, \
         found 1 violation at 1:1. Resolve the marker. Update the rule or fixture.\n\
         notes/a.txt:1:1: [notes/no-todo] fixture 3 expected 1 violation at 1:1, \
         found no violations. Resolve the marker. Update the rule or fixture.\n\
         notes/a.txt:1:1: [notes/no-todo] fixture 4 expected no violations, \
         found 1 violation at 1:1. Resolve the marker. Update the rule or fixture.\n"
    );
    // Long lists are shortened.
    let many = policy(&[counted_rule(json!([
        { "path": "notes/a.txt", "content": "TODO ".repeat(20), "expectAll": [] }
    ]))]);
    let (stdout, _) = run(cli(&["test"], &many, root), 1);
    assert!(
        stdout.contains("found 20 violations at 1:1, 1:6, "),
        "{stdout}"
    );
    assert!(stdout.contains(", 1:76 and 4 more. "), "{stdout}");
}

#[test]
fn malformed_expect_all_fixtures_are_rejected() {
    let root = Path::new(ROOT);
    let at = |line: u64, column: u64| json!({ "line": line, "column": column });
    for (fixture, message) in [
        (
            json!({ "path": "notes/a.txt", "content": "", "expect": null, "expectAll": [] }),
            "provide exactly one of expect or expectAll",
        ),
        (
            json!({ "path": "notes/a.txt", "content": "" }),
            "provide exactly one of expect or expectAll",
        ),
        (
            json!({ "path": "notes/a.txt", "content": "", "expectAll": [at(1, 2), at(1, 1)] }),
            "list expectAll locations in ascending order without duplicates",
        ),
        (
            json!({ "path": "notes/a.txt", "content": "", "expectAll": [at(1, 1), at(1, 1)] }),
            "list expectAll locations in ascending order without duplicates",
        ),
        (
            json!({ "path": "notes/a.txt", "content": "", "expectAll": [at(0, 1)] }),
            "fixture uses one-based locations",
        ),
    ] {
        let raw =
            serde_json::to_vec(&json!({ "version": 1, "rules": [counted_rule(json!([fixture]))] }))
                .unwrap();
        let (_, stderr) = run(cli(&["validate"], &raw, root), 2);
        assert_eq!(
            stderr,
            format!("rulecheck: [notes/no-todo] notes/a.txt: {message}\n")
        );
    }
}

const RULE: &str = r#"import { defineRule } from "@rulecheck/sdk";

export default defineRule({
  id: "notes/no-todo",
  kind: "text",
  paths: ["notes/*.txt"],
  pattern: { kind: "literal", value: "TODO" },
  remediation: "Resolve the marker.",
  fixtures: [
    { path: "notes/a.txt", content: "TODO TODO\n", expectAll: [{ line: 1, column: 1 }, { line: 1, column: 6 }] },
  ],
});
"#;

fn build(root: &Path) -> Output {
    Command::new("node")
        .arg(Path::new(ROOT).join("tools/build.mjs"))
        .arg("--root")
        .arg(root)
        .output()
        .expect("run build; install Node.js and run npm ci")
}

#[test]
fn builder_accepts_expect_all_and_reports_misuse_at_the_rule_file() {
    let repo = repo();
    let root = repo.path();
    let rule = root.join(".rulecheck/rules/todo.rule.ts");
    fs::create_dir_all(rule.parent().unwrap()).unwrap();
    fs::write(&rule, RULE).unwrap();
    let built = build(root);
    let (policy, _) = run(built, 0);
    let (stdout, _) = run(cli(&["test"], policy.as_bytes(), root), 0);
    assert_eq!(stdout, "1 fixtures passed\n");

    for (from, to, message) in [
        (
            "expectAll: [",
            "expect: null, expectAll: [",
            "10:66: fixture 1 has both expect and expectAll. Keep one of them.",
        ),
        (
            "{ line: 1, column: 6 }",
            "{ line: 1, col: 6 }",
            "10:99: Unknown property col in fixture 1 expectAll 2. Use line and column.",
        ),
        (
            "expectAll: [{ line: 1, column: 1 }, { line: 1, column: 6 }]",
            "expectAll: { line: 1, column: 1 }",
            "10:52: fixture 1 expectAll must be an array of { line, column }. \
             Change expectAll to an array of { line, column }.",
        ),
        (
            "expectAll: [{ line: 1, column: 1 }, { line: 1, column: 6 }]",
            "expectAll: [1]",
            "10:64: fixture 1 expectAll 1 must be an object. Write { line, column }.",
        ),
        (
            "kind: \"text\",\n  paths: [\"notes/*.txt\"],\n  pattern: { kind: \"literal\", value: \"TODO\" },",
            "kind: \"syntax\",\n  paths: [\"notes/*.txt\"],\n  selector: { language: \"typescript\", node: \"comment\" },",
            "10:52: fixture 1 uses expectAll, but syntax rules do not count matching nodes yet. Use expect.",
        ),
    ] {
        assert!(RULE.contains(from), "{from}");
        fs::write(&rule, RULE.replace(from, to)).unwrap();
        let (stdout, stderr) = run(build(root), 2);
        assert!(stdout.is_empty());
        assert_eq!(
            stderr,
            format!("rulecheck build: .rulecheck/rules/todo.rule.ts:{message}\n")
        );
    }
}

#[test]
fn installed_wrapper_passes_all_matches_to_the_evaluator() {
    let install = tempfile::tempdir().unwrap();
    let dir = install.path();
    for path in ["bin", "tools", "target/release"] {
        fs::create_dir_all(dir.join(path)).unwrap();
    }
    for path in ["bin/rulecheck.mjs", "tools/build.mjs", "package.json"] {
        fs::copy(Path::new(ROOT).join(path), dir.join(path)).unwrap();
    }
    symlink(
        Path::new(ROOT).join("node_modules"),
        dir.join("node_modules"),
    )
    .unwrap();
    fs::copy(
        env!("CARGO_BIN_EXE_rulecheck-evaluator"),
        dir.join("target/release/rulecheck-evaluator"),
    )
    .unwrap();
    let wrapper = |root: &Path, args: &[&str]| {
        Command::new("node")
            .arg(dir.join("bin/rulecheck.mjs"))
            .args(args)
            .current_dir(root)
            .env_remove("RULECHECK_WRAPPER_ACTIVE")
            .output()
            .unwrap()
    };

    let repo = repo();
    let root = repo.path();
    write(root, ".rulecheck/rules/todo.rule.ts", RULE);
    write(root, "notes/a.txt", "TODO\nTODO TODO\n");
    let sha = commit(root);
    let (stdout, _) = run(wrapper(root, &["test"]), 0);
    assert_eq!(stdout, "1 fixtures passed\n");
    let (stdout, _) = run(wrapper(root, &["check"]), 1);
    assert_eq!(
        stdout,
        "notes/a.txt:1:1: [notes/no-todo] Resolve the marker.\n"
    );
    let (all, _) = run(wrapper(root, &["check", "--all-matches"]), 1);
    assert_eq!(
        all,
        "notes/a.txt:1:1: [notes/no-todo] Resolve the marker.\n\
         notes/a.txt:2:1: [notes/no-todo] Resolve the marker.\n\
         notes/a.txt:2:6: [notes/no-todo] Resolve the marker.\n"
    );
    let (exact, _) = run(
        wrapper(root, &["check", "--commit", &sha, "--all-matches"]),
        1,
    );
    assert_eq!(exact, all);
    let (stdout, _) = run(
        wrapper(root, &["check", "--all-matches", "--format", "json"]),
        1,
    );
    let result: Value = serde_json::from_str(&stdout).unwrap();
    assert!(common::violations(&schema("result-v1.json"), &result).is_empty());
    assert_eq!(result["source"]["matches"], "all");
    assert_eq!(result["findings"].as_array().unwrap().len(), 3);

    // A builder failure still records the requested mode in the error result.
    write(
        root,
        ".rulecheck/rules/todo.rule.ts",
        &RULE.replace("remediation:", "remedy:"),
    );
    let (stdout, _) = run(
        wrapper(root, &["check", "--all-matches", "--format", "json"]),
        2,
    );
    let result: Value = serde_json::from_str(&stdout).unwrap();
    assert!(common::violations(&schema("result-v1.json"), &result).is_empty());
    assert_eq!(
        result["source"],
        json!({ "mode": "working-tree", "commit": null, "matches": "all" })
    );

    for args in [
        &["test", "--all-matches"][..],
        &["check", "--all-matches", "--all-matches"],
    ] {
        let (_, stderr) = run(wrapper(root, args), 2);
        assert!(
            stderr.contains("unexpected argument \"--all-matches\""),
            "{stderr}"
        );
    }
    let (help, _) = run(wrapper(root, &["help"]), 0);
    assert!(help.contains("[--all-matches]"), "{help}");
}
