//! Pins count-only baselines: `baseline` records the all-match counts of a
//! complete scan per rule ID and path, and `check --baseline` fails a rule and
//! path above its allowance, while `--strict-baseline` also fails one below it.
//! Covers same-file additions, the clean-up ratchet, renames, removed rules,
//! changed match semantics, invalid input, incomplete scans, scoped
//! regeneration, working-tree and exact-commit parity, and the exact-commit CI
//! phases that keep a pull request from raising its own allowance.
mod common;

use serde_json::{Value, json};
use std::fs;
use std::io::Write;
use std::os::unix::fs::symlink;
use std::path::Path;
use std::process::{Command, Output, Stdio};

const ROOT: &str = env!("CARGO_MANIFEST_DIR");
const BASELINE: &str = ".rulecheck/baseline.json";

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
            "Rulecheck baseline fixture",
        ],
    );
    git(root, &["rev-parse", "HEAD"])
}

fn write(root: &Path, path: &str, content: &str) {
    let path = root.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

fn read(root: &Path, path: &str) -> String {
    fs::read_to_string(root.join(path)).unwrap()
}

fn repo() -> tempfile::TempDir {
    let repo = tempfile::tempdir().unwrap();
    git(repo.path(), &["init", "-q"]);
    // Where the baseline is kept; `baseline --write` does not create directories.
    fs::create_dir(repo.path().join(".rulecheck")).unwrap();
    repo
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

fn marker_rule(id: &str, value: &str, paths: &[&str]) -> Value {
    json!({
        "id": id, "kind": "text", "paths": paths,
        "pattern": { "kind": "literal", "value": value },
        "remediation": "Remove the marker.",
        "fixtures": [{ "path": "src/x.ts", "content": "", "expect": null }]
    })
}

fn todo() -> Vec<u8> {
    policy(&[marker_rule("source/no-todo", "TODO", &["src/**"])])
}

/// A synthetic repository with existing findings: two in `src/a.ts`, one in
/// `src/b.ts`, and a clean `src/c.ts`.
fn legacy() -> tempfile::TempDir {
    let repo = repo();
    let root = repo.path();
    write(root, "src/a.ts", "// TODO one\nlet a = 1; // TODO two\n");
    write(root, "src/b.ts", "// TODO three\n");
    write(root, "src/c.ts", "let c = 3;\n");
    git(root, &["add", "-A"]);
    repo
}

fn record(root: &Path, policy: &[u8], extra: &[&str]) -> String {
    let mut args = vec!["baseline", "--baseline", BASELINE, "--write"];
    args.extend_from_slice(extra);
    run(cli(&args, policy, root), 0);
    read(root, BASELINE)
}

fn json_check(args: &[&str], policy: &[u8], root: &Path, code: i32) -> Value {
    let (stdout, _) = run(cli(args, policy, root), code);
    let result: Value = serde_json::from_str(&stdout).unwrap();
    let errors = common::violations(&schema("result-v1.json"), &result);
    assert!(errors.is_empty(), "{errors:?}\n{result:#}");
    result
}

fn baseline_json(root: &Path) -> Value {
    serde_json::from_str(&read(root, BASELINE)).unwrap()
}

#[test]
fn records_a_canonical_versioned_baseline_of_all_match_counts() {
    let repo = legacy();
    let root = repo.path();
    let text = record(root, &todo(), &[]);
    let value = baseline_json(root);
    assert!(common::violations(&schema("baseline-v1.json"), &value).is_empty());
    assert_eq!(value["baselineVersion"], 1);
    assert_eq!(value["matches"], "all");
    let entry = &value["rules"]["source/no-todo"];
    assert_eq!(entry["paths"], json!({ "src/a.ts": 2, "src/b.ts": 1 }));
    assert!(entry["matcher"].as_str().unwrap().starts_with("sha256:"));
    // Sorted keys, two-space indent and one trailing newline.
    assert!(text.starts_with("{\n  \"baselineVersion\": 1,\n  \"matches\": \"all\",\n"));
    assert!(text.ends_with("}\n") && !text.ends_with("\n\n"));

    // The same counts give the same bytes, from stdout or from the commit.
    let (stdout, stderr) = run(cli(&["baseline"], &todo(), root), 0);
    assert_eq!(stdout, text);
    assert_eq!(
        stderr,
        "rulecheck: the baseline records 3 findings in 2 paths for 1 rules\n"
    );
    let sha = commit(root);
    let (exact, _) = run(cli(&["baseline", "--commit", &sha], &todo(), root), 0);
    assert_eq!(exact, text);

    // The check-only path passes the existing findings and lists none.
    let (stdout, stderr) = run(cli(&["check", "--baseline", BASELINE], &todo(), root), 0);
    assert_eq!((stdout.as_str(), stderr.as_str()), ("", ""));
    let result = json_check(
        &["check", "--baseline", BASELINE, "--format", "json"],
        &todo(),
        root,
        0,
    );
    assert_eq!(result["findings"], json!([]));
    assert_eq!(result["source"]["matches"], "all");
    assert!(result["source"]["baselineSha256"].is_string());
    assert!(result["source"].get("strictBaseline").is_none());
    assert_eq!(
        result["baseline"],
        json!({ "strict": false, "allowedFindings": 3, "exceeded": [], "loose": [], "staleRuleIds": [] })
    );
    // Without --baseline the check still fails on every file with a finding.
    run(cli(&["check"], &todo(), root), 1);
}

#[test]
fn a_same_file_addition_or_a_new_path_fails_with_every_match_listed() {
    let repo = legacy();
    let root = repo.path();
    record(root, &todo(), &[]);
    write(
        root,
        "src/a.ts",
        "// TODO one\nlet a = 1; // TODO two\n// TODO added\n",
    );
    write(root, "src/c.ts", "let c = 3; // TODO new path\n");
    let (stdout, _) = run(cli(&["check", "--baseline", BASELINE], &todo(), root), 1);
    // A count cannot tell which match is new, so the whole file is listed.
    assert_eq!(
        stdout,
        "src/a.ts:1:4: [source/no-todo] Remove the marker.\n\
         src/a.ts:2:15: [source/no-todo] Remove the marker.\n\
         src/a.ts:3:4: [source/no-todo] Remove the marker.\n\
         src/c.ts:1:15: [source/no-todo] Remove the marker.\n\
         src/a.ts: [source/no-todo] 3 findings, baseline allows 2\n\
         src/c.ts: [source/no-todo] 1 finding, baseline allows 0\n"
    );
    let result = json_check(
        &["check", "--baseline", BASELINE, "--format", "json"],
        &todo(),
        root,
        1,
    );
    assert_eq!(result["findings"].as_array().unwrap().len(), 4);
    assert_eq!(result["baseline"]["allowedFindings"], 1);
    assert_eq!(
        result["baseline"]["exceeded"],
        json!([
            { "ruleId": "source/no-todo", "path": "src/a.ts", "count": 3, "allowed": 2 },
            { "ruleId": "source/no-todo", "path": "src/c.ts", "count": 1, "allowed": 0 },
        ])
    );
}

#[test]
fn equal_count_replacements_in_one_file_are_not_detected() {
    let repo = legacy();
    let root = repo.path();
    record(root, &todo(), &[]);
    // One marker removed and another added elsewhere in the same file keeps
    // the count at 2: a count-only baseline accepts it.
    write(
        root,
        "src/a.ts",
        "let a = 1; // TODO two\n// TODO replaced\n",
    );
    run(
        cli(
            &["check", "--baseline", BASELINE, "--strict-baseline"],
            &todo(),
            root,
        ),
        0,
    );
}

#[test]
fn a_clean_up_ratchets_only_with_a_tightened_baseline_under_strict() {
    let repo = legacy();
    let root = repo.path();
    let before = record(root, &todo(), &[]);
    write(root, "src/a.ts", "// TODO one\nlet a = 1;\n");
    write(root, "src/b.ts", "// done\n");

    // The normal check passes and notes that the baseline can be tightened.
    let (stdout, stderr) = run(cli(&["check", "--baseline", BASELINE], &todo(), root), 0);
    assert_eq!(stdout, "");
    assert_eq!(
        stderr,
        "rulecheck: note: src/a.ts: [source/no-todo] 1 finding, baseline allows 2; tighten the \
         baseline: regenerate it with rulecheck baseline\n\
         rulecheck: note: src/b.ts: [source/no-todo] 0 findings, baseline allows 1; tighten the \
         baseline: regenerate it with rulecheck baseline\n"
    );
    // Strict CI fails until the lower counts are recorded.
    let args = ["check", "--baseline", BASELINE, "--strict-baseline"];
    let (stdout, _) = run(cli(&args, &todo(), root), 1);
    assert!(stdout.starts_with("src/a.ts: [source/no-todo] 1 finding, baseline allows 2;"));
    let mut json_args = args.to_vec();
    json_args.extend(["--format", "json"]);
    let result = json_check(&json_args, &todo(), root, 1);
    assert_eq!(result["findings"], json!([]));
    assert_eq!(result["source"]["strictBaseline"], true);
    assert_eq!(result["baseline"]["loose"].as_array().unwrap().len(), 2);

    let after = record(root, &todo(), &[]);
    assert_ne!(before, after);
    assert_eq!(
        baseline_json(root)["rules"]["source/no-todo"]["paths"],
        json!({ "src/a.ts": 1 })
    );
    run(cli(&args, &todo(), root), 0);

    // With every finding fixed, the rule leaves the baseline entirely.
    write(root, "src/a.ts", "let a = 1;\n");
    record(root, &todo(), &[]);
    assert_eq!(baseline_json(root)["rules"], json!({}));
    run(cli(&args, &todo(), root), 0);
}

#[test]
fn renames_removed_rules_and_changed_semantics_give_no_new_allowance() {
    let repo = legacy();
    let root = repo.path();
    commit(root);
    record(root, &todo(), &[]);

    // A rename is a new path with no allowance plus a stale one.
    git(root, &["mv", "src/b.ts", "src/moved.ts"]);
    let result = json_check(
        &["check", "--baseline", BASELINE, "--format", "json"],
        &todo(),
        root,
        1,
    );
    assert_eq!(
        result["baseline"]["exceeded"],
        json!([{ "ruleId": "source/no-todo", "path": "src/moved.ts", "count": 1, "allowed": 0 }])
    );
    assert_eq!(
        result["baseline"]["loose"],
        json!([{ "ruleId": "source/no-todo", "path": "src/b.ts", "count": 0, "allowed": 1 }])
    );
    git(root, &["mv", "src/moved.ts", "src/b.ts"]);

    // Renaming the rule ID drops its allowance and leaves a stale entry.
    let renamed = policy(&[marker_rule("source/no-todo-marker", "TODO", &["src/**"])]);
    let (stdout, stderr) = run(cli(&["check", "--baseline", BASELINE], &renamed, root), 1);
    assert!(stdout.contains("src/a.ts: [source/no-todo-marker] 2 findings, baseline allows 0"));
    assert!(stderr.contains(
        "rulecheck: note: [source/no-todo] the baseline lists a rule that is not in the policy"
    ));
    // A removed rule's entry is stale: a note normally, a failure under strict.
    let other = policy(&[marker_rule("source/no-fixme", "FIXME", &["src/**"])]);
    run(cli(&["check", "--baseline", BASELINE], &other, root), 0);
    let result = json_check(
        &[
            "check",
            "--baseline",
            BASELINE,
            "--strict-baseline",
            "--format",
            "json",
        ],
        &other,
        root,
        1,
    );
    assert_eq!(
        result["baseline"]["staleRuleIds"],
        json!(["source/no-todo"])
    );
    // --id-prefix leaves other rules' entries out of the comparison.
    let both = policy(&[
        marker_rule("source/no-fixme", "FIXME", &["src/**"]),
        marker_rule("source/no-todo", "TODO", &["src/**"]),
    ]);
    let prefix = [
        "check",
        "--baseline",
        BASELINE,
        "--strict-baseline",
        "--id-prefix",
    ];
    run(
        cli(&[&prefix[..], &["source/no-fixme"]].concat(), &both, root),
        0,
    );

    // Scope, remediation and fixtures do not change what matches inside a
    // file, so the counts still apply; a wider scope has no allowance.
    let mut rescoped = marker_rule("source/no-todo", "TODO", &["src/**", "lib/**"]);
    rescoped["exclude"] = json!(["src/c.ts"]);
    rescoped["remediation"] = json!("Track it in the issue tracker.");
    rescoped["fixtures"] =
        json!([{ "path": "src/y.ts", "content": "TODO", "expect": { "line": 1, "column": 1 } }]);
    run(
        cli(
            &["check", "--baseline", BASELINE],
            &policy(&[rescoped]),
            root,
        ),
        0,
    );

    // A changed pattern or kind cannot reuse counts recorded for the old one.
    for changed in [
        marker_rule("source/no-todo", "TODO ", &["src/**"]),
        json!({
            "id": "source/no-todo", "kind": "text", "paths": ["src/**"],
            "pattern": { "kind": "regex", "value": "TODO" },
            "remediation": "Remove the marker.",
            "fixtures": [{ "path": "src/x.ts", "content": "", "expect": null }]
        }),
    ] {
        let (_, stderr) = run(
            cli(
                &["check", "--baseline", BASELINE],
                &policy(&[changed]),
                root,
            ),
            2,
        );
        assert!(
            stderr.contains("[source/no-todo] the baseline was recorded for other match semantics"),
            "{stderr}"
        );
        assert!(stderr.contains("rulecheck baseline --id-prefix source/no-todo"));
    }
}

#[test]
fn invalid_missing_or_incompatible_baselines_fail_closed() {
    let repo = legacy();
    let root = repo.path();
    let good = record(root, &todo(), &[]);
    let value: Value = serde_json::from_str(&good).unwrap();
    let pretty = |value: &Value| format!("{}\n", serde_json::to_string_pretty(value).unwrap());
    let edit = |change: &dyn Fn(&mut Value)| {
        let mut value = value.clone();
        change(&mut value);
        pretty(&value)
    };
    let entry = "/rules/source~1no-todo";
    let cases: Vec<(String, &str)> = vec![
        ("not json".into(), "invalid baseline JSON"),
        ("".into(), "invalid baseline JSON"),
        (edit(&|v| v["extra"] = json!(1)), "unknown field `extra`"),
        (
            edit(&|v| {
                v.as_object_mut().unwrap().remove("matches");
            }),
            "missing field `matches`",
        ),
        (
            edit(&|v| {
                v.pointer_mut(entry)
                    .unwrap()
                    .as_object_mut()
                    .unwrap()
                    .remove("matcher");
            }),
            "missing field `matcher`",
        ),
        (
            edit(&|v| v["baselineVersion"] = json!(2)),
            "unsupported baselineVersion 2",
        ),
        (
            edit(&|v| v["matches"] = json!("first")),
            "unsupported baseline matches \"first\"",
        ),
        (
            edit(&|v| v.pointer_mut(entry).unwrap()["matcher"] = json!("sha256:abc")),
            "baseline matcher must be sha256:",
        ),
        (
            edit(&|v| v.pointer_mut(entry).unwrap()["paths"] = json!({})),
            "baseline lists no paths",
        ),
        (
            edit(&|v| v.pointer_mut(entry).unwrap()["paths"]["src/a.ts"] = json!(0)),
            "baseline count must be 1 to 10000",
        ),
        (
            edit(&|v| v.pointer_mut(entry).unwrap()["paths"]["src/a.ts"] = json!(-1)),
            "invalid baseline JSON",
        ),
        (
            edit(&|v| v.pointer_mut(entry).unwrap()["paths"]["./src/z.ts"] = json!(1)),
            "invalid baseline path \"./src/z.ts\"",
        ),
        (
            edit(&|v| v.pointer_mut(entry).unwrap()["paths"]["src/../z.ts"] = json!(1)),
            "invalid baseline path",
        ),
        (
            edit(&|v| v.pointer_mut(entry).unwrap()["paths"]["src\\z.ts"] = json!(1)),
            "invalid baseline path",
        ),
        (
            edit(&|v| v["rules"]["Bad ID"] = v.pointer(entry).unwrap().clone()),
            "invalid baseline rule ID",
        ),
        (
            serde_json::to_string(&value).unwrap(),
            "not in canonical form",
        ),
        (good.trim_end().to_owned(), "not in canonical form"),
        (
            good.replacen(
                "\"src/a.ts\": 2",
                "\"src/a.ts\": 9,\n        \"src/a.ts\": 2",
                1,
            ),
            "not in canonical form",
        ),
    ];
    for (bytes, message) in cases {
        fs::write(root.join(BASELINE), &bytes).unwrap();
        let (stdout, stderr) = run(cli(&["check", "--baseline", BASELINE], &todo(), root), 2);
        assert_eq!(stdout, "");
        assert!(stderr.contains(message), "{bytes:?}: {stderr}");
        let result = json_check(
            &["check", "--baseline", BASELINE, "--format", "json"],
            &todo(),
            root,
            2,
        );
        assert_eq!(result["findings"], json!([]));
        assert!(result.get("baseline").is_none());
        assert!(
            result["diagnostics"][0]["message"]
                .as_str()
                .unwrap()
                .contains(message)
        );
        // Scoped regeneration keeps other entries, so it needs a valid file too.
        let (_, stderr) = run(
            cli(
                &["baseline", "--baseline", BASELINE, "--id-prefix", "source/"],
                &todo(),
                root,
            ),
            2,
        );
        assert!(
            stderr.contains("scoped regeneration keeps the other rules' entries"),
            "{stderr}"
        );
    }

    // A missing file or a directory cannot be judged; the result records no digest.
    fs::remove_file(root.join(BASELINE)).unwrap();
    let result = json_check(
        &["check", "--baseline", BASELINE, "--format", "json"],
        &todo(),
        root,
        2,
    );
    assert!(result["source"]["baselineSha256"].is_null());
    fs::create_dir(root.join(BASELINE)).unwrap();
    let (_, stderr) = run(cli(&["check", "--baseline", BASELINE], &todo(), root), 2);
    assert!(
        stderr.contains("baseline .rulecheck/baseline.json:"),
        "{stderr}"
    );
    fs::remove_dir(root.join(BASELINE)).unwrap();

    // Flags that need a baseline file, or belong to another command.
    for (args, message) in [
        (
            &["check", "--strict-baseline"][..],
            "--strict-baseline requires --baseline FILE",
        ),
        (&["baseline", "--write"], "--write requires --baseline FILE"),
        (&["check", "--baseline"], "--baseline requires a file"),
        (&["check", "--baseline", ""], "--baseline requires a file"),
        (
            &["check", "--baseline", BASELINE, "--write"],
            "unexpected argument \"--write\"",
        ),
        (
            &["baseline", "--strict-baseline"],
            "unexpected argument \"--strict-baseline\"",
        ),
        (
            &["baseline", "--format", "json"],
            "unexpected argument \"--format\"",
        ),
        (
            &["baseline", "--all-matches"],
            "unexpected argument \"--all-matches\"",
        ),
        (
            &["test", "--baseline", BASELINE],
            "unexpected argument \"--baseline\"",
        ),
    ] {
        let (_, stderr) = run(cli(args, &todo(), root), 2);
        assert!(stderr.contains(message), "{args:?}: {stderr}");
    }
}

#[test]
fn incomplete_scans_record_nothing_and_are_never_compared() {
    let repo = repo();
    let root = repo.path();
    let half = format!("TODO\n{}", "x".repeat(600 * 1024));
    write(root, "src/a.ts", &half);
    write(root, "src/b.ts", &half);
    git(root, &["add", "-A"]);
    let saved = record(root, &todo(), &[]);

    // A budget stop: regeneration exits 2 and leaves the file as it was.
    let budget = ["--text-budget", "1"];
    let (stdout, stderr) = run(
        cli(
            &[
                &["baseline", "--baseline", BASELINE, "--write"][..],
                &budget,
            ]
            .concat(),
            &todo(),
            root,
        ),
        2,
    );
    assert_eq!(stdout, "");
    assert!(stderr.contains("[source/no-todo] not evaluated on every file in scope"));
    assert!(
        stderr.contains("cannot record a baseline from a partial scan, so nothing was written")
    );
    assert_eq!(read(root, BASELINE), saved);
    // A partial check is an error with its partial findings, and no comparison.
    let result = json_check(
        &[
            &["check", "--baseline", BASELINE, "--format", "json"][..],
            &budget,
        ]
        .concat(),
        &todo(),
        root,
        2,
    );
    assert_eq!(result["partial"]["stoppedAt"], "src/b.ts");
    assert!(result.get("baseline").is_none());

    // The findings cap applies to every match, and records nothing either.
    write(root, "src/a.ts", &"TODO".repeat(10_001));
    for args in [
        &["baseline", "--baseline", BASELINE, "--write"][..],
        &["check", "--baseline", BASELINE],
    ] {
        let (_, stderr) = run(cli(args, &todo(), root), 2);
        assert!(stderr.contains("more than 10,000 findings"), "{stderr}");
    }
    assert_eq!(read(root, BASELINE), saved);

    // An empty text scope cannot be recorded.
    let empty = policy(&[marker_rule("docs/no-todo", "TODO", &["docs/**"])]);
    let (_, stderr) = run(cli(&["baseline"], &empty, root), 2);
    assert!(stderr.contains("[docs/no-todo] empty text scope"));
    assert!(stderr.contains("a baseline is recorded only from a complete scan"));
}

#[test]
fn scoped_regeneration_replaces_only_the_selected_rules() {
    let repo = legacy();
    let root = repo.path();
    write(root, "src/d.ts", "// FIXME four\n");
    git(root, &["add", "-A"]);
    let rules = policy(&[
        marker_rule("source/no-fixme", "FIXME", &["src/**"]),
        marker_rule("source/no-todo", "TODO", &["src/**"]),
    ]);
    record(root, &rules, &[]);
    write(root, "src/a.ts", "// TODO one\n");
    write(root, "src/d.ts", "// FIXME four\n// FIXME five\n");

    // Without --write the regenerated baseline goes to stdout only.
    let saved = read(root, BASELINE);
    let (stdout, _) = run(
        cli(
            &[
                "baseline",
                "--baseline",
                BASELINE,
                "--id-prefix",
                "source/no-todo",
            ],
            &rules,
            root,
        ),
        0,
    );
    assert_eq!(read(root, BASELINE), saved);
    record(root, &rules, &["--id-prefix", "source/no-todo"]);
    assert_eq!(read(root, BASELINE), stdout);
    let value = baseline_json(root);
    assert_eq!(
        value["rules"]["source/no-todo"]["paths"],
        json!({ "src/a.ts": 1, "src/b.ts": 1 })
    );
    // The other rule's entry is kept as reviewed, so its new finding still fails.
    assert_eq!(
        value["rules"]["source/no-fixme"]["paths"],
        json!({ "src/d.ts": 1 })
    );
    let (stdout, _) = run(cli(&["check", "--baseline", BASELINE], &rules, root), 1);
    assert!(stdout.ends_with("src/d.ts: [source/no-fixme] 2 findings, baseline allows 1\n"));

    // Scoped regeneration drops entries of removed rules under its prefix only.
    let mut value = baseline_json(root);
    value["rules"]["source/old"] = value["rules"]["source/no-fixme"].clone();
    value["rules"]["legacy/old"] = value["rules"]["source/no-fixme"].clone();
    fs::write(
        root.join(BASELINE),
        format!("{}\n", serde_json::to_string_pretty(&value).unwrap()),
    )
    .unwrap();
    record(root, &rules, &["--id-prefix", "source/"]);
    let ids: Vec<_> = baseline_json(root)["rules"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    assert_eq!(ids, ["legacy/old", "source/no-fixme", "source/no-todo"]);
    // A full regeneration rebuilds the file from the scan alone, even over an invalid one.
    fs::write(root.join(BASELINE), "not json").unwrap();
    record(root, &rules, &[]);
    let ids: Vec<_> = baseline_json(root)["rules"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    assert_eq!(ids, ["source/no-fixme", "source/no-todo"]);
    // A missing file starts empty.
    fs::remove_file(root.join(BASELINE)).unwrap();
    record(root, &rules, &["--id-prefix", "source/no-todo"]);
    let ids: Vec<_> = baseline_json(root)["rules"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    assert_eq!(ids, ["source/no-todo"]);
}

#[test]
fn syntax_rules_are_never_recorded_and_get_no_allowance() {
    let repo = repo();
    let root = repo.path();
    write(root, "src/a.ts", "let a = 1; // TODO\n");
    write(root, "web/b.ts", "let b = 2;\n");
    git(root, &["add", "-A"]);
    let syntax = |paths: &[&str]| {
        json!({
            "id": "source/no-comments", "kind": "syntax", "paths": paths,
            "selector": { "language": "typescript", "node": "comment" },
            "remediation": "Remove the comment.",
            "fixtures": [{ "path": "src/x.ts", "content": "// c\n", "expect": { "line": 1, "column": 1 } }]
        })
    };
    let rules = policy(&[
        syntax(&["src/**"]),
        marker_rule("source/no-todo", "TODO", &["src/**"]),
    ]);
    let (_, stderr) = run(cli(&["baseline"], &rules, root), 2);
    assert!(
        stderr.contains(
            "[source/no-comments] src/a.ts: syntax rules cannot be recorded in a baseline"
        )
    );

    // A clean syntax rule is fine: it has no entry and no allowance.
    let clean = policy(&[
        syntax(&["web/**"]),
        marker_rule("source/no-todo", "TODO", &["src/**"]),
    ]);
    record(root, &clean, &[]);
    run(cli(&["check", "--baseline", BASELINE], &clean, root), 0);
    write(root, "web/b.ts", "let b = 2; // new\n");
    let (stdout, _) = run(cli(&["check", "--baseline", BASELINE], &clean, root), 1);
    assert!(stdout.contains("web/b.ts: [source/no-comments] 1 finding, baseline allows 0"));
    // --all-matches alone still refuses syntax rules.
    run(cli(&["check", "--all-matches"], &clean, root), 2);

    // A hand-made entry for a syntax rule cannot be judged.
    let mut value = baseline_json(root);
    value["rules"]["source/no-comments"] = value["rules"]["source/no-todo"].clone();
    fs::write(
        root.join(BASELINE),
        format!("{}\n", serde_json::to_string_pretty(&value).unwrap()),
    )
    .unwrap();
    let (_, stderr) = run(cli(&["check", "--baseline", BASELINE], &clean, root), 2);
    assert!(stderr.contains("[source/no-comments] the baseline lists counts for a syntax rule"));
}

#[test]
fn working_tree_and_exact_commit_checks_agree() {
    let repo = legacy();
    let root = repo.path();
    record(root, &todo(), &[]);
    write(
        root,
        "src/a.ts",
        "// TODO one\n// TODO two\n// TODO three\n",
    );
    write(root, "src/b.ts", "clean\n");
    let sha = commit(root);
    for strict in [false, true] {
        let mut args = vec!["check", "--baseline", BASELINE, "--format", "json"];
        if strict {
            args.push("--strict-baseline");
        }
        let tree = json_check(&args, &todo(), root, 1);
        args.extend(["--commit", &sha]);
        let exact = json_check(&args, &todo(), root, 1);
        for key in ["findings", "baseline", "diagnostics", "outcome", "policy"] {
            assert_eq!(tree[key], exact[key], "{key}");
        }
        assert_eq!(
            tree["source"]["baselineSha256"],
            exact["source"]["baselineSha256"]
        );
        assert_eq!(exact["source"]["commit"], sha);
    }
    // Human output matches too.
    let tree = run(cli(&["check", "--baseline", BASELINE], &todo(), root), 1);
    let exact = run(
        cli(
            &["check", "--baseline", BASELINE, "--commit", &sha],
            &todo(),
            root,
        ),
        1,
    );
    assert_eq!(tree, exact);
}

const RULE: &str = r#"import { defineRule } from "@rulecheck/sdk";

export default defineRule({
  id: "source/no-marker",
  kind: "text",
  paths: ["src/**"],
  pattern: { kind: "literal", value: "MARKER" },
  remediation: "Remove the marker.",
  fixtures: [{ path: "src/x.ts", content: "MARKER\n", expect: { line: 1, column: 1 } }],
});
"#;

fn built_policy(root: &Path) -> Vec<u8> {
    let output = Command::new("node")
        .arg(Path::new(ROOT).join("tools/build.mjs"))
        .arg("--root")
        .arg(root)
        .output()
        .expect("run build; install Node.js and run npm ci");
    let (stdout, _) = run(output, 0);
    stdout.into_bytes()
}

/// Runs exact-commit CI. `reviews` is the owner reviews file, if any.
fn check_ci(root: &Path, base: &str, candidate: &str, code: i32, reviews: Option<&Path>) -> Value {
    let mut command = Command::new("node");
    command
        .current_dir(root)
        .arg(Path::new(ROOT).join("tools/check-ci.mjs"))
        .arg(root)
        .args([base, candidate, env!("CARGO_BIN_EXE_rulecheck-evaluator")])
        .env("RULECHECK_TOOL_SHA", base)
        .env_remove("RULECHECK_POLICY_REVIEWS");
    if let Some(reviews) = reviews {
        command.env("RULECHECK_POLICY_REVIEWS", reviews);
    }
    let (stdout, _) = run(command.output().unwrap(), code);
    serde_json::from_str(&stdout).unwrap()
}

fn outcome(report: &Value, phase: &str) -> String {
    report["phases"][phase]["outcome"]
        .as_str()
        .unwrap_or("missing")
        .to_owned()
}

/// A repository whose base commit adopts the rule with a reviewed baseline of
/// two markers in `src/a.ts`.
fn adopted() -> (tempfile::TempDir, String) {
    let repo = repo();
    let root = repo.path();
    write(root, ".rulecheck/rules/marker.rule.ts", RULE);
    write(root, "src/a.ts", "MARKER\nMARKER\n");
    write(root, "src/b.ts", "clean\n");
    git(root, &["add", "-A"]);
    let policy = built_policy(root);
    record(root, &policy, &[]);
    let base = commit(root);
    (repo, base)
}

fn regenerate(root: &Path) {
    let policy = built_policy(root);
    record(root, &policy, &[]);
}

#[test]
fn exact_ci_judges_the_candidate_against_the_base_baseline() {
    let (repo, base) = adopted();
    let root = repo.path();

    let clean = check_ci(root, &base, &base, 0, None);
    let check = &clean["phases"]["base-check"];
    assert_eq!(check["baseline"]["source"], "base");
    assert_eq!(check["baseline"]["sha"], base);
    assert_eq!(check["baseline"]["strict"], false);
    assert_eq!(
        clean["phases"]["proposed-check"]["baseline"]["strict"],
        true
    );
    assert_eq!(clean["baselineChange"]["raised"], json!([]));

    // A same-file addition fails both phases.
    write(root, "src/a.ts", "MARKER\nMARKER\nMARKER\n");
    let added = commit(root);
    let report = check_ci(root, &base, &added, 1, None);
    assert_eq!(outcome(&report, "base-check"), "violation");
    assert_eq!(outcome(&report, "proposed-check"), "violation");
    assert!(
        report["phases"]["base-check"]["stdout"]
            .as_str()
            .unwrap()
            .contains("src/a.ts: [source/no-marker] 3 findings, baseline allows 2")
    );

    // Inflation: the same pull request raises its own allowance. The proposed
    // check accepts it, but base-check still uses the base commit's baseline.
    regenerate(root);
    let inflated = commit(root);
    let report = check_ci(root, &base, &inflated, 1, None);
    assert_eq!(outcome(&report, "base-check"), "violation");
    assert_eq!(outcome(&report, "proposed-check"), "pass");
    assert_eq!(report["baselineChange"]["rebaseline"], false);
    assert_eq!(
        report["baselineChange"]["raised"],
        json!([{ "ruleId": "source/no-marker", "path": "src/a.ts", "before": 2, "after": 3, "matcherChanged": false }])
    );
    // The baseline is policy-owned, so its change also needs owner review.
    let reviews = tempfile::NamedTempFile::new().unwrap();
    fs::write(
        reviews.path(),
        json!({"owner": "pmdroid", "state": "APPROVED", "sha": inflated}).to_string(),
    )
    .unwrap();
    let report = check_ci(root, &base, &inflated, 1, Some(reviews.path()));
    assert_eq!(report["phases"]["proposed-owner-review"]["required"], true);
    assert_eq!(outcome(&report, "base-check"), "violation");

    // Inflating the file alone, without new findings, is a baseline-only
    // rebaseline: it needs owner review, and strict still fails the padding.
    git(root, &["checkout", "-q", &base, "--", "src"]);
    let inflated_only = commit(root);
    let report = check_ci(root, &base, &inflated_only, 2, None);
    assert_eq!(report["baselineChange"]["rebaseline"], true);
    assert_eq!(outcome(&report, "proposed-owner-review"), "error");
    assert_eq!(outcome(&report, "proposed-check"), "violation");
    assert!(
        report["phases"]["proposed-check"]["stdout"]
            .as_str()
            .unwrap()
            .contains("src/a.ts: [source/no-marker] 2 findings, baseline allows 3; tighten")
    );
}

#[test]
fn exact_ci_requires_a_tightened_baseline_after_a_clean_up() {
    let (repo, base) = adopted();
    let root = repo.path();
    write(root, "src/a.ts", "MARKER\n");
    let untightened = commit(root);
    let report = check_ci(root, &base, &untightened, 1, None);
    assert_eq!(outcome(&report, "base-check"), "pass");
    assert_eq!(outcome(&report, "proposed-check"), "violation");
    assert!(
        report["phases"]["base-check"]["stderr"]
            .as_str()
            .unwrap()
            .contains("rulecheck: note:")
    );

    regenerate(root);
    let tightened = commit(root);
    let report = check_ci(root, &base, &tightened, 0, None);
    assert_eq!(outcome(&report, "base-check"), "pass");
    assert_eq!(outcome(&report, "proposed-check"), "pass");
    assert_eq!(
        report["baselineChange"]["lowered"],
        json!([{ "ruleId": "source/no-marker", "path": "src/a.ts", "before": 2, "after": 1, "matcherChanged": false }])
    );
}

#[test]
fn exact_ci_accepts_a_raise_only_from_a_reviewed_baseline_only_pull_request() {
    let (repo, _) = adopted();
    let root = repo.path();
    // A change landed without updating the baseline, so the base is over it.
    write(root, "src/a.ts", "MARKER\nMARKER\nMARKER\n");
    let red = commit(root);
    regenerate(root);
    let rebaseline = commit(root);

    // Baseline-only: base-check uses the proposed baseline, which strict
    // proposed-check pins to the counts the base already has. It needs an
    // exact-head owner approval, so without the reviews file it cannot pass.
    let report = check_ci(root, &red, &rebaseline, 2, None);
    assert_eq!(
        report["phases"]["base-check"]["baseline"]["source"],
        "proposed"
    );
    assert_eq!(outcome(&report, "base-check"), "pass");
    assert_eq!(outcome(&report, "proposed-check"), "pass");
    assert_eq!(outcome(&report, "proposed-owner-review"), "error");
    assert_eq!(report["baselineChange"]["rebaseline"], true);
    let reviews = tempfile::NamedTempFile::new().unwrap();
    fs::write(
        reviews.path(),
        json!({"owner": "pmdroid", "state": "APPROVED", "sha": red}).to_string(),
    )
    .unwrap();
    let report = check_ci(root, &red, &rebaseline, 1, Some(reviews.path()));
    assert_eq!(outcome(&report, "proposed-owner-review"), "violation");
    fs::write(
        reviews.path(),
        json!({"owner": "pmdroid", "state": "APPROVED", "sha": rebaseline}).to_string(),
    )
    .unwrap();
    let report = check_ci(root, &red, &rebaseline, 0, Some(reviews.path()));
    assert_eq!(outcome(&report, "proposed-owner-review"), "pass");

    // A baseline-only raise above the real counts still fails strict.
    let mut value = baseline_json(root);
    value["rules"]["source/no-marker"]["paths"]["src/b.ts"] = json!(5);
    fs::write(
        root.join(BASELINE),
        format!("{}\n", serde_json::to_string_pretty(&value).unwrap()),
    )
    .unwrap();
    let padded = commit(root);
    let report = check_ci(root, &rebaseline, &padded, 2, None);
    assert_eq!(
        report["phases"]["base-check"]["baseline"]["source"],
        "proposed"
    );
    assert_eq!(outcome(&report, "proposed-check"), "violation");
}

#[test]
fn exact_ci_fails_closed_on_an_unreadable_baseline() {
    let (repo, base) = adopted();
    let root = repo.path();
    fs::remove_file(root.join(BASELINE)).unwrap();
    symlink("elsewhere.json", root.join(BASELINE)).unwrap();
    let link = commit(root);
    let report = check_ci(root, &base, &link, 2, None);
    assert_eq!(outcome(&report, "baseline-read"), "error");
    assert!(
        report["phases"]["baseline-read"]["error"]
            .as_str()
            .unwrap()
            .contains("must be a regular file")
    );
    assert!(report["phases"].get("base-check").is_none());

    // A changed rule must ship a regenerated entry: old counts do not apply.
    fs::remove_file(root.join(BASELINE)).unwrap();
    git(root, &["checkout", "-q", &base, "--", BASELINE]);
    write(
        root,
        ".rulecheck/rules/marker.rule.ts",
        &RULE.replace("\"MARKER\" }", "\"MARK\" }"),
    );
    let changed = commit(root);
    let report = check_ci(root, &base, &changed, 2, None);
    assert_eq!(outcome(&report, "base-check"), "pass");
    assert_eq!(outcome(&report, "proposed-check"), "error");
    regenerate(root);
    let regenerated = commit(root);
    let report = check_ci(root, &base, &regenerated, 0, None);
    assert_eq!(report["baselineChange"]["raised"], json!([]));
    assert_eq!(report["baselineChange"]["lowered"], json!([]));
}

#[test]
fn installed_wrapper_forwards_baselines_relative_to_the_current_directory() {
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
    let wrapper = |cwd: &Path, args: &[&str]| {
        Command::new("node")
            .arg(dir.join("bin/rulecheck.mjs"))
            .args(args)
            .current_dir(cwd)
            .env_remove("RULECHECK_WRAPPER_ACTIVE")
            .output()
            .unwrap()
    };
    let (repo, base) = adopted();
    let root = repo.path();
    let recorded = read(root, BASELINE);
    fs::remove_file(root.join(BASELINE)).unwrap();

    let (stdout, _) = run(wrapper(root, &["baseline"]), 0);
    assert_eq!(stdout, recorded);
    // From a subdirectory with --root, FILE is relative to the current directory.
    let (_, stderr) = run(
        wrapper(
            &root.join("src"),
            &[
                "baseline",
                "--root",
                "..",
                "--baseline",
                "../.rulecheck/baseline.json",
                "--write",
            ],
        ),
        0,
    );
    assert!(stderr.contains("the baseline records 2 findings in 1 paths for 1 rules"));
    assert_eq!(read(root, BASELINE), recorded);
    run(
        wrapper(
            root,
            &["check", "--baseline", BASELINE, "--strict-baseline"],
        ),
        0,
    );
    write(root, "src/b.ts", "MARKER\n");
    let (stdout, _) = run(wrapper(root, &["check", "--baseline", BASELINE]), 1);
    assert!(stdout.contains("src/b.ts: [source/no-marker] 1 finding, baseline allows 0"));
    run(
        wrapper(root, &["check", "--commit", &base, "--baseline", BASELINE]),
        0,
    );
    let (stdout, _) = run(
        wrapper(root, &["check", "--baseline", BASELINE, "--format", "json"]),
        1,
    );
    let result: Value = serde_json::from_str(&stdout).unwrap();
    assert!(common::violations(&schema("result-v1.json"), &result).is_empty());
    assert!(result["tools"]["builder"].is_string());
    assert_eq!(result["baseline"]["exceeded"][0]["path"], "src/b.ts");

    for (args, message) in [
        (
            &["check", "--strict-baseline"][..],
            "--strict-baseline requires --baseline FILE",
        ),
        (&["baseline", "--write"], "--write requires --baseline FILE"),
        (
            &["baseline", "--format", "json"],
            "unexpected argument \"--format\"",
        ),
    ] {
        let (stdout, stderr) = run(wrapper(root, args), 2);
        assert_eq!(stdout, "");
        assert!(stderr.contains(message), "{stderr}");
    }
    // A builder failure still names the baseline flags in the error result.
    write(
        root,
        ".rulecheck/rules/marker.rule.ts",
        &RULE.replace("remediation:", "remedy:"),
    );
    let (stdout, _) = run(
        wrapper(
            root,
            &[
                "check",
                "--baseline",
                BASELINE,
                "--strict-baseline",
                "--format",
                "json",
            ],
        ),
        2,
    );
    let result: Value = serde_json::from_str(&stdout).unwrap();
    assert!(common::violations(&schema("result-v1.json"), &result).is_empty());
    assert_eq!(
        result["source"],
        json!({ "mode": "working-tree", "commit": null, "matches": "all", "baselineSha256": null, "strictBaseline": true })
    );
    let (help, _) = run(wrapper(root, &["help"]), 0);
    assert!(
        help.contains("[--baseline FILE [--strict-baseline]]"),
        "{help}"
    );
    let (help, _) = run(cli(&["help"], b"", root), 0);
    assert!(help.contains("rulecheck baseline"), "{help}");
}
