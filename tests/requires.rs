//! Pins same-file conditional text rules (`requires`): a file where the
//! trigger `pattern` matches must also match `requires` somewhere, or each
//! trigger match is a finding. Covers no trigger, a present and a missing
//! requirement, first- and all-match reporting, baselines, working-tree and
//! commit agreement, and malformed or unreadable input that must exit 2.
mod common;

use serde_json::{Value, json};
use std::fs;
use std::io::Write;
use std::os::unix::fs::symlink;
use std::path::Path;
use std::process::{Command, Output, Stdio};

const ROOT: &str = env!("CARGO_MANIFEST_DIR");
const REMEDIATION: &str = "Call close() on every opened handle.";

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
            "Rulecheck requires fixture",
        ],
    );
    git(root, &["rev-parse", "HEAD"])
}

fn write(root: &Path, path: &str, content: &[u8]) {
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

/// `open(` in `src/` requires `close(` somewhere in the same file.
fn open_close(requires: Value) -> Value {
    json!({
        "id": "source/open-needs-close", "kind": "text", "paths": ["src/**/*.ts"],
        "pattern": { "kind": "regex", "value": "\\bopen\\(" },
        "requires": requires,
        "remediation": REMEDIATION,
        "fixtures": [{ "path": "src/x.ts", "content": "", "expect": null }]
    })
}

fn rule() -> Vec<u8> {
    policy(&[open_close(json!({ "kind": "literal", "value": "close(" }))])
}

fn json_check(args: &[&str], policy: &[u8], root: &Path, code: i32) -> Value {
    let (stdout, _) = run(cli(args, policy, root), code);
    let result: Value = serde_json::from_str(&stdout).unwrap();
    let errors = common::violations(&schema("result-v1.json"), &result);
    assert!(errors.is_empty(), "{errors:?}\n{result:#}");
    result
}

fn finding(path: &str, line: usize, column: usize) -> String {
    format!("{path}:{line}:{column}: [source/open-needs-close] {REMEDIATION}\n")
}

/// The same report from the working tree and from the exact commit, in human
/// and JSON form, apart from the JSON source.
fn agree(root: &Path, policy: &[u8], extra: &[&str], code: i32) -> Value {
    let sha = commit(root);
    let mut args = vec!["check"];
    args.extend_from_slice(extra);
    let (tree, _) = run(cli(&args, policy, root), code);
    let mut exact = args.clone();
    exact.extend_from_slice(&["--commit", &sha]);
    let (committed, _) = run(cli(&exact, policy, root), code);
    assert_eq!(tree, committed);
    args.extend_from_slice(&["--format", "json"]);
    exact.extend_from_slice(&["--format", "json"]);
    let tree = json_check(&args, policy, root, code);
    let committed = json_check(&exact, policy, root, code);
    assert_eq!(tree["findings"], committed["findings"]);
    assert_eq!(committed["source"]["commit"], json!(sha));
    tree
}

#[test]
fn a_trigger_without_its_requirement_fails_at_the_trigger() {
    let repo = repo();
    let root = repo.path();
    // No trigger: nothing is required.
    write(root, "src/clean.ts", b"export const ok = 1;\n");
    // Trigger and requirement, in either order and on different lines.
    write(root, "src/paired.ts", b"const h = open(p);\nh.close();\n");
    write(
        root,
        "src/reversed.ts",
        b"close(h);\n\nconst h = open(p);\n",
    );
    assert_eq!(agree(root, &rule(), &[], 0)["findings"], json!([]));
    assert_eq!(
        agree(root, &rule(), &["--all-matches"], 0)["findings"],
        json!([])
    );

    // A missing requirement reports the first trigger by default, and every
    // trigger with --all-matches, in the same file only.
    write(
        root,
        "src/leak.ts",
        "// π\nconst a = open(p);\nconst b = open(q);\n".as_bytes(),
    );
    let first = agree(root, &rule(), &[], 1);
    let (stdout, _) = run(cli(&["check"], &rule(), root), 1);
    assert_eq!(stdout, finding("src/leak.ts", 2, 11));
    assert_eq!(
        first["findings"],
        json!([{
            "ruleId": "source/open-needs-close", "path": "src/leak.ts",
            "line": 2, "column": 11, "byteStart": 16, "byteEnd": 21,
            "remediation": REMEDIATION,
        }])
    );
    let all = agree(root, &rule(), &["--all-matches"], 1);
    let (stdout, _) = run(cli(&["check", "--all-matches"], &rule(), root), 1);
    assert_eq!(
        stdout,
        finding("src/leak.ts", 2, 11) + &finding("src/leak.ts", 3, 11)
    );
    assert_eq!(all["findings"].as_array().unwrap().len(), 2);

    // A requirement in another file does not count.
    write(root, "src/closer.ts", b"export const c = () => close(h);\n");
    let (stdout, _) = run(cli(&["check"], &rule(), root), 1);
    assert_eq!(stdout, finding("src/leak.ts", 2, 11));
    // Adding the requirement to the file clears every trigger in it.
    write(
        root,
        "src/leak.ts",
        b"const a = open(p);\nconst b = open(q);\nclose(a); close(b);\n",
    );
    agree(root, &rule(), &["--all-matches"], 0);

    // A regex requirement works the same way.
    let regex = policy(&[open_close(
        json!({ "kind": "regex", "value": "\\.close\\(\\)" }),
    )]);
    run(cli(&["check"], &regex, root), 1);
    write(root, "src/leak.ts", b"const a = open(p);\na.close();\n");
    write(root, "src/reversed.ts", b"const h = open(p);\nh.close();\n");
    agree(root, &regex, &[], 0);
}

#[test]
fn fixtures_assert_trigger_locations_with_and_without_the_requirement() {
    let mut fixtures = open_close(json!({ "kind": "literal", "value": "close(" }));
    fixtures["fixtures"] = json!([
        { "path": "src/a.ts", "content": "export {};\n", "expect": null },
        { "path": "src/a.ts", "content": "open(p);\nclose(p);\n", "expect": null },
        { "path": "src/a.ts", "content": "open(p);\nopen(q);\n", "expect": { "line": 1, "column": 1 } },
        { "path": "src/a.ts", "content": "open(p);\nopen(q);\n", "expectAll": [
            { "line": 1, "column": 1 }, { "line": 2, "column": 1 }
        ] },
        { "path": "src/a.ts", "content": "open(p);\nclose(p);\n", "expectAll": [] },
        // Out of scope: no finding, even without the requirement.
        { "path": "lib/a.ts", "content": "open(p);\n", "expect": null },
    ]);
    let rules = policy(&[fixtures.clone()]);
    let (stdout, _) = run(cli(&["test"], &rules, Path::new(ROOT)), 0);
    assert_eq!(stdout, "6 fixtures passed\n");

    // A fixture that forgets the requirement fails at the trigger.
    fixtures["fixtures"][1]["content"] = json!("open(p);\n");
    let (stdout, _) = run(cli(&["test"], &policy(&[fixtures]), Path::new(ROOT)), 1);
    assert_eq!(
        stdout,
        format!(
            "src/a.ts:1:1: [source/open-needs-close] fixture 2 expected no violation, found a \
             violation at 1:1. {REMEDIATION} Update the rule or fixture.\n"
        )
    );
}

#[test]
fn malformed_requirements_are_refused_before_any_file_is_read() {
    let root = Path::new(ROOT);
    let validate = |rule: Value| {
        let bytes = serde_json::to_vec(&json!({ "version": 1, "rules": [rule] })).unwrap();
        run(cli(&["validate"], &bytes, root), 2).1
    };
    let error = validate(open_close(json!({ "kind": "regex", "value": "close(" })));
    assert!(
        error.starts_with("rulecheck: [source/open-needs-close] requires: invalid regex"),
        "{error}"
    );
    // A requirement that can match empty text would be met by every file.
    for value in ["(close\\()?", "x*", "^", "|close"] {
        let error = validate(open_close(json!({ "kind": "regex", "value": value })));
        assert!(
            error.contains("requires: the regex can match empty text"),
            "{error}"
        );
    }
    let error = validate(open_close(json!({ "kind": "literal", "value": "" })));
    assert!(
        error.contains("requires: use a pattern of 1 to 4096 bytes"),
        "{error}"
    );
    let error = validate(open_close(json!({ "kind": "glob", "value": "x" })));
    assert!(error.contains("unknown variant `glob`"), "{error}");
    let error = validate(open_close(json!("close(")));
    assert!(error.contains("invalid policy JSON"), "{error}");

    // Only text rules take requires, in the schema and in the evaluator.
    let path_rule = json!({
        "id": "config/no-pem", "kind": "forbidden-path", "paths": ["**/*.pem"],
        "requires": { "kind": "literal", "value": "x" },
        "remediation": "Remove the key.",
        "fixtures": [{ "path": "a.pem", "expect": { "line": 1, "column": 1 } }]
    });
    let syntax_rule = json!({
        "id": "source/no-comments", "kind": "syntax", "paths": ["src/**/*.ts"],
        "selector": { "language": "typescript", "node": "comment" },
        "requires": { "kind": "literal", "value": "x" },
        "remediation": "Remove the comment.",
        "fixtures": [{ "path": "src/a.ts", "content": "", "expect": null }]
    });
    for rule in [path_rule, syntax_rule] {
        let document = json!({ "version": 1, "rules": [rule.clone()] });
        assert!(!common::violations(&schema("policy-v1.json"), &document).is_empty());
        let error = validate(rule);
        assert!(error.contains("only text rules take requires"), "{error}");
    }
    let mut extra = open_close(json!({ "kind": "literal", "value": "close(" }));
    extra["requires"]["flags"] = json!("i");
    let document = json!({ "version": 1, "rules": [extra.clone()] });
    assert!(!common::violations(&schema("policy-v1.json"), &document).is_empty());
    assert!(validate(extra).contains("unknown field `flags`"));
}

#[test]
fn unreadable_or_non_utf8_candidates_cannot_be_judged() {
    // Text that cannot be decoded neither meets nor misses the requirement:
    // the whole check exits 2 with no findings, never a pass.
    let repo = repo();
    let root = repo.path();
    write(root, "src/a.ts", b"const h = open(p);\n");
    write(root, "src/b.ts", b"open(p); \xff close(p);\n");
    let sha = commit(root);
    for args in [vec!["check"], vec!["check", "--commit", &sha]] {
        let (stdout, stderr) = run(cli(&args, &rule(), root), 2);
        assert_eq!(stdout, "");
        assert!(
            stderr.contains("[source/open-needs-close] src/b.ts: expected UTF-8 text"),
            "{stderr}"
        );
        let mut json = args.clone();
        json.extend_from_slice(&["--format", "json"]);
        let result = json_check(&json, &rule(), root, 2);
        assert_eq!(result["outcome"], "error");
        assert_eq!(result["findings"], json!([]));
    }

    // A tracked file that became a symlink is not followed, and a commit
    // whose scoped entry is a symlink is refused.
    write(root, "src/b.ts", b"close(p);\n");
    commit(root);
    fs::remove_file(root.join("src/b.ts")).unwrap();
    symlink("a.ts", root.join("src/b.ts")).unwrap();
    let (stdout, stderr) = run(cli(&["check"], &rule(), root), 2);
    assert_eq!(stdout, "");
    assert!(stderr.contains("without following symlinks"), "{stderr}");
    let sha = commit(root);
    let (stdout, stderr) = run(cli(&["check", "--commit", &sha], &rule(), root), 2);
    assert_eq!(stdout, "");
    assert!(stderr.contains("expected a regular Git blob"), "{stderr}");

    // An empty scope cannot be judged either.
    let empty = self::repo();
    write(empty.path(), "lib/a.ts", b"open(p);\n");
    git(empty.path(), &["add", "-A"]);
    let (_, stderr) = run(cli(&["check"], &rule(), empty.path()), 2);
    assert!(
        stderr.contains("[source/open-needs-close] empty text scope"),
        "{stderr}"
    );
}

#[test]
fn baselines_count_unmet_triggers_and_track_the_requirement() {
    let repo = repo();
    let root = repo.path();
    write(root, "src/a.ts", b"open(p);\nopen(q);\n");
    write(root, "src/b.ts", b"open(p);\nclose(p);\n");
    commit(root);
    let (baseline, _) = run(cli(&["baseline"], &rule(), root), 0);
    let recorded: Value = serde_json::from_str(&baseline).unwrap();
    assert!(common::violations(&schema("baseline-v1.json"), &recorded).is_empty());
    assert_eq!(
        recorded["rules"]["source/open-needs-close"]["paths"],
        json!({ "src/a.ts": 2 })
    );
    write(root, "baseline.json", baseline.as_bytes());
    run(
        cli(&["check", "--baseline", "baseline.json"], &rule(), root),
        0,
    );

    // Dropping the requirement from b.ts adds findings with no allowance.
    write(root, "src/b.ts", b"open(p);\n");
    let (stdout, _) = run(
        cli(&["check", "--baseline", "baseline.json"], &rule(), root),
        1,
    );
    assert!(stdout.contains(&finding("src/b.ts", 1, 1)), "{stdout}");

    // A different requirement is a different matcher, so old counts are refused.
    let other = policy(&[open_close(
        json!({ "kind": "literal", "value": "dispose(" }),
    )]);
    let (_, stderr) = run(
        cli(&["check", "--baseline", "baseline.json"], &other, root),
        2,
    );
    assert!(stderr.contains("other match semantics"), "{stderr}");
    // Adding or removing requires changes the digest too.
    let mut plain = open_close(json!(null));
    plain.as_object_mut().unwrap().remove("requires");
    let plain = policy(&[plain]);
    let (baseline, _) = run(cli(&["baseline"], &plain, root), 0);
    let plain: Value = serde_json::from_str(&baseline).unwrap();
    assert_ne!(
        plain["rules"]["source/open-needs-close"]["matcher"],
        recorded["rules"]["source/open-needs-close"]["matcher"]
    );
}

/// Runs the TypeScript rule builder over one rule source.
fn build(source: &str) -> Output {
    let repo = repo();
    write(repo.path(), ".rulecheck/rules/a.rule.ts", source.as_bytes());
    Command::new("node")
        .arg(Path::new(ROOT).join("tools/build.mjs"))
        .arg("--root")
        .arg(repo.path())
        .output()
        .expect("run build; install Node.js and run npm ci")
}

#[test]
fn the_builder_passes_requires_through_and_points_at_misuse() {
    let rule = |kind: &str, extra: &str, requires: &str| {
        format!(
            "import {{ defineRule }} from \"@pmdroid/rulecheck\";\n\
             export default defineRule({{\n  id: \"a/b\",\n  kind: \"{kind}\",\n  paths: [\"**/*.pem\"],\n{extra}\
             \x20 requires: {requires},\n  remediation: \"x\",\n  fixtures: [{{ path: \"a.pem\", expect: null }}],\n}});\n"
        )
    };
    let output = build(&rule(
        "forbidden-path",
        "",
        "{ kind: \"literal\", value: \"x\" }",
    ));
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains(
            "a.rule.ts:6:3: Forbidden-path rules have no requires. Remove requires or use kind: \"text\"."
        ),
        "{stderr}"
    );
    let output = build(&rule(
        "text",
        "  pattern: { kind: \"literal\", value: \"x\" },\n",
        "{ kind: \"glob\", value: \"x\" }",
    ));
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("requires kind must be \"literal\" or \"regex\""),
        "{stderr}"
    );
}
