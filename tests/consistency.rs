//! Bounded cross-file consistency rules: normalization, duplicates, empty
//! scopes, symlinks, path traversal, deterministic findings and limits, with
//! the same result in a working-tree check and an exact-commit check.
mod common;

use serde_json::{Value, json};
use std::fs;
use std::io::Write;
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

/// Stages everything, commits and returns the commit.
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
            "Consistency fixture",
        ],
    );
    git(root, &["rev-parse", "HEAD"])
}

fn write(root: &Path, path: &str, content: &str) {
    let path = root.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

fn repo(files: &[(&str, &str)]) -> tempfile::TempDir {
    let repo = tempfile::tempdir().unwrap();
    git(repo.path(), &["init", "-q"]);
    for (path, content) in files {
        write(repo.path(), path, content);
    }
    repo
}

fn policy(rule: Value) -> Vec<u8> {
    serde_json::to_vec(&json!({ "version": 1, "rules": [rule] })).unwrap()
}

/// Flags documented in docs/*.md must be the keys of config/*.json.
fn flags() -> Value {
    json!({
        "id": "docs/flags-match-config",
        "kind": "consistency",
        "paths": ["docs/*.md", "config/*.json"],
        "consistency": {
            "form": "same-values",
            "left": { "name": "documented", "paths": ["docs/*.md"], "extract": "(?m)^- `([^`]*)`" },
            "right": { "name": "configured", "paths": ["config/*.json"], "extract": "\"([^\"]*)\"\\s*:" }
        },
        "remediation": "Keep docs and config in step.",
        "fixtures": [{
            "files": [
                { "path": "docs/flags.md", "content": "- `a`\n" },
                { "path": "config/flags.json", "content": "{\"a\": 1}" }
            ],
            "expectFindings": []
        }]
    })
}

/// Every `include:` line of a manifest names a file under pkg/.
fn manifest(base: &str) -> Value {
    let reference = if base == "root" { "pkg/a.txt" } else { "a.txt" };
    json!({
        "id": "pkg/manifest-files-exist",
        "kind": "consistency",
        "paths": ["pkg/**"],
        "consistency": {
            "form": "path-exists",
            "source": { "name": "manifest", "paths": ["pkg/**/MANIFEST"], "extract": "(?m)^include:(.*)$" },
            "base": base
        },
        "remediation": "List only files that exist.",
        "fixtures": [{
            "files": [
                { "path": "pkg/MANIFEST", "content": format!("include: {reference}\n") },
                { "path": "pkg/a.txt" }
            ],
            "expectFindings": []
        }]
    })
}

/// Runs the human and JSON checks in both modes and asserts they agree.
/// Returns the human stdout and the JSON findings.
fn both_modes(root: &Path, policy: &[u8], code: i32) -> (String, String, Value) {
    let sha = commit(root);
    let tree = run(cli(&["check"], policy, root), code);
    let exact = run(cli(&["check", "--commit", &sha], policy, root), code);
    assert_eq!(tree, exact, "working tree and commit differ");
    let json_tree = run(cli(&["check", "--format", "json"], policy, root), code).0;
    let json_exact = run(
        cli(
            &["check", "--format", "json", "--commit", &sha],
            policy,
            root,
        ),
        code,
    )
    .0;
    let mut tree: Value = serde_json::from_str(&json_tree).unwrap();
    let exact: Value = serde_json::from_str(&json_exact).unwrap();
    let schema: Value = serde_json::from_str(include_str!("../schema/result-v1.json")).unwrap();
    assert_eq!(common::violations(&schema, &tree), Vec::<String>::new());
    assert_eq!(tree["findings"], exact["findings"]);
    assert_eq!(tree["diagnostics"], exact["diagnostics"]);
    let (stdout, stderr) = run(cli(&["check"], policy, root), code);
    (stdout, stderr, tree["findings"].take())
}

#[test]
fn rule_and_fixture_shapes_match_the_schema() {
    let schema: Value = serde_json::from_str(include_str!("../schema/policy-v1.json")).unwrap();
    for rule in [flags(), manifest("root"), manifest("file")] {
        let bytes = policy(rule);
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(common::violations(&schema, &value), Vec::<String>::new());
        let root = Path::new(ROOT);
        assert_eq!(run(cli(&["validate"], &bytes, root), 0).0, "");
        assert_eq!(
            run(cli(&["test"], &bytes, root), 0).0,
            "1 fixtures passed\n"
        );
    }
    // A text-style fixture on a consistency rule, and a consistency fixture on
    // a text rule, are refused by both.
    let mut text_fixture = flags();
    text_fixture["fixtures"] = json!([{ "path": "docs/a.md", "content": "", "expect": null }]);
    let mut text_rule = json!({
        "id": "x/y", "kind": "text", "paths": ["**"], "remediation": "r",
        "pattern": { "kind": "literal", "value": "x" },
        "fixtures": [{ "files": [{ "path": "a", "content": "" }], "expectFindings": [] }]
    });
    for (rule, message) in [
        (
            text_fixture,
            "consistency fixtures take files and expectFindings",
        ),
        (
            text_rule.take(),
            "files and expectFindings are for consistency rules",
        ),
    ] {
        let bytes = policy(rule);
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert_ne!(common::violations(&schema, &value), Vec::<String>::new());
        let stderr = run(cli(&["validate"], &bytes, Path::new(ROOT)), 2).1;
        assert!(stderr.contains(message), "{stderr}");
    }
}

#[test]
fn invalid_consistency_blocks_are_refused() {
    let root = Path::new(ROOT);
    let cases: Vec<(Value, &str)> = vec![
        (json!("([a-z]+)-([0-9]+)"), "exactly one capture group"),
        (json!("[a-z]+"), "exactly one capture group"),
        (json!("("), "invalid extract regex"),
    ];
    for (extract, message) in cases {
        let mut rule = flags();
        rule["consistency"]["left"]["extract"] = extract;
        let stderr = run(cli(&["validate"], &policy(rule), root), 2).1;
        assert!(stderr.contains(message), "{stderr}");
    }
    let mut same_name = flags();
    same_name["consistency"]["right"]["name"] = json!("documented");
    let stderr = run(cli(&["validate"], &policy(same_name), root), 2).1;
    assert!(stderr.contains("both named \"documented\""), "{stderr}");
    let mut missing = flags();
    missing.as_object_mut().unwrap().remove("consistency");
    let stderr = run(cli(&["validate"], &policy(missing), root), 2).1;
    assert!(stderr.contains("require a consistency block"), "{stderr}");
    let mut wrong_kind = flags();
    wrong_kind["kind"] = json!("forbidden-path");
    let stderr = run(cli(&["validate"], &policy(wrong_kind), root), 2).1;
    assert!(stderr.contains("only they take one"), "{stderr}");
    let mut skip = flags();
    skip["skipIfContent"] = json!({ "kind": "literal", "value": "x", "firstBytes": 10 });
    let stderr = run(cli(&["validate"], &policy(skip), root), 2).1;
    assert!(stderr.contains("take no skipIfContent"), "{stderr}");
}

#[test]
fn values_are_trimmed_deduplicated_and_reported_once_at_their_first_occurrence() {
    let repo = repo(&[
        // " beta " is trimmed, "" is dropped, "gamma" appears twice.
        ("docs/a.md", "- ` beta `\n- ``\n- `gamma`\n"),
        ("docs/b.md", "- `gamma`\n- `alpha`\n"),
        ("config/one.json", "{\"beta\": 1, \"delta\": 2}"),
        ("config/two.json", "{\"alpha\": 1, \"delta\": 3}"),
        // Outside the rule's paths: never read.
        ("notes/x.md", "- `zeta`\n"),
    ]);
    let (stdout, stderr, findings) = both_modes(repo.path(), &policy(flags()), 1);
    assert_eq!(
        stdout,
        "config/one.json:1:14: [docs/flags-match-config] \"delta\" is in configured but not in documented. Keep docs and config in step.\n\
         docs/a.md:3:4: [docs/flags-match-config] \"gamma\" is in documented but not in configured. Keep docs and config in step.\n"
    );
    assert_eq!(stderr, "");
    assert_eq!(
        findings[0]["message"],
        json!("\"delta\" is in configured but not in documented.")
    );
    assert_eq!(findings[0]["byteStart"], json!(13));
    assert_eq!(findings[0]["byteEnd"], json!(18));
    // --all-matches reports the same findings: each missing value once.
    let all = run(
        cli(&["check", "--all-matches"], &policy(flags()), repo.path()),
        1,
    )
    .0;
    assert_eq!(all, stdout);
}

#[test]
fn a_scope_with_no_file_cannot_be_judged_in_either_mode() {
    let repo = repo(&[("docs/flags.md", "- `a`\n"), ("src/main.rs", "")]);
    let (stdout, stderr, findings) = both_modes(repo.path(), &policy(flags()), 2);
    assert_eq!(stdout, "");
    assert_eq!(findings, json!([]));
    assert_eq!(
        stderr,
        "[docs/flags-match-config] empty consistency scope \"configured\": no file of the rule's \
inventory matches it; track a matching file or update the scope's path globs\n\
rulecheck: one or more consistency rules have a scope that matched no repository files\n"
    );
    // A scope whose files hold no values is judged: the other side's values
    // are all missing from it.
    write(repo.path(), "config/flags.json", "{}");
    let (stdout, ..) = both_modes(repo.path(), &policy(flags()), 1);
    assert_eq!(
        stdout,
        "docs/flags.md:1:4: [docs/flags-match-config] \"a\" is in documented but not in configured. Keep docs and config in step.\n"
    );
}

#[test]
fn references_resolve_from_the_root_or_the_file_and_never_leave_the_repository() {
    let manifest_text = "include: a.txt\n\
include: ./sub/../a.txt\n\
include: sub/b.txt\n\
include: missing.txt\n\
include: ../outside.txt\n\
include: ../../../etc/passwd\n\
include: /etc/passwd\n\
include: sub//b.txt\n\
include: .git/config\n\
include: sub\\b.txt\n";
    let repo = repo(&[
        ("pkg/MANIFEST", manifest_text),
        ("pkg/a.txt", ""),
        ("pkg/sub/b.txt", ""),
        // In the repository but outside the rule's paths, so not in its inventory.
        ("outside.txt", ""),
    ]);
    let (stdout, _, findings) = both_modes(repo.path(), &policy(manifest("file")), 1);
    let lines: Vec<_> = stdout.lines().collect();
    let r = "List only files that exist.";
    assert_eq!(
        lines,
        [
            format!(
                "pkg/MANIFEST:4:10: [pkg/manifest-files-exist] \"missing.txt\" resolves to pkg/missing.txt, which is not in the selected inventory. {r}"
            ),
            format!(
                "pkg/MANIFEST:5:10: [pkg/manifest-files-exist] \"../outside.txt\" resolves to outside.txt, which is not in the selected inventory. {r}"
            ),
            format!(
                "pkg/MANIFEST:6:10: [pkg/manifest-files-exist] \"../../../etc/passwd\" climbs above the repository root; a reference must name a file inside the repository. {r}"
            ),
            format!(
                "pkg/MANIFEST:7:10: [pkg/manifest-files-exist] \"/etc/passwd\" is absolute; a reference must name a file inside the repository. {r}"
            ),
            format!(
                "pkg/MANIFEST:8:10: [pkg/manifest-files-exist] \"sub//b.txt\" has an empty path segment; a reference must name a file inside the repository. {r}"
            ),
            format!(
                "pkg/MANIFEST:9:10: [pkg/manifest-files-exist] \".git/config\" names a .git directory; a reference must name a file inside the repository. {r}"
            ),
            format!(
                "pkg/MANIFEST:10:10: [pkg/manifest-files-exist] \"sub\\b.txt\" contains a backslash or control character; a reference must name a file inside the repository. {r}"
            ),
        ]
    );
    assert_eq!(findings.as_array().unwrap().len(), 7);

    // From the root, the same manifest names root-relative paths.
    let mut root_rule = manifest("root");
    root_rule["paths"] = json!(["pkg/**", "a.txt"]);
    write(
        repo.path(),
        "pkg/MANIFEST",
        "include: pkg/a.txt\ninclude: a.txt\ninclude: ../a.txt\n",
    );
    let (stdout, ..) = both_modes(repo.path(), &policy(root_rule), 1);
    assert_eq!(
        stdout,
        format!(
            "pkg/MANIFEST:2:10: [pkg/manifest-files-exist] a.txt is not in the selected inventory. {r}\n\
             pkg/MANIFEST:3:10: [pkg/manifest-files-exist] \"../a.txt\" climbs above the repository root; a reference must name a file inside the repository. {r}\n"
        )
    );
}

#[cfg(unix)]
#[test]
fn a_symlink_in_scope_cannot_be_judged_in_either_mode() {
    let repo = repo(&[("pkg/MANIFEST", "include: a.txt\n"), ("pkg/real.txt", "")]);
    std::os::unix::fs::symlink("real.txt", repo.path().join("pkg/a.txt")).unwrap();
    let message = "rulecheck: [pkg/manifest-files-exist] pkg/a.txt: a symlink or submodule is in \
the scope of a consistency rule, which judges regular files only; exclude it\n";
    let (stdout, stderr, _) = both_modes(repo.path(), &policy(manifest("file")), 2);
    assert_eq!(stdout, "");
    assert_eq!(stderr, message);

    // Excluded, the symlink is outside the inventory, so the reference to it
    // is missing in both modes.
    let mut excluded = manifest("file");
    excluded["exclude"] = json!(["pkg/a.txt"]);
    let (stdout, ..) = both_modes(repo.path(), &policy(excluded), 1);
    assert_eq!(
        stdout,
        "pkg/MANIFEST:1:10: [pkg/manifest-files-exist] \"a.txt\" resolves to pkg/a.txt, which is not \
in the selected inventory. List only files that exist.\n"
    );
}

#[test]
fn a_scope_over_its_value_limit_cannot_be_judged() {
    let many: String = (0..10_001).map(|n| format!("- `v{n}`\n")).collect();
    let repo = repo(&[("docs/flags.md", &many), ("config/flags.json", "{}")]);
    let (_, stderr, _) = both_modes(repo.path(), &policy(flags()), 2);
    assert_eq!(
        stderr,
        "rulecheck: [docs/flags-match-config] scope \"documented\" extracted more than 10000 \
values; narrow its paths or tighten the extract regex\n"
    );
    let long = format!("- `{}`\n", "x".repeat(1025));
    write(repo.path(), "docs/flags.md", &long);
    let (_, stderr, _) = both_modes(repo.path(), &policy(flags()), 2);
    assert!(stderr.contains("longer than 1024 bytes"), "{stderr}");
}

#[test]
fn a_budget_stop_reports_no_consistency_findings() {
    // 1.2 MiB of documented values with a 1 MiB budget: the scan stops at the
    // second docs file, so the rule is unevaluated instead of reporting
    // values that file might have held.
    let big: String = format!("- `a`\n{}", "filler\n".repeat(90_000));
    let repo = repo(&[
        ("config/flags.json", "{\"a\": 1}"),
        ("docs/a.md", &big),
        ("docs/b.md", &format!("- `b`\n{big}")),
    ]);
    let (stdout, stderr, findings) = {
        let sha = commit(repo.path());
        let args = ["check", "--text-budget", "1"];
        let tree = run(cli(&args, &policy(flags()), repo.path()), 2);
        let exact = run(
            cli(
                &[&args[..], &["--commit", &sha]].concat(),
                &policy(flags()),
                repo.path(),
            ),
            2,
        );
        assert_eq!(tree, exact);
        let json = run(
            cli(
                &["check", "--text-budget", "1", "--format", "json"],
                &policy(flags()),
                repo.path(),
            ),
            2,
        )
        .0;
        let value: Value = serde_json::from_str(&json).unwrap();
        (tree.0, tree.1, value["findings"].clone())
    };
    assert_eq!(stdout, "");
    assert_eq!(findings, json!([]));
    assert!(
        stderr.starts_with(
            "[docs/flags-match-config] not evaluated on every file in scope: the scan stopped at the text budget\n"
        ),
        "{stderr}"
    );
}

#[test]
fn a_failing_fixture_names_both_finding_lists() {
    let mut rule = flags();
    rule["fixtures"] = json!([{
        "files": [
            { "path": "docs/flags.md", "content": "- `a`\n- `b`\n" },
            { "path": "config/flags.json", "content": "{\"a\": 1}" }
        ],
        "expectFindings": []
    }]);
    let stdout = run(cli(&["test"], &policy(rule.clone()), Path::new(ROOT)), 1).0;
    assert_eq!(
        stdout,
        "docs/flags.md:2:4: [docs/flags-match-config] fixture 1 expected no violations, found \
violations at docs/flags.md:2:4. Keep docs and config in step. Update the rule or fixture.\n"
    );
    // A fixture whose scope has no file cannot be judged.
    rule["fixtures"][0]["files"] = json!([{ "path": "docs/flags.md", "content": "- `a`\n" }]);
    let stdout = run(cli(&["test"], &policy(rule), Path::new(ROOT)), 1).0;
    assert!(
        stdout.contains("scope \"configured\" matches no file in the fixture"),
        "{stdout}"
    );
}
