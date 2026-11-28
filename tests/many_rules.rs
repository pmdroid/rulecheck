//! Pins a policy far past the old 128-rule cap: 360 rule files of every kind,
//! built from nested directories through the installed wrapper and checked in
//! the working tree, at an exact commit and through exact-commit CI evidence.
//! Both discovery modes agree on 1024 rules and refuse 1025 with one message;
//! invalid rules, settings and budgets fail closed with the rule named; and
//! rule, finding and diagnostic order does not depend on file order.
//! Every repository, rule and file here is synthetic.
mod common;

use serde_json::{Value, json};
use std::fs;
use std::io::Write;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::Instant;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");
const RULES: usize = 360;
const PER_PACKAGE: usize = 30;
const LIMIT: usize = 1024;
const KINDS: [&str; 4] = ["lit", "re", "path", "syn"];

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

fn commit(root: &Path, message: &str) -> String {
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
            message,
        ],
    );
    git(root, &["rev-parse", "HEAD"])
}

fn write(root: &Path, path: &str, content: &str) {
    let path = root.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

/// Rule `n` lives in package `n / 30` and has kind `n % 4`. Its ID counts
/// down, so ID order is the reverse of file order within each kind.
fn id(n: usize) -> String {
    format!("scale/{}-{:04}", KINDS[n % 4], 9999 - n)
}

fn package(n: usize) -> String {
    format!("pkg-{:02}", n / PER_PACKAGE)
}

fn rule_path(n: usize) -> String {
    format!(".rulecheck/rules/{}/r{n:04}.rule.ts", package(n))
}

fn rule_source(n: usize) -> String {
    let (id, pkg) = (id(n), package(n));
    let body = match n % 4 {
        0 => format!(
            r#"  kind: "text",
  paths: ["{pkg}/src/**/*.ts"],
  pattern: {{ kind: "literal", value: "LIT-{n}-" }},
  remediation: "Remove LIT-{n}-.",
  fixtures: [
    {{ path: "{pkg}/src/x.ts", content: "a\nLIT-{n}-\n", expect: {{ line: 2, column: 1 }} }},
    {{ path: "{pkg}/src/x.ts", content: "LIT-{n}0\n", expect: null }},
  ],"#
        ),
        1 => format!(
            r#"  kind: "text",
  paths: ["{pkg}/src/**/*.ts"],
  pattern: {{ kind: "regex", value: "RE-{n}-[0-9]+" }},
  remediation: "Remove RE-{n}.",
  fixtures: [
    {{ path: "{pkg}/src/x.ts", content: "  RE-{n}-7\n", expect: {{ line: 1, column: 3 }} }},
    {{ path: "{pkg}/src/x.ts", content: "RE-{n}-x\n", expect: null }},
  ],"#
        ),
        2 => format!(
            r#"  kind: "forbidden-path",
  paths: ["{pkg}/forbidden/f{n}.txt"],
  remediation: "Delete f{n}.txt.",
  fixtures: [
    {{ path: "{pkg}/forbidden/f{n}.txt", expect: {{ line: 1, column: 1 }} }},
    {{ path: "{pkg}/forbidden/g{n}.txt", expect: null }},
  ],"#
        ),
        _ => format!(
            r#"  kind: "syntax",
  paths: ["{pkg}/src/**/*.ts"],
  selector: {{ language: "typescript", node: "comment", ignore: [{{ kind: "literal", value: "allowed" }}] }},
  remediation: "Remove the comment.",
  fixtures: [
    {{ path: "{pkg}/src/x.ts", content: "// allowed\nlet a = 1; // note\n", expect: {{ line: 2, column: 12 }} }},
    {{ path: "{pkg}/src/x.ts", content: "let a = 1;\n", expect: null }},
  ],"#
        ),
    };
    format!(
        "import {{ defineRule }} from \"@rulecheck/sdk\";\n\n\
         export default defineRule({{\n  id: \"{id}\",\n{body}\n}});\n"
    )
}

/// A checkout with `rules` rule files and a clean file in every package.
fn repo(rules: usize) -> tempfile::TempDir {
    let repo = tempfile::tempdir().unwrap();
    let root = repo.path();
    git(root, &["init", "-q"]);
    for n in 0..rules {
        write(root, &rule_path(n), &rule_source(n));
    }
    for p in 0..rules.div_ceil(PER_PACKAGE) {
        write(
            root,
            &format!("pkg-{p:02}/src/a.ts"),
            "export const value = 1;\n",
        );
    }
    repo
}

/// Violations for every rule in the even packages: one file per package with a
/// comment on line 1 and each text marker on its own line, and each forbidden
/// path. Returns how many findings that adds.
fn violate(root: &Path, rules: usize) -> usize {
    let mut findings = 0;
    for p in (0..rules.div_ceil(PER_PACKAGE)).step_by(2) {
        let mut bad = String::from("// note\n");
        for n in (p * PER_PACKAGE..(p + 1) * PER_PACKAGE).filter(|n| *n < rules) {
            match n % 4 {
                0 => bad.push_str(&format!("LIT-{n}-\n")),
                1 => bad.push_str(&format!("RE-{n}-42\n")),
                2 => write(root, &format!("pkg-{p:02}/forbidden/f{n}.txt"), "x\n"),
                _ => {}
            }
            findings += 1;
        }
        write(root, &format!("pkg-{p:02}/src/bad.ts"), &bad);
    }
    findings
}

/// The installed layout: wrapper, builder and the evaluator under target/release.
struct Install(tempfile::TempDir);

impl Install {
    fn new() -> Self {
        let install = tempfile::tempdir().unwrap();
        let dir = install.path();
        for path in ["bin", "tools", "target/release"] {
            fs::create_dir_all(dir.join(path)).unwrap();
        }
        for path in [
            "bin/rulecheck.mjs",
            "tools/build.mjs",
            "tools/check-ci.mjs",
            "package.json",
        ] {
            fs::copy(Path::new(ROOT).join(path), dir.join(path)).unwrap();
        }
        symlink(
            Path::new(ROOT).join("node_modules"),
            dir.join("node_modules"),
        )
        .unwrap();
        fs::copy(evaluator(), dir.join("target/release/rulecheck-evaluator")).unwrap();
        Self(install)
    }

    fn rulecheck(&self, root: &Path, args: &[&str]) -> Output {
        Command::new("node")
            .arg(self.0.path().join("bin/rulecheck.mjs"))
            .args(args)
            .current_dir(root)
            .env_remove("RULECHECK_WRAPPER_ACTIVE")
            .output()
            .unwrap()
    }

    fn build(&self, root: &Path, args: &[&str]) -> Output {
        Command::new("node")
            .arg(self.0.path().join("tools/build.mjs"))
            .arg("--root")
            .arg(root)
            .args(args)
            .output()
            .unwrap()
    }
}

fn evaluator() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_rulecheck-evaluator"))
}

fn evaluate(args: &[&str], policy: &[u8], root: &Path) -> Output {
    let mut child = Command::new(evaluator())
        .current_dir(root)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let _ = child.stdin.take().unwrap().write_all(policy);
    child.wait_with_output().unwrap()
}

fn schema(name: &str) -> Value {
    let text = fs::read_to_string(Path::new(ROOT).join("schema").join(name)).unwrap();
    serde_json::from_str(&text).unwrap()
}

fn ids(findings: &Value) -> Vec<&str> {
    findings
        .as_array()
        .unwrap()
        .iter()
        .map(|finding| finding["ruleId"].as_str().unwrap())
        .collect()
}

/// Findings in published order: path, line, column, then rule ID.
fn assert_ordered(findings: &Value) {
    let key = |f: &Value| {
        (
            f["path"].as_str().unwrap().to_owned(),
            f["line"].as_u64().unwrap(),
            f["column"].as_u64().unwrap(),
            f["ruleId"].as_str().unwrap().to_owned(),
        )
    };
    let keys: Vec<_> = findings.as_array().unwrap().iter().map(key).collect();
    let mut sorted = keys.clone();
    sorted.sort();
    assert_eq!(keys, sorted);
}

#[test]
fn a_360_rule_policy_checks_the_same_in_the_working_tree_at_a_commit_and_in_ci() {
    let install = Install::new();
    let repo = repo(RULES);
    let root = repo.path();
    let clean = commit(root, "Synthetic 360-rule policy");

    // The builder emits rules in sorted file order, and stays well under 4 MiB.
    let started = Instant::now();
    let (policy, _) = run(install.build(root, &[]), 0);
    let built = started.elapsed();
    assert!(policy.len() < 1024 * 1024, "{}", policy.len());
    let parsed: Value = serde_json::from_str(&policy).unwrap();
    assert!(common::violations(&schema("policy-v1.json"), &parsed).is_empty());
    let built_ids: Vec<_> = parsed["rules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|rule| rule["id"].as_str().unwrap().to_owned())
        .collect();
    let mut by_path: Vec<_> = (0..RULES).collect();
    by_path.sort_by_key(|n| rule_path(*n));
    assert_eq!(
        built_ids,
        by_path.iter().map(|n| id(*n)).collect::<Vec<_>>()
    );
    let (exact_policy, _) = run(install.build(root, &["--commit", &clean]), 0);
    assert_eq!(exact_policy, policy);

    // The evaluator lists, validates and tests every rule in ID order.
    let mut sorted = built_ids.clone();
    sorted.sort();
    let (list, _) = run(install.rulecheck(root, &["list"]), 0);
    assert_eq!(list.lines().collect::<Vec<_>>(), sorted);
    let (exact_list, _) = run(install.rulecheck(root, &["list", "--commit", &clean]), 0);
    assert_eq!(exact_list, list);
    run(install.rulecheck(root, &["validate"]), 0);
    let (tested, _) = run(install.rulecheck(root, &["test"]), 0);
    assert!(
        tested.contains(&format!("{} fixtures passed", RULES * 2)),
        "{tested}"
    );
    let (prefix, _) = run(
        install.rulecheck(root, &["list", "--id-prefix", "scale/syn-"]),
        0,
    );
    assert_eq!(prefix.lines().count(), RULES / 4);

    let (clean_check, _) = run(install.rulecheck(root, &["check", "--format", "json"]), 0);
    let clean_check: Value = serde_json::from_str(&clean_check).unwrap();
    assert_eq!(clean_check["findings"], json!([]));

    // Every rule in the even packages fires once. Syntax rules share the
    // package's first comment; the rest have their own line or path.
    let expected = violate(root, RULES);
    assert_eq!(expected, RULES / 2);
    let candidate = commit(root, "Synthetic violations");
    let started = Instant::now();
    let (tree, _) = run(install.rulecheck(root, &["check", "--format", "json"]), 1);
    let checked = started.elapsed();
    let (exact, _) = run(
        install.rulecheck(root, &["check", "--commit", &candidate, "--format", "json"]),
        1,
    );
    let tree: Value = serde_json::from_str(&tree).unwrap();
    let exact: Value = serde_json::from_str(&exact).unwrap();
    assert!(common::violations(&schema("result-v1.json"), &tree).is_empty());
    assert_eq!(tree["findings"], exact["findings"]);
    assert_eq!(tree["policy"], exact["policy"]);
    assert_eq!(tree["findings"].as_array().unwrap().len(), expected);
    assert_ordered(&tree["findings"]);
    let mut fired = ids(&tree["findings"]);
    fired.sort();
    fired.dedup();
    assert_eq!(fired.len(), expected);
    let first = &tree["findings"][0];
    assert_eq!(
        (
            first["path"].as_str(),
            first["line"].as_u64(),
            first["column"].as_u64()
        ),
        (Some("pkg-00/forbidden/f10.txt"), Some(1), Some(1))
    );
    let syntax: Vec<_> = tree["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["ruleId"].as_str().unwrap().starts_with("scale/syn-"))
        .collect();
    let even_syntax = (0..RULES)
        .filter(|n| n % 4 == 3 && (n / PER_PACKAGE).is_multiple_of(2))
        .count();
    assert_eq!(syntax.len(), even_syntax);
    assert!(syntax.iter().all(|f| f["line"] == 1 && f["column"] == 1));

    // Human output is the same list, and a second run is byte-identical.
    let (human, _) = run(install.rulecheck(root, &["check"]), 1);
    assert_eq!(human.lines().count(), expected);
    let (again, _) = run(install.rulecheck(root, &["check"]), 1);
    assert_eq!(again, human);
    let (exact_human, _) = run(
        install.rulecheck(root, &["check", "--commit", &candidate]),
        1,
    );
    assert_eq!(exact_human, human);

    // Exact-commit CI evidence: the candidate adds a 361st rule; both policies
    // build from their commits and the diff names only the new rule.
    write(root, &rule_path(RULES), &rule_source(RULES));
    write(
        root,
        &format!("{}/src/a.ts", package(RULES)),
        "export const value = 1;\n",
    );
    let grown = commit(root, "Add a 361st rule");
    let output = Command::new("node")
        .current_dir(root)
        .arg(install.0.path().join("tools/check-ci.mjs"))
        .arg(root)
        .args([&candidate, &grown])
        .arg(install.0.path().join("target/release/rulecheck-evaluator"))
        .env("RULECHECK_TOOL_SHA", &candidate)
        .env_remove("RULECHECK_POLICY_REVIEWS")
        .output()
        .unwrap();
    let (evidence, _) = run(output, 1);
    let evidence: Value = serde_json::from_str(&evidence).unwrap();
    for phase in [
        "base-build",
        "base-validate",
        "proposed-build",
        "proposed-validate",
        "proposed-test",
    ] {
        assert_eq!(
            evidence["phases"][phase]["outcome"], "pass",
            "{phase}: {evidence}"
        );
    }
    for phase in ["base-check", "proposed-check"] {
        assert_eq!(evidence["phases"][phase]["outcome"], "violation", "{phase}");
    }
    let diff = evidence["effectivePolicyDiff"].to_string();
    assert!(diff.contains(&id(RULES)), "{diff}");
    assert!(!diff.contains(&id(0)), "{diff}");

    eprintln!(
        "360 rules (debug evaluator): build {} ms, check {} ms",
        built.as_millis(),
        checked.as_millis()
    );
}

#[test]
fn both_discovery_modes_accept_1024_rules_and_refuse_1025_with_one_message() {
    let install = Install::new();
    let repo = repo(LIMIT);
    let root = repo.path();
    let full = commit(root, "Synthetic policy at the rule limit");
    let (policy, _) = run(install.build(root, &[]), 0);
    let (exact, _) = run(install.build(root, &["--commit", &full]), 0);
    assert_eq!(policy, exact);
    let (list, _) = run(evaluate(&["list"], policy.as_bytes(), root), 0);
    assert_eq!(list.lines().count(), LIMIT);

    write(root, &rule_path(LIMIT), &rule_source(LIMIT));
    let over = commit(root, "One rule past the limit");
    let message = "rulecheck build: .rulecheck/rules: more than 1024 .rule.ts files; \
                   a policy holds at most 1024 rules\n";
    let (_, stderr) = run(install.build(root, &[]), 2);
    assert_eq!(stderr, message);
    let (_, stderr) = run(install.build(root, &["--commit", &over]), 2);
    assert_eq!(stderr, message);
    let (_, stderr) = run(install.rulecheck(root, &["check", "--commit", &over]), 2);
    assert!(stderr.starts_with(message), "{stderr}");

    // The evaluator holds the same line for policy JSON from another builder.
    let mut parsed: Value = serde_json::from_str(&policy).unwrap();
    let extra = json!({
        "id": "scale/extra", "kind": "forbidden-path", "paths": ["nowhere.txt"],
        "remediation": "Remove it.", "fixtures": [{ "path": "nowhere.txt", "expect": { "line": 1, "column": 1 } }]
    });
    parsed["rules"].as_array_mut().unwrap().push(extra);
    assert!(!common::violations(&schema("policy-v1.json"), &parsed).is_empty());
    let (_, stderr) = run(
        evaluate(&["validate"], parsed.to_string().as_bytes(), root),
        2,
    );
    assert!(
        stderr.contains("policy must have 1 to 1024 rules"),
        "{stderr}"
    );
}

#[test]
fn invalid_rules_and_settings_in_a_large_policy_fail_closed_with_the_rule_named() {
    let install = Install::new();
    let repo = repo(RULES);
    let root = repo.path();
    commit(root, "Synthetic 360-rule policy");

    // A builder error names the file and position, wherever it sorts.
    let broken = rule_path(301);
    write(
        root,
        &broken,
        &rule_source(301).replace("kind: \"text\"", "kind: \"txt\""),
    );
    let (_, stderr) = run(install.rulecheck(root, &["check"]), 2);
    assert!(
        stderr.starts_with(&format!("rulecheck build: {broken}:5:3: rule kind must be")),
        "{stderr}"
    );
    write(root, &broken, &rule_source(301));

    // Evaluator errors name the rule ID.
    write(root, &broken, &rule_source(301).replace("[0-9]+", "[0-9"));
    let (_, stderr) = run(install.rulecheck(root, &["validate"]), 2);
    assert!(
        stderr.contains(&format!("[{}] invalid regex", id(301))),
        "{stderr}"
    );
    write(root, &broken, &rule_source(301).replace(&id(301), &id(5)));
    let (_, stderr) = run(install.rulecheck(root, &["validate"]), 2);
    assert!(
        stderr.contains(&format!("[{}] duplicate rule ID", id(5))),
        "{stderr}"
    );
    write(root, &broken, &rule_source(301));
    run(install.rulecheck(root, &["validate"]), 0);

    // Invalid settings are refused before any rule runs.
    for args in [
        &["check", "--text-budget", "0"][..],
        &["check", "--text-budget", "1025"],
        &["check", "--id-prefix", "scale/none-"],
        &["check", "--all-matches"],
    ] {
        let (stdout, stderr) = run(install.rulecheck(root, args), 2);
        assert!(stdout.is_empty() && !stderr.is_empty(), "{args:?}");
    }
    let (_, stderr) = run(
        install.rulecheck(
            root,
            &["check", "--all-matches", "--id-prefix", "scale/lit-"],
        ),
        0,
    );
    assert!(stderr.is_empty(), "{stderr}");
}

#[test]
fn budgets_hold_across_many_rules_and_diagnostics_keep_policy_order() {
    let repo = repo(RULES);
    let root = repo.path();
    let install = Install::new();
    let (policy, _) = run(install.build(root, &[]), 0);

    // The scoped text budget counts each file once for all 270 rules that read
    // it. A 1 MiB budget stops in pkg-01; the unevaluated rules are listed in
    // ID order, one diagnostic each, before the budget message.
    let filler = "x".repeat(600 * 1024);
    for p in 0..RULES / PER_PACKAGE {
        write(
            root,
            &format!("pkg-{p:02}/src/big.ts"),
            &format!("let a = 1;\n/*{filler}*/\n"),
        );
    }
    git(root, &["add", "-A"]);
    let (result, _) = run(
        evaluate(
            &["check", "--text-budget", "1", "--format", "json"],
            policy.as_bytes(),
            root,
        ),
        2,
    );
    let result: Value = serde_json::from_str(&result).unwrap();
    assert_eq!(result["partial"]["stoppedAt"], "pkg-01/src/big.ts");
    let diagnostics = result["diagnostics"].as_array().unwrap();
    let named: Vec<_> = diagnostics
        .iter()
        .filter_map(|d| d["ruleId"].as_str())
        .collect();
    let mut sorted = named.clone();
    sorted.sort();
    assert_eq!(named, sorted);
    // Every rule outside pkg-00 that reads text. Forbidden-path rules match
    // no file here, so the stop does not leave them unevaluated.
    let readers = (PER_PACKAGE..RULES).filter(|n| n % 4 != 2).count();
    assert_eq!(named.len(), readers);
    assert!(diagnostics.last().unwrap()["ruleId"].is_null());
    for p in 0..RULES / PER_PACKAGE {
        fs::remove_file(root.join(format!("pkg-{p:02}/src/big.ts"))).unwrap();
    }

    // The 10,000-finding limit holds for the whole policy, not per rule: each
    // marked file gets one finding from every text and syntax rule of its
    // package, 270 per file index across the 12 packages, 10,260 for 38.
    for p in 0..RULES / PER_PACKAGE {
        let mut marked = String::from("// note\n");
        for n in p * PER_PACKAGE..(p + 1) * PER_PACKAGE {
            marked.push_str(&format!("LIT-{n}- RE-{n}-1\n"));
        }
        for f in 0..38 {
            write(root, &format!("pkg-{p:02}/src/m{f:02}.ts"), &marked);
        }
    }
    git(root, &["add", "-A"]);
    let (stdout, stderr) = run(evaluate(&["check"], policy.as_bytes(), root), 2);
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("more than 10,000 findings"), "{stderr}");
}
