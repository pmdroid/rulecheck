//! Pins `ignoreInside` on text rules: a pattern match that lies wholly inside
//! a parsed TypeScript comment or string is skipped, one match at a time. An
//! ignored occurrence never hides a later match, a match that leaves the
//! context is still reported, and a candidate that cannot be classified (an
//! unparsable file, or a file the grammar does not cover) exits 2 instead of
//! being skipped or reported.
mod common;

use serde_json::{Value, json};
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

const ROOT: &str = env!("CARGO_MANIFEST_DIR");
const REMEDIATION: &str = "Parse the input instead of calling eval.";

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
            "--allow-empty",
            "-qm",
            "Rulecheck ignoreInside fixture",
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

/// `eval(` in `src/` code, but not in comments or strings.
fn no_eval(contexts: Value) -> Value {
    json!({
        "id": "source/no-eval", "kind": "text", "paths": ["src/**"],
        "pattern": { "kind": "regex", "value": "\\beval\\(" },
        "ignoreInside": { "language": "typescript", "contexts": contexts },
        "remediation": REMEDIATION,
        "fixtures": [{ "path": "src/x.ts", "content": "", "expect": null }]
    })
}

fn rule() -> Vec<u8> {
    policy(&[no_eval(json!(["comment", "string"]))])
}

fn finding(path: &str, line: usize, column: usize) -> String {
    format!("{path}:{line}:{column}: [source/no-eval] {REMEDIATION}\n")
}

fn json_check(args: &[&str], policy: &[u8], root: &Path, code: i32) -> Value {
    let (stdout, _) = run(cli(args, policy, root), code);
    let result: Value = serde_json::from_str(&stdout).unwrap();
    let errors = common::violations(&schema("result-v1.json"), &result);
    assert!(errors.is_empty(), "{errors:?}\n{result:#}");
    result
}

/// `(line, column, byteStart)` of each finding, after checking that the
/// working tree and the exact commit agree.
fn agree(root: &Path, policy: &[u8], extra: &[&str], code: i32) -> Vec<(u64, u64, u64)> {
    let sha = commit(root);
    let mut args = vec!["check", "--format", "json"];
    args.extend_from_slice(extra);
    let tree = json_check(&args, policy, root, code);
    args.extend_from_slice(&["--commit", &sha]);
    let committed = json_check(&args, policy, root, code);
    assert_eq!(tree["findings"], committed["findings"]);
    tree["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|finding| {
            (
                finding["line"].as_u64().unwrap(),
                finding["column"].as_u64().unwrap(),
                finding["byteStart"].as_u64().unwrap(),
            )
        })
        .collect()
}

#[test]
fn an_ignored_occurrence_does_not_hide_a_later_match() {
    let repo = repo();
    let root = repo.path();
    // Every occurrence is in a comment, a string or a template literal part.
    write(
        root,
        "src/clean.ts",
        "// never call eval(input)\n/* eval(a) */\nconst a = \"eval(x)\";\nconst b = 'eval(y)';\nconst c = `eval(${a})`;\n",
    );
    assert_eq!(agree(root, &rule(), &[], 0), []);
    assert_eq!(agree(root, &rule(), &["--all-matches"], 0), []);

    // The comment on line 1 and the string on line 2 are skipped, and the
    // call after them on line 2 is the first finding.
    write(
        root,
        "src/run.ts",
        "// eval(old)\nconst s = \"eval(\"; eval(s);\n",
    );
    let (stdout, _) = run(cli(&["check"], &rule(), root), 1);
    assert_eq!(stdout, finding("src/run.ts", 2, 20));
    assert_eq!(agree(root, &rule(), &[], 1), [(2, 20, 32)]);

    // A substitution in a template string is code: its call is reported,
    // while the template's literal text before it is not.
    write(root, "src/run.ts", "const t = `eval(${eval(code)})`;\n");
    assert_eq!(agree(root, &rule(), &[], 1), [(1, 19, 18)]);

    // --all-matches reports every match outside the contexts, in order.
    write(
        root,
        "src/run.ts",
        "// eval(a)\neval(b); \"eval(c)\"; eval(d);\n/* eval(e) */ eval(f);\n",
    );
    assert_eq!(
        agree(root, &rule(), &["--all-matches"], 1),
        [(2, 1, 11), (2, 21, 31), (3, 15, 54)]
    );
}

#[test]
fn only_the_listed_contexts_are_ignored_and_only_whole_matches() {
    let repo = repo();
    let root = repo.path();
    write(root, "src/a.ts", "// eval(a)\nconst s = \"eval(b)\";\n");
    // Comments only: the string match is reported.
    let comments = policy(&[no_eval(json!(["comment"]))]);
    assert_eq!(agree(root, &comments, &[], 1), [(2, 12, 22)]);
    // Strings only: the comment match is reported.
    let strings = policy(&[no_eval(json!(["string"]))]);
    assert_eq!(agree(root, &strings, &[], 1), [(1, 4, 3)]);

    // A match that starts in a comment and ends in code is not inside it.
    let mut spanning = no_eval(json!(["comment"]));
    spanning["pattern"] = json!({ "kind": "regex", "value": "legacy\\s+call" });
    write(root, "src/a.ts", "// legacy\ncall();\n");
    assert_eq!(agree(root, &policy(&[spanning]), &[], 1), [(1, 4, 3)]);

    // The `string` type keyword is not a string literal.
    let mut keyword = no_eval(json!(["string"]));
    keyword["pattern"] = json!({ "kind": "literal", "value": "string" });
    write(root, "src/a.ts", "let s: string = \"string\";\n");
    assert_eq!(
        agree(root, &policy(&[keyword]), &["--all-matches"], 1),
        [(1, 8, 7)]
    );
}

#[test]
fn requires_still_searches_the_whole_file() {
    let repo = repo();
    let root = repo.path();
    let mut interval = no_eval(json!(["comment"]));
    interval["pattern"] = json!({ "kind": "regex", "value": "\\bsetInterval\\(" });
    interval["requires"] = json!({ "kind": "literal", "value": "clearInterval(" });
    let interval = policy(&[interval]);
    // The trigger in the comment is skipped, the one in code is not, and the
    // requirement counts wherever it appears.
    write(root, "src/a.ts", "// setInterval(f)\nsetInterval(g);\n");
    assert_eq!(agree(root, &interval, &[], 1), [(2, 1, 18)]);
    write(
        root,
        "src/a.ts",
        "// clearInterval(id) on stop\nsetInterval(g);\n",
    );
    assert_eq!(agree(root, &interval, &[], 0), []);
}

#[test]
fn a_candidate_that_cannot_be_classified_cannot_be_judged() {
    let repo = repo();
    let root = repo.path();
    // A file that does not parse is only a problem once the pattern matches
    // in it: without a candidate there is nothing to classify.
    write(root, "src/bad.ts", "function f() {\n  return (;\n}\n");
    commit(root);
    assert_eq!(run(cli(&["check"], &rule(), root), 0).0, "");

    write(
        root,
        "src/bad.ts",
        "// eval(x)\nfunction f() {\n  return (;\n}\n",
    );
    let sha = commit(root);
    for args in [
        vec!["check"],
        vec!["check", "--all-matches"],
        vec!["check", "--commit", &sha],
    ] {
        let (stdout, stderr) = run(cli(&args, &rule(), root), 2);
        assert_eq!(stdout, "");
        assert!(
            stderr.contains(
                "[source/no-eval] src/bad.ts: ignoreInside cannot classify the match: \
                 cannot parse as TypeScript near 3:10; fix the syntax or narrow the path scope"
            ),
            "{stderr}"
        );
    }
    let result = json_check(&["check", "--format", "json"], &rule(), root, 2);
    assert_eq!(result["exitCode"], 2);

    // The grammar covers .ts, .mts, .cts and .tsx only.
    fs::remove_file(root.join("src/bad.ts")).unwrap();
    write(root, "src/a.js", "eval(x);\n");
    commit(root);
    let (_, stderr) = run(cli(&["check"], &rule(), root), 2);
    assert!(
        stderr.contains(
            "[source/no-eval] src/a.js: ignoreInside for typescript classifies \
             .ts, .mts, .cts and .tsx files only; narrow the path scope"
        ),
        "{stderr}"
    );
}

#[test]
fn fixtures_classify_matches_like_a_scan() {
    let mut fixtures = no_eval(json!(["comment", "string"]));
    fixtures["fixtures"] = json!([
        { "path": "src/a.ts", "content": "// eval(a)\nconst s = \"eval(b)\";\n", "expect": null },
        { "path": "src/a.ts", "content": "// eval(a)\neval(b);\n", "expect": { "line": 2, "column": 1 } },
        {
            "path": "src/a.tsx",
            "content": "/* eval(a) */ eval(b); eval(c);\n",
            "expectAll": [{ "line": 1, "column": 15 }, { "line": 1, "column": 24 }]
        }
    ]);
    let (stdout, _) = run(cli(&["test"], &policy(&[fixtures]), Path::new(ROOT)), 0);
    assert_eq!(stdout, "3 fixtures passed\n");

    let mut broken = no_eval(json!(["comment"]));
    broken["fixtures"] = json!([
        { "path": "src/a.ts", "content": "eval(\n", "expect": null }
    ]);
    let (stdout, _) = run(cli(&["test"], &policy(&[broken]), Path::new(ROOT)), 1);
    assert!(
        stdout.starts_with(
            "src/a.ts:1:1: [source/no-eval] fixture 1 cannot be judged: \
             ignoreInside cannot classify the match: cannot parse as TypeScript"
        ),
        "{stdout}"
    );
}

#[test]
fn malformed_ignore_inside_is_rejected() {
    let root = Path::new(ROOT);
    let schema = schema("policy-v1.json");
    let syntax = json!({
        "id": "source/no-comments", "kind": "syntax", "paths": ["src/**/*.ts"],
        "selector": { "language": "typescript", "node": "comment" },
        "ignoreInside": { "language": "typescript", "contexts": ["comment"] },
        "remediation": REMEDIATION,
        "fixtures": [{ "path": "src/x.ts", "content": "", "expect": null }]
    });
    let path = json!({
        "id": "config/no-env", "kind": "forbidden-path", "paths": [".env"],
        "ignoreInside": { "language": "typescript", "contexts": ["comment"] },
        "remediation": REMEDIATION,
        "fixtures": [{ "path": ".env", "expect": { "line": 1, "column": 1 } }]
    });
    for rule in [syntax, path] {
        let document = json!({ "version": 1, "rules": [rule] });
        assert!(!common::violations(&schema, &document).is_empty());
        let (_, stderr) = run(
            cli(&["validate"], &serde_json::to_vec(&document).unwrap(), root),
            2,
        );
        assert!(
            stderr.contains("only text rules take ignoreInside"),
            "{stderr}"
        );
    }

    for (ignore, message) in [
        (
            json!({ "language": "typescript", "contexts": [] }),
            "provide 1 or 2 ignoreInside contexts",
        ),
        (
            json!({ "language": "typescript", "contexts": ["comment", "comment"] }),
            "ignoreInside lists a context twice",
        ),
        (
            json!({ "language": "typescript", "contexts": ["regex"] }),
            "unknown variant `regex`",
        ),
        (
            json!({ "language": "rust", "contexts": ["comment"] }),
            "unknown variant `rust`",
        ),
        (
            json!({ "language": "typescript", "contexts": ["comment"], "nodes": [] }),
            "unknown field `nodes`",
        ),
    ] {
        let mut rule = no_eval(json!(["comment"]));
        rule["ignoreInside"] = ignore;
        let document = json!({ "version": 1, "rules": [rule] });
        let (_, stderr) = run(
            cli(&["validate"], &serde_json::to_vec(&document).unwrap(), root),
            2,
        );
        assert!(stderr.contains(message), "{message}: {stderr}");
    }
}

#[test]
fn a_message_template_reads_the_reported_match() {
    let repo = repo();
    let root = repo.path();
    let mut named = no_eval(json!(["comment"]));
    named["pattern"] = json!({ "kind": "regex", "value": "\\beval\\((?P<arg>\\w+)\\)" });
    named["message"] = json!("{arg} is passed to eval.");
    let named = policy(&[named]);
    // The skipped comment match is never rendered: the message names the call.
    write(root, "src/a.ts", "// eval(old)\neval(input);\n");
    commit(root);
    let (stdout, _) = run(cli(&["check"], &named, root), 1);
    assert_eq!(
        stdout,
        format!("src/a.ts:2:1: [source/no-eval] input is passed to eval. {REMEDIATION}\n")
    );
}
