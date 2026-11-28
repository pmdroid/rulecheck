//! Pins the per-run scoped-text budget (`check --text-budget MIB`): files read
//! once for overlapping scopes, disjoint scopes that only fail together, the
//! partial result after a budget stop (deterministic findings, unevaluated
//! rules, exit 2 in human and JSON output), working-tree and commit parity,
//! argument validation, and forwarding through the installed wrapper.
//! Every repository, rule and file here is synthetic.
mod common;

use serde_json::{Value, json};
use std::fs;
use std::io::Write;
use std::os::unix::fs::symlink;
use std::path::Path;
use std::process::{Command, Output, Stdio};

const ROOT: &str = env!("CARGO_MANIFEST_DIR");
const KIB: usize = 1024;
const HINT: &str = "--text-budget requires a whole number of MiB from 1 to 1024 (default 64)";

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
            "Rulecheck text budget fixture",
        ],
    );
    git(root, &["rev-parse", "HEAD"])
}

fn write(root: &Path, path: &str, content: &[u8]) {
    let path = root.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

/// A text file of `size` bytes that starts with `MARKER` when `marked`.
fn text(size: usize, marked: bool) -> Vec<u8> {
    let mut content = vec![b'a'; size];
    if marked {
        content[..7].copy_from_slice(b"MARKER\n");
    }
    content
}

fn schema(name: &str) -> Value {
    let text = fs::read_to_string(Path::new(ROOT).join("schema").join(name)).unwrap();
    serde_json::from_str(&text).unwrap()
}

fn policy(rules: &[Value]) -> Vec<u8> {
    let policy = json!({ "version": 1, "rules": rules });
    let errors = common::violations(&schema("policy-v1.json"), &policy);
    assert!(errors.is_empty(), "{errors:?}");
    serde_json::to_vec(&policy).unwrap()
}

fn text_rule(id: &str, paths: &[&str]) -> Value {
    json!({
        "id": id, "kind": "text", "paths": paths,
        "pattern": { "kind": "literal", "value": "MARKER" },
        "remediation": "Remove the marker.",
        "fixtures": [{ "path": "elsewhere.txt", "content": "", "expect": null }]
    })
}

fn path_rule(id: &str, paths: &[&str]) -> Value {
    json!({
        "id": id, "kind": "forbidden-path", "paths": paths,
        "remediation": "Remove the file.",
        "fixtures": [{ "path": "elsewhere.txt", "expect": null }]
    })
}

fn json_check(args: &[&str], policy: &[u8], root: &Path, code: i32) -> (Value, String) {
    let mut args = args.to_vec();
    args.extend(["--format", "json"]);
    let (stdout, stderr) = run(cli(&args, policy, root), code);
    let result: Value = serde_json::from_str(&stdout).unwrap();
    let errors = common::violations(&schema("result-v1.json"), &result);
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(result["exitCode"], code);
    (result, stderr)
}

/// Two disjoint 600 KiB scopes, each under a 1 MiB budget on its own, plus a
/// forbidden-path rule whose only file sorts after both.
///
/// ```text
/// api/a.txt  300 KiB  MARKER     api/b.txt  300 KiB
/// web/a.txt  300 KiB  MARKER     web/b.txt  300 KiB  MARKER   <- 1200 KiB > 1 MiB
/// zz/forbidden.txt
/// ```
fn disjoint() -> (tempfile::TempDir, Vec<u8>) {
    let repo = tempfile::tempdir().unwrap();
    let root = repo.path();
    git(root, &["init", "-q"]);
    write(root, "api/a.txt", &text(300 * KIB, true));
    write(root, "api/b.txt", &text(300 * KIB, false));
    write(root, "web/a.txt", &text(300 * KIB, true));
    write(root, "web/b.txt", &text(300 * KIB, true));
    write(root, "zz/forbidden.txt", b"x\n");
    git(root, &["add", "-A"]);
    let rules = policy(&[
        text_rule("api/no-marker", &["api/**"]),
        text_rule("web/no-marker", &["web/**"]),
        path_rule("zz/no-forbidden", &["zz/forbidden.txt"]),
    ]);
    (repo, rules)
}

#[test]
fn overlapping_scopes_read_each_file_once() {
    let repo = tempfile::tempdir().unwrap();
    let root = repo.path();
    git(root, &["init", "-q"]);
    // 900 KiB of shared text; three rules over it would be 2700 KiB if each
    // rule re-read the files.
    for name in ["a", "b", "c"] {
        write(
            root,
            &format!("shared/{name}.txt"),
            &text(300 * KIB, name == "a"),
        );
    }
    git(root, &["add", "-A"]);
    let rules = policy(&[
        text_rule("one/no-marker", &["shared/**"]),
        text_rule("two/no-marker", &["shared/*.txt"]),
        text_rule("three/no-marker", &["**/*.txt"]),
    ]);
    let (stdout, stderr) = run(cli(&["check", "--text-budget", "1"], &rules, root), 1);
    assert_eq!(stderr, "");
    assert_eq!(
        stdout,
        "shared/a.txt:1:1: [one/no-marker] Remove the marker.\n\
         shared/a.txt:1:1: [three/no-marker] Remove the marker.\n\
         shared/a.txt:1:1: [two/no-marker] Remove the marker.\n"
    );
    let (result, _) = json_check(&["check", "--text-budget", "1"], &rules, root, 1);
    assert!(result.get("partial").is_none());
    assert_eq!(result["source"]["textBudgetMiB"], 1);
}

#[test]
fn disjoint_scopes_each_fit_but_stop_together_with_partial_findings() {
    let (repo, rules) = disjoint();
    let root = repo.path();

    // Each scope on its own is a complete scan under the same budget.
    let (api, _) = run(
        cli(
            &["check", "--text-budget", "1", "--id-prefix", "api/"],
            &rules,
            root,
        ),
        1,
    );
    assert_eq!(api, "api/a.txt:1:1: [api/no-marker] Remove the marker.\n");
    let (web, _) = run(
        cli(
            &["check", "--text-budget", "1", "--id-prefix", "web/"],
            &rules,
            root,
        ),
        1,
    );
    assert_eq!(
        web,
        "web/a.txt:1:1: [web/no-marker] Remove the marker.\n\
         web/b.txt:1:1: [web/no-marker] Remove the marker.\n"
    );

    // Together they stop at web/b.txt. The findings before it are kept and
    // the scan still exits 2, never 1.
    let (stdout, stderr) = run(cli(&["check", "--text-budget", "1"], &rules, root), 2);
    assert_eq!(
        stdout,
        "api/a.txt:1:1: [api/no-marker] Remove the marker.\n\
         web/a.txt:1:1: [web/no-marker] Remove the marker.\n"
    );
    let unevaluated = "not evaluated on every file in scope: the scan stopped at the text budget";
    let stop = format!(
        "rulecheck: scan stopped at web/b.txt: its text would bring the scan to {} bytes, over \
         the 1 MiB text budget, after {} bytes were evaluated; findings are partial and 2 rules \
         were not evaluated on every file in scope. Narrow the rule path scopes or select fewer \
         rules with --id-prefix; raise --text-budget (at most 1024) only if the scopes are \
         already tight\n",
        1200 * KIB,
        900 * KIB
    );
    assert_eq!(
        stderr,
        format!("[web/no-marker] {unevaluated}\n[zz/no-forbidden] {unevaluated}\n{stop}")
    );

    let (result, json_stderr) = json_check(&["check", "--text-budget", "1"], &rules, root, 2);
    assert_eq!(json_stderr, stderr);
    assert_eq!(result["outcome"], "error");
    assert_eq!(
        result["partial"],
        json!({
            "reason": "text-budget",
            "textBudgetBytes": 1024 * KIB,
            "textBytesEvaluated": 900 * KIB,
            "stoppedAt": "web/b.txt",
            "unevaluatedRuleIds": ["web/no-marker", "zz/no-forbidden"],
        })
    );
    let found: Vec<_> = result["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|finding| {
            (
                finding["path"].as_str().unwrap(),
                finding["ruleId"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        found,
        [
            ("api/a.txt", "api/no-marker"),
            ("web/a.txt", "web/no-marker")
        ]
    );
    // Each unevaluated rule has a diagnostic, so no consumer can read it as passing.
    let diagnostics: Vec<_> = result["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .map(|diagnostic| diagnostic["ruleId"].clone())
        .collect();
    assert_eq!(
        diagnostics,
        [
            json!("web/no-marker"),
            json!("zz/no-forbidden"),
            Value::Null
        ]
    );
    assert_eq!(
        result["diagnostics"][2]["message"].as_str().unwrap(),
        stop.trim_end().strip_prefix("rulecheck: ").unwrap()
    );

    // The same input stops at the same place with the same document.
    let (again, _) = json_check(&["check", "--text-budget", "1"], &rules, root, 2);
    assert_eq!(again, result);

    // A budget that covers both scopes completes the scan in one run.
    let (stdout, _) = run(cli(&["check", "--text-budget", "2"], &rules, root), 1);
    assert_eq!(
        stdout,
        "api/a.txt:1:1: [api/no-marker] Remove the marker.\n\
         web/a.txt:1:1: [web/no-marker] Remove the marker.\n\
         web/b.txt:1:1: [web/no-marker] Remove the marker.\n\
         zz/forbidden.txt:1:1: [zz/no-forbidden] Remove the file.\n"
    );
    let (complete, _) = json_check(&["check", "--text-budget", "2"], &rules, root, 1);
    assert!(complete.get("partial").is_none());
}

#[test]
fn a_budget_stop_is_reported_even_without_findings() {
    let (repo, _) = disjoint();
    let root = repo.path();
    // No marker before the stop: still exit 2, not 0.
    write(root, "api/a.txt", &text(300 * KIB, false));
    write(root, "web/a.txt", &text(300 * KIB, false));
    git(root, &["add", "-A"]);
    let rules = policy(&[
        text_rule("api/no-marker", &["api/**"]),
        text_rule("web/no-marker", &["web/**"]),
        // Its only file sorts after the stop, so its scope is not empty.
        text_rule("zz/no-marker", &["zz/**"]),
    ]);
    let (stdout, stderr) = run(cli(&["check", "--text-budget", "1"], &rules, root), 2);
    assert_eq!(stdout, "");
    assert!(!stderr.contains("empty text scope"), "{stderr}");
    let (result, _) = json_check(&["check", "--text-budget", "1"], &rules, root, 2);
    assert_eq!(result["findings"], json!([]));
    assert_eq!(
        result["partial"]["unevaluatedRuleIds"],
        json!(["web/no-marker", "zz/no-marker"])
    );
}

#[test]
fn empty_scopes_are_still_reported_beside_a_budget_stop() {
    let (repo, _) = disjoint();
    let root = repo.path();
    let rules = policy(&[
        text_rule("api/no-marker", &["api/**"]),
        text_rule("none/no-marker", &["none/**"]),
        text_rule("web/no-marker", &["web/**"]),
    ]);
    let (_, stderr) = run(cli(&["check", "--text-budget", "1"], &rules, root), 2);
    assert!(
        stderr.starts_with(
            "[none/no-marker] empty text scope: track a matching file or update the rule's path globs\n"
        ),
        "{stderr}"
    );
    assert!(stderr.contains("rulecheck: one or more text rules matched no repository files\n"));
    let (result, _) = json_check(&["check", "--text-budget", "1"], &rules, root, 2);
    assert_eq!(result["diagnostics"][0]["ruleId"], "none/no-marker");
    assert_eq!(
        result["partial"]["unevaluatedRuleIds"],
        json!(["web/no-marker"])
    );
}

#[test]
fn commit_scans_stop_at_the_same_place_as_the_working_tree() {
    let (repo, rules) = disjoint();
    let root = repo.path();
    let sha = commit(root);
    for budget in ["1", "2"] {
        let code = if budget == "1" { 2 } else { 1 };
        let args = ["check", "--text-budget", budget];
        let (tree, tree_stderr) = run(cli(&args, &rules, root), code);
        let (exact, exact_stderr) = run(
            cli(
                &["check", "--text-budget", budget, "--commit", &sha],
                &rules,
                root,
            ),
            code,
        );
        assert_eq!((&exact, &exact_stderr), (&tree, &tree_stderr));

        let (mut tree, _) = json_check(&args, &rules, root, code);
        let (mut exact, _) = json_check(
            &["check", "--text-budget", budget, "--commit", &sha],
            &rules,
            root,
            code,
        );
        assert_eq!(
            exact["source"],
            json!({ "mode": "commit", "commit": sha, "textBudgetMiB": budget.parse::<u64>().unwrap() })
        );
        tree["source"] = Value::Null;
        exact["source"] = Value::Null;
        assert_eq!(exact, tree);
    }
}

#[test]
fn the_default_budget_is_64_mib() {
    let repo = tempfile::tempdir().unwrap();
    let root = repo.path();
    git(root, &["init", "-q"]);
    let chunk = text(1024 * KIB, false);
    for n in 0..65 {
        write(root, &format!("bulk/{n:02}.txt"), &chunk);
    }
    git(root, &["add", "-A"]);
    let rules = policy(&[text_rule("bulk/no-marker", &["bulk/**"])]);
    let (result, _) = json_check(&["check"], &rules, root, 2);
    assert!(result["source"].get("textBudgetMiB").is_none());
    assert_eq!(
        result["partial"],
        json!({
            "reason": "text-budget",
            "textBudgetBytes": 64 * 1024 * KIB,
            "textBytesEvaluated": 64 * 1024 * KIB,
            "stoppedAt": "bulk/64.txt",
            "unevaluatedRuleIds": ["bulk/no-marker"],
        })
    );
    // An explicit 64 is the same scan, with the requested value recorded.
    let (explicit, _) = json_check(&["check", "--text-budget", "64"], &rules, root, 2);
    assert_eq!(explicit["partial"], result["partial"]);
    let (_, _) = json_check(&["check", "--text-budget", "65"], &rules, root, 0);
}

#[test]
fn per_file_and_finding_limits_still_apply_under_any_budget() {
    let repo = tempfile::tempdir().unwrap();
    let root = repo.path();
    git(root, &["init", "-q"]);
    write(root, "big/a.txt", &text(1024 * KIB + 1, false));
    git(root, &["add", "-A"]);
    let rules = policy(&[text_rule("big/no-marker", &["big/**"])]);
    let (result, _) = json_check(&["check", "--text-budget", "1024"], &rules, root, 2);
    assert!(result.get("partial").is_none());
    let message = result["diagnostics"][0]["message"].as_str().unwrap();
    assert!(message.contains("exceeds 1048576 bytes"), "{message}");

    fs::remove_dir_all(root.join("big")).unwrap();
    for n in 0..10_001 {
        write(root, &format!("many/{n:05}.txt"), b"MARKER\n");
    }
    git(root, &["add", "-A"]);
    let rules = policy(&[text_rule("many/no-marker", &["many/**"])]);
    let (result, _) = json_check(&["check", "--text-budget", "1024"], &rules, root, 2);
    assert_eq!(result["findings"], json!([]));
    assert!(result.get("partial").is_none());
    let message = result["diagnostics"][0]["message"].as_str().unwrap();
    assert!(message.contains("more than 10,000 findings"), "{message}");
}

#[test]
fn invalid_and_overflowing_budgets_are_refused_before_scanning() {
    let (repo, rules) = disjoint();
    let root = repo.path();
    for value in [
        "0",
        "1025",
        "4096",
        "-1",
        "+1",
        "1.5",
        "1e3",
        "64MiB",
        " 64",
        "",
        "18446744073709551616",
        "99999999999999999999999999999999",
        "--format",
    ] {
        let (stdout, stderr) = run(cli(&["check", "--text-budget", value], &rules, root), 2);
        assert_eq!(stdout, "", "{value:?}");
        assert_eq!(stderr, format!("rulecheck: {HINT}\n"), "{value:?}");
    }
    let (_, stderr) = run(cli(&["check", "--text-budget"], &rules, root), 2);
    assert_eq!(stderr, format!("rulecheck: {HINT}\n"));
    for args in [
        &["check", "--text-budget", "1", "--text-budget", "2"][..],
        &["test", "--text-budget", "1"],
        &["list", "--text-budget", "1"],
        &["validate", "--text-budget", "1"],
    ] {
        let (_, stderr) = run(cli(args, &rules, root), 2);
        assert!(
            stderr.contains("unexpected argument \"--text-budget\""),
            "{args:?}: {stderr}"
        );
    }
    let help = String::from_utf8(cli(&["help"], b"", root).stdout).unwrap();
    assert!(help.contains("[--text-budget MIB]"), "{help}");
    assert!(help.contains("1 to 1024"), "{help}");
}

const API_RULE: &str = r#"import { defineRule } from "@rulecheck/sdk";

export default defineRule({
  id: "api/no-marker",
  kind: "text",
  paths: ["api/**"],
  pattern: { kind: "literal", value: "MARKER" },
  remediation: "Remove the marker.",
  fixtures: [
    { path: "api/x.txt", content: "MARKER\n", expect: { line: 1, column: 1 } },
    { path: "api/y.txt", content: "clean\n", expect: null },
  ],
});
"#;

#[test]
fn installed_wrapper_forwards_the_budget() {
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

    let (repo, _) = disjoint();
    let root = repo.path();
    write(root, ".rulecheck/rules/api.rule.ts", API_RULE.as_bytes());
    write(
        root,
        ".rulecheck/rules/web.rule.ts",
        API_RULE.replace("api/", "web/").as_bytes(),
    );
    let sha = commit(root);

    let (stdout, stderr) = run(wrapper(root, &["check", "--text-budget", "1"]), 2);
    assert_eq!(
        stdout,
        "api/a.txt:1:1: [api/no-marker] Remove the marker.\n\
         web/a.txt:1:1: [web/no-marker] Remove the marker.\n"
    );
    assert!(
        stderr.contains("rulecheck: scan stopped at web/b.txt"),
        "{stderr}"
    );
    let (exact, exact_stderr) = run(
        wrapper(root, &["check", "--commit", &sha, "--text-budget", "1"]),
        2,
    );
    assert_eq!((exact, exact_stderr), (stdout, stderr));
    run(wrapper(root, &["check", "--text-budget", "2"]), 1);

    let (stdout, _) = run(
        wrapper(root, &["check", "--text-budget", "1", "--format", "json"]),
        2,
    );
    let result: Value = serde_json::from_str(&stdout).unwrap();
    assert!(common::violations(&schema("result-v1.json"), &result).is_empty());
    assert_eq!(result["partial"]["stoppedAt"], "web/b.txt");
    assert_eq!(result["source"]["textBudgetMiB"], 1);
    assert!(result["tools"]["builder"].is_string());

    // The wrapper refuses a bad value before it starts the builder.
    for value in ["0", "1025", "99999999999999999999", "1.5", "-1"] {
        let (stdout, stderr) = run(
            wrapper(root, &["check", "--text-budget", value, "--format", "json"]),
            2,
        );
        assert_eq!(stdout, "");
        assert_eq!(stderr, format!("rulecheck: {HINT}\n"), "{value:?}");
    }
    for args in [
        &["test", "--text-budget", "1"][..],
        &["check", "--text-budget", "1", "--text-budget", "1"],
    ] {
        let (_, stderr) = run(wrapper(root, args), 2);
        assert!(
            stderr.contains("unexpected argument \"--text-budget\""),
            "{stderr}"
        );
    }

    // A builder failure still records the requested budget in the error result.
    write(
        root,
        ".rulecheck/rules/api.rule.ts",
        API_RULE.replace("remediation:", "remedy:").as_bytes(),
    );
    let (stdout, _) = run(
        wrapper(root, &["check", "--text-budget", "8", "--format", "json"]),
        2,
    );
    let result: Value = serde_json::from_str(&stdout).unwrap();
    assert!(common::violations(&schema("result-v1.json"), &result).is_empty());
    assert_eq!(
        result["source"],
        json!({ "mode": "working-tree", "commit": null, "textBudgetMiB": 8 })
    );
    let (help, _) = run(wrapper(root, &["help"]), 0);
    assert!(help.contains("[--text-budget MIB]"), "{help}");
}
