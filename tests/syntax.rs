//! Pins the `syntax` rule kind: the TypeScript/TSX comment selector from the
//! cookbook, a second rule that reuses it with no evaluator change, a shadow
//! comparison against the existing regex text checker, files that cannot be
//! judged, and the exact parser and grammar versions. The Rust recipe pins the
//! same selector over the second language.
mod common;

use serde_json::{Value, json};
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

const ROOT: &str = env!("CARGO_MANIFEST_DIR");
const COMMENTS: &str = include_str!("../docs/examples/rules/syntax.rule.ts");
const REUSE: &str = include_str!("../docs/examples/rules/syntax-reuse.rule.ts");
const RUST: &str = include_str!("../docs/examples/rules/syntax-rust.rule.ts");

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
            "syntax fixture",
        ],
    );
    git(root, &["rev-parse", "HEAD"])
}

fn write(root: &Path, path: &str, content: &str) {
    let path = root.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

/// A repository whose `.rulecheck/rules` holds `rules`, built into policy
/// bytes by the shipped builder.
fn repo_with(rules: &[(&str, &str)]) -> (tempfile::TempDir, Vec<u8>) {
    let repo = tempfile::tempdir().unwrap();
    git(repo.path(), &["init", "-q"]);
    for (name, source) in rules {
        write(repo.path(), &format!(".rulecheck/rules/{name}"), source);
    }
    let output = Command::new("node")
        .arg(Path::new(ROOT).join("tools/build.mjs"))
        .arg("--root")
        .arg(repo.path())
        .output()
        .expect("run build; install Node.js and run npm ci");
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    (repo, output.stdout)
}

fn policy(rules: &[Value]) -> Vec<u8> {
    json!({ "version": 1, "rules": rules })
        .to_string()
        .into_bytes()
}

fn comment_rule(selector: Value) -> Value {
    json!({
        "id": "source/no-comments", "kind": "syntax", "paths": ["src/**"],
        "selector": selector,
        "remediation": "Remove the comment.",
        "fixtures": [{ "path": "src/a.ts", "content": "// c\n", "expect": { "line": 1, "column": 1 } }]
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

#[test]
fn cookbook_rules_pass_their_fixtures_and_match_the_schema() {
    let (repo, policy) = repo_with(&[
        ("syntax.rule.ts", COMMENTS),
        ("syntax-reuse.rule.ts", REUSE),
    ]);
    assert_eq!(
        run(cli(&["test"], &policy, repo.path()), 0),
        "21 fixtures passed\n"
    );
    let schema: Value = serde_json::from_str(include_str!("../schema/policy-v1.json")).unwrap();
    let value: Value = serde_json::from_slice(&policy).unwrap();
    assert_eq!(common::violations(&schema, &value), Vec::<String>::new());

    // Both rules share one selector shape and differ only in the node kind.
    let rules = value["rules"].as_array().unwrap();
    assert_eq!(
        rules[0]["selector"]["inside"],
        rules[1]["selector"]["inside"]
    );
    assert_eq!(rules[0]["selector"]["node"], "non_null_expression");
    assert_eq!(rules[1]["selector"]["node"], "comment");
}

#[test]
fn schema_requires_exactly_one_matcher_per_kind() {
    let schema: Value = serde_json::from_str(include_str!("../schema/policy-v1.json")).unwrap();
    let selector = json!({ "language": "typescript", "node": "comment" });
    let pattern = json!({ "kind": "literal", "value": "x" });
    let mut both = comment_rule(selector.clone());
    both["pattern"] = pattern.clone();
    let mut text_with_selector = both.clone();
    text_with_selector["kind"] = json!("text");
    let mut path_with_selector = comment_rule(selector.clone());
    path_with_selector["kind"] = json!("forbidden-path");
    path_with_selector["fixtures"] = json!([{ "path": "src/a.ts", "expect": null }]);
    let mut syntax_without_content = comment_rule(selector);
    syntax_without_content["fixtures"] = json!([{ "path": "src/a.ts", "expect": null }]);
    for rule in [
        both,
        text_with_selector,
        path_with_selector,
        syntax_without_content,
    ] {
        let bytes = policy(&[rule]);
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert_ne!(common::violations(&schema, &value), Vec::<String>::new());
        let error = run(cli(&["validate"], &bytes, Path::new(ROOT)), 2);
        assert!(
            error.contains("require") || error.contains("content only"),
            "{error}"
        );
    }
}

/// Files with comments in every position the selector distinguishes, checked
/// both from the working tree and from the commit.
#[test]
fn selector_reports_the_first_real_function_body_comment_per_file() {
    let (repo, policy) = repo_with(&[
        ("syntax.rule.ts", COMMENTS),
        ("syntax-reuse.rule.ts", REUSE),
    ]);
    let root = repo.path();
    write(
        root,
        "src/nested.ts",
        "// header\nexport function f(x?: number): /* type */ number {\n  // TODO: later\n  if (x) {\n    /* deep */ return x!;\n  }\n  return 0;\n}\n",
    );
    write(
        root,
        "src/strings.ts",
        "export function f() {\n  const a = \"// no\";\n  const b = `\n// no\n/* no */`;\n  return a + b;\n}\n",
    );
    write(
        root,
        "src/view.tsx",
        "export const View = (p: { t?: string }) => (\n  <p>\n    {/* note */}\n    {p.t!}\n  </p>\n);\n",
    );
    write(
        root,
        "src/directives.ts",
        "export function f() {\n  // eslint-disable-next-line no-console\n  console.log(1);\n  /* istanbul ignore next */\n  return 1; // why\n}\n",
    );
    git(root, &["add", "-A"]);
    let working = run(cli(&["check", "--format", "json"], &policy, root), 1);
    let sha = commit(root);
    let committed = run(
        cli(
            &["check", "--commit", &sha, "--format", "json"],
            &policy,
            root,
        ),
        1,
    );
    let comments = "source/no-function-body-comments";
    let bangs = "source/no-non-null-assertion";
    let expected = vec![
        (comments.into(), "src/directives.ts".into(), 5, 13, 122, 128),
        (comments.into(), "src/nested.ts".into(), 5, 5, 93, 103),
        (bangs.into(), "src/nested.ts".into(), 5, 23, 111, 113),
        (comments.into(), "src/view.tsx".into(), 3, 6, 56, 66),
        (bangs.into(), "src/view.tsx".into(), 4, 6, 73, 77),
    ];
    assert_eq!(findings(&working), expected);
    assert_eq!(findings(&committed), expected);

    let human = run(cli(&["check"], &policy, root), 1);
    assert!(
        human.starts_with(
            "src/directives.ts:5:13: [source/no-function-body-comments] \
             Move the explanation out of the function body or into a name.\n"
        ),
        "{human}"
    );
}

/// The existing text checker stays in place. This runs it beside the syntax
/// selector over one checkout and pins where they agree and differ: a
/// line-anchored comment regex is what a rule author can write today.
#[test]
fn shadow_comparison_with_the_existing_regex_checker() {
    let regex = r#"import { defineRule } from "@rulecheck/sdk";

export default defineRule({
  id: "source/no-comment-lines",
  kind: "text",
  paths: ["src/**/*.ts", "src/**/*.tsx"],
  pattern: { kind: "regex", value: "(?m)^[ \\t]*(?://|/\\*)" },
  remediation: "Remove the comment.",
  fixtures: [{ path: "src/a.ts", content: "  // c\n", expect: { line: 1, column: 1 } }],
});
"#;
    let (repo, policy) = repo_with(&[("regex.rule.ts", regex), ("syntax.rule.ts", COMMENTS)]);
    let root = repo.path();
    let cases = [
        // (file, content, text checker flags it, syntax selector flags it)
        (
            "src/body.ts",
            "function f() {\n  // explain\n  return 1;\n}\n",
            true,
            true,
        ),
        ("src/clean.ts", "export const x = 1;\n", false, false),
        (
            "src/header.ts",
            "// file header\nexport const x = 1;\n",
            true,
            false,
        ),
        (
            "src/return-type.ts",
            "function f():\n  /* type note */ number {\n  return 1;\n}\n",
            true,
            false,
        ),
        (
            "src/template.ts",
            "function f() {\n  return `\n// inside a string\n`;\n}\n",
            true,
            false,
        ),
        (
            "src/todo.ts",
            "function f() {\n  // TODO: split\n  return 1;\n}\n",
            true,
            false,
        ),
        (
            "src/directive.ts",
            "function f() {\n  // @ts-expect-error: runtime value\n  return g(1);\n}\n",
            true,
            false,
        ),
        (
            "src/trailing.ts",
            "function f() {\n  return 1; // explain\n}\n",
            false,
            true,
        ),
        (
            "src/jsx.tsx",
            "export const A = () => <div>{/* note */}</div>;\n",
            false,
            true,
        ),
    ];
    for (path, content, _, _) in cases {
        write(root, path, content);
    }
    git(root, &["add", "-A"]);
    let result = run(cli(&["check", "--format", "json"], &policy, root), 1);
    let found = findings(&result);
    let flagged = |rule: &str, path: &str| found.iter().any(|f| f.0 == rule && f.1 == path);
    let mut agree = 0;
    for (path, _, text, syntax) in cases {
        assert_eq!(
            flagged("source/no-comment-lines", path),
            text,
            "text checker on {path}"
        );
        assert_eq!(
            flagged("source/no-function-body-comments", path),
            syntax,
            "syntax selector on {path}"
        );
        agree += usize::from(text == syntax);
    }
    assert_eq!((agree, cases.len()), (2, 9));
}

#[test]
fn files_the_grammar_cannot_parse_cannot_be_judged() {
    let selector = json!({ "language": "typescript", "node": "comment" });
    let rule = policy(&[comment_rule(selector)]);
    let repo = tempfile::tempdir().unwrap();
    let root = repo.path();
    git(root, &["init", "-q"]);

    write(root, "src/bad.ts", "function f() {\n  return (;\n}\n");
    git(root, &["add", "-A"]);
    let error = run(cli(&["check"], &rule, root), 2);
    assert!(
        error.contains(
            "[source/no-comments] src/bad.ts: cannot parse as TypeScript near 2:10; \
             fix the syntax or narrow the path scope"
        ),
        "{error}"
    );
    let result: Value =
        serde_json::from_slice(&cli(&["check", "--format", "json"], &rule, root).stdout).unwrap();
    assert_eq!(result["exitCode"], 2);

    // JSX is TSX syntax, not TypeScript, so the extension picks the grammar.
    fs::remove_file(root.join("src/bad.ts")).unwrap();
    write(root, "src/a.ts", "const a = <div />;\n");
    write(root, "src/a.tsx", "const a = <div />;\n");
    git(root, &["add", "-A"]);
    let error = run(cli(&["check"], &rule, root), 2);
    assert!(
        error.contains("src/a.ts: cannot parse as TypeScript"),
        "{error}"
    );
    fs::remove_file(root.join("src/a.ts")).unwrap();
    git(root, &["add", "-A"]);
    assert_eq!(run(cli(&["check"], &rule, root), 0), "");

    write(root, "src/a.js", "// c\n");
    git(root, &["add", "-A"]);
    let error = run(cli(&["check"], &rule, root), 2);
    assert!(
        error.contains("src/a.js: syntax rules for typescript apply to .ts, .mts, .cts and .tsx"),
        "{error}"
    );
}

#[test]
fn a_fixture_that_does_not_parse_fails_the_test_command() {
    let mut rule = comment_rule(json!({ "language": "typescript", "node": "comment" }));
    rule["fixtures"] = json!([
        { "path": "src/a.ts", "content": "function (\n", "expect": null },
        { "path": "src/a.ts", "content": "// c\n", "expect": { "line": 1, "column": 1 } }
    ]);
    let output = run(cli(&["test"], &policy(&[rule]), Path::new(ROOT)), 1);
    assert!(
        output.starts_with(
            "src/a.ts:1:1: [source/no-comments] fixture 1 cannot be judged: cannot parse as TypeScript"
        ),
        "{output}"
    );
    assert!(!output.contains("fixtures passed"), "{output}");
}

#[test]
fn selector_names_are_checked_against_both_grammars() {
    for (selector, message) in [
        (
            json!({ "language": "typescript", "node": "coment" }),
            "selector node \"coment\" is not a named node kind in every TypeScript grammar",
        ),
        (
            // JSX exists only in the TSX grammar, so one rule can't cover .ts files with it.
            json!({ "language": "typescript", "node": "jsx_element" }),
            "selector node \"jsx_element\" is not a named node kind",
        ),
        (
            json!({ "language": "typescript", "node": "comment",
                    "inside": { "kinds": ["function_declaration"], "field": "bdy" } }),
            "selector inside.field \"bdy\" is not a field in every TypeScript grammar",
        ),
        (
            json!({ "language": "typescript", "node": "comment", "inside": { "kinds": [] } }),
            "provide 1 to 16 selector inside.kinds",
        ),
        (
            json!({ "language": "typescript", "node": "comment",
                    "ignore": [{ "kind": "regex", "value": "(" }] }),
            "invalid selector ignore regex",
        ),
        (
            json!({ "language": "go", "node": "comment" }),
            "unknown variant `go`, expected `typescript` or `rust`",
        ),
        (
            // Rust has line_comment and block_comment, no plain comment kind.
            json!({ "language": "rust", "node": "comment" }),
            "selector node \"comment\" is not a named node kind in every Rust grammar",
        ),
        (
            json!({ "language": "typescript", "node": "line_comment" }),
            "selector node \"line_comment\" is not a named node kind in every TypeScript grammar",
        ),
        (
            json!({ "language": "rust", "node": "line_comment",
                    "inside": { "kinds": ["function_declaration"] } }),
            "selector inside kind \"function_declaration\" is not a named node kind in every Rust grammar",
        ),
        (
            json!({ "language": "rust", "node": "line_comment",
                    "inside": { "kinds": ["function_item"], "field": "bdy" } }),
            "selector inside.field \"bdy\" is not a field in every Rust grammar",
        ),
        (
            json!({ "language": "typescript", "node": "comment", "query": "(comment)" }),
            "unknown field `query`",
        ),
    ] {
        let error = run(
            cli(
                &["validate"],
                &policy(&[comment_rule(selector)]),
                Path::new(ROOT),
            ),
            2,
        );
        assert!(error.contains(message), "{error}");
    }
}

#[test]
fn parser_and_grammar_versions_are_pinned() {
    let manifest = include_str!("../Cargo.toml");
    let lock = include_str!("../Cargo.lock");
    for (name, version) in [
        ("tree-sitter", "0.27.2"),
        ("tree-sitter-typescript", "0.23.2"),
        ("tree-sitter-rust", "0.24.2"),
    ] {
        assert!(
            manifest.contains(&format!("\n{name} = \"={version}\"\n")),
            "pin {name} exactly in Cargo.toml"
        );
        assert!(
            lock.contains(&format!("name = \"{name}\"\nversion = \"{version}\"\n")),
            "Cargo.lock must resolve {name} {version}"
        );
    }
}

#[test]
fn rust_recipe_passes_its_fixtures_and_matches_the_schema() {
    let (repo, policy) = repo_with(&[("syntax-rust.rule.ts", RUST)]);
    assert_eq!(
        run(cli(&["test"], &policy, repo.path()), 0),
        "16 fixtures passed\n"
    );
    let schema: Value = serde_json::from_str(include_str!("../schema/policy-v1.json")).unwrap();
    let value: Value = serde_json::from_slice(&policy).unwrap();
    assert_eq!(common::violations(&schema, &value), Vec::<String>::new());
    let selector = &value["rules"][0]["selector"];
    assert_eq!(selector["language"], "rust");
    assert_eq!(selector["node"], "line_comment");
}

/// Rust files with comments in every position the selector distinguishes,
/// checked from the working tree and from the commit. A block-comment rule
/// built from the same `inside` value needs no evaluator change.
#[test]
fn rust_selector_reports_the_first_real_function_body_comment_per_file() {
    let (repo, policy) = repo_with(&[("syntax-rust.rule.ts", RUST)]);
    let mut value: Value = serde_json::from_slice(&policy).unwrap();
    let mut blocks = value["rules"][0].clone();
    blocks["id"] = json!("source/no-rust-block-comments");
    blocks["selector"]["node"] = json!("block_comment");
    blocks["selector"].as_object_mut().unwrap().remove("ignore");
    blocks["fixtures"] = json!([
        { "path": "src/a.rs", "content": "fn f() {\n    /* c */\n}\n", "expect": { "line": 2, "column": 5 } },
        { "path": "src/a.rs", "content": "fn f() -> /* c */ i32 {\n    1\n}\n", "expect": null }
    ]);
    value["rules"].as_array_mut().unwrap().push(blocks);
    let policy = value.to_string().into_bytes();
    assert_eq!(
        run(cli(&["test"], &policy, repo.path()), 0),
        "18 fixtures passed\n"
    );

    let root = repo.path();
    write(
        root,
        "src/nested.rs",
        "//! Header.\npub fn f(x: Option<i32>) -> // return type\n    i32 {\n    // TODO: later\n    match x {\n        Some(v) => {\n            // deep\n            v\n        }\n        None => 0,\n    }\n}\n",
    );
    write(
        root,
        "src/strings.rs",
        "pub fn f() -> String {\n    let a = \"// no\";\n    let b = r#\"/* no */ // no\"#;\n    let c = '/';\n    format!(\"{a}{b}{c}\")\n}\n",
    );
    write(
        root,
        "src/directives.rs",
        "pub fn f(p: *const i32) -> i32 {\n    // SAFETY: the caller passes a valid pointer.\n    let v = unsafe { *p };\n    // FIXME: overflow\n    v + 1 // why\n}\n",
    );
    write(
        root,
        "src/blocks.rs",
        "pub fn f() -> /* type */ i32 {\n    let add = |a: i32, b: i32| {\n        /* inner */ a + b\n    };\n    add(1, 2)\n}\n",
    );
    git(root, &["add", "-A"]);
    let working = run(cli(&["check", "--format", "json"], &policy, root), 1);
    let sha = commit(root);
    let committed = run(
        cli(
            &["check", "--commit", &sha, "--format", "json"],
            &policy,
            root,
        ),
        1,
    );
    let lines = "source/no-rust-function-body-comments";
    let blocks = "source/no-rust-block-comments";
    let expected = vec![
        (blocks.into(), "src/blocks.rs".into(), 3, 9, 72, 83),
        (lines.into(), "src/directives.rs".into(), 5, 11, 143, 149),
        (lines.into(), "src/nested.rs".into(), 7, 13, 131, 138),
    ];
    assert_eq!(findings(&working), expected);
    assert_eq!(findings(&committed), expected);
}

#[test]
fn rust_files_the_grammar_cannot_parse_cannot_be_judged() {
    let mut rule = comment_rule(json!({ "language": "rust", "node": "line_comment" }));
    rule["fixtures"] =
        json!([{ "path": "src/a.rs", "content": "// c\n", "expect": { "line": 1, "column": 1 } }]);
    let rule = policy(&[rule]);
    let repo = tempfile::tempdir().unwrap();
    let root = repo.path();
    git(root, &["init", "-q"]);

    write(root, "src/bad.rs", "fn f() {\n    let = 1;\n}\n");
    git(root, &["add", "-A"]);
    let error = run(cli(&["check"], &rule, root), 2);
    assert!(
        error.contains("[source/no-comments] src/bad.rs: cannot parse as Rust near 2:"),
        "{error}"
    );
    fs::remove_file(root.join("src/bad.rs")).unwrap();
    write(root, "src/ok.rs", "fn f() {}\n");
    git(root, &["add", "-A"]);
    assert_eq!(run(cli(&["check"], &rule, root), 0), "");

    write(root, "src/a.ts", "// c\n");
    git(root, &["add", "-A"]);
    let error = run(cli(&["check"], &rule, root), 2);
    assert!(
        error.contains("src/a.ts: syntax rules for rust apply to .rs files; narrow the path scope"),
        "{error}"
    );
}
