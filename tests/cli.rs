use serde_json::json;
use std::fs;
use std::io::Write;
use std::os::unix::fs::symlink;
use std::path::Path;
use std::process::{Command, Output, Stdio};

const ROOT: &str = env!("CARGO_MANIFEST_DIR");
const EXAMPLE: &str = include_str!("../.rulecheck/rules/no-legacy-mode.rule.ts");

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

fn build(root: &Path) -> Output {
    Command::new("node")
        .arg(Path::new(ROOT).join("tools/build.mjs"))
        .arg("--root")
        .arg(root)
        .output()
        .expect("run build; install Node.js and run npm ci")
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

fn expect(output: &Output, code: i32) {
    assert_eq!(
        output.status.code(),
        Some(code),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
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
            "Rulecheck fixture",
        ],
    );
    git(root, &["rev-parse", "HEAD"])
}

fn check_ci(root: &Path, base: &str, candidate: &str, code: i32) -> serde_json::Value {
    check_ci_with_reviews(root, base, candidate, code, None)
}

fn check_ci_with_reviews(
    root: &Path,
    base: &str,
    candidate: &str,
    code: i32,
    reviews: Option<&Path>,
) -> serde_json::Value {
    let mut command = Command::new("node");
    command
        .current_dir(root)
        .arg(Path::new(ROOT).join("tools/check-ci.mjs"))
        .arg(root)
        .args([base, candidate, env!("CARGO_BIN_EXE_rulecheck")])
        .env("RULECHECK_TOOL_SHA", base)
        .env_remove("RULECHECK_POLICY_REVIEWS");
    if let Some(reviews) = reviews {
        command.env("RULECHECK_POLICY_REVIEWS", reviews);
    }
    let output = command.output().expect("run exact-commit wrapper");
    expect(&output, code);
    let evidence: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(evidence["baseSha"], base);
    assert_eq!(evidence["candidateSha"], candidate);
    assert_eq!(evidence["tools"]["sourceSha"], base);
    assert_eq!(
        evidence["outcome"],
        ["pass", "violation", "error"][code as usize]
    );
    evidence
}

fn init_exact_repo(root: &Path) -> String {
    git(root, &["init", "-q"]);
    fs::create_dir_all(root.join(".rulecheck/rules")).unwrap();
    fs::create_dir(root.join("config")).unwrap();
    fs::write(root.join(".rulecheck/rules/demo.rule.ts"), EXAMPLE).unwrap();
    fs::write(root.join("config/app.toml"), "legacy_mode = false\n").unwrap();
    commit(root)
}

#[test]
fn exact_ci_checks_both_policies_in_a_bare_repository() {
    let repo = tempfile::tempdir().unwrap();
    let base = init_exact_repo(repo.path());
    fs::write(repo.path().join("config/app.toml"), "legacy_mode = true\n").unwrap();
    let violation = commit(repo.path());
    let bare = tempfile::tempdir().unwrap();
    git(bare.path(), &["init", "--bare", "-q"]);
    git(
        bare.path(),
        &[
            "fetch",
            "--depth=1",
            repo.path().to_str().unwrap(),
            &base,
            &violation,
        ],
    );

    let clean = check_ci(bare.path(), &base, &base, 0);
    let phases = &clean["phases"];
    assert_eq!(clean["effectivePolicyDiff"]["changes"], json!([]));
    assert_eq!(phases["proposed-test"]["outcome"], "pass");
    assert_eq!(phases["base-check"]["policySha"], base);
    let digest = phases["base-check"]["policyDigest"].as_str().unwrap();
    assert_eq!(digest.len(), 71);
    assert!(digest.starts_with("sha256:"));
    assert_eq!(phases["proposed-check"]["policyDigest"], digest);
    for tool in ["node", "git", "builder", "evaluator"] {
        assert!(!clean["tools"][tool].as_str().unwrap().is_empty());
    }

    let report = check_ci(bare.path(), &base, &violation, 1);
    for phase in ["base-check", "proposed-check"] {
        assert_eq!(report["phases"][phase]["outcome"], "violation");
        assert_eq!(report["phases"][phase]["policyDigest"], digest);
        assert!(
            report["phases"][phase]["stdout"]
                .as_str()
                .unwrap()
                .contains("config/app.toml:1:1: [config/no-legacy-mode]")
        );
    }
}

#[test]
fn exact_ci_keeps_base_policy_when_the_candidate_weakens_or_removes_it() {
    let repo = tempfile::tempdir().unwrap();
    let root = repo.path();
    let base = init_exact_repo(root);
    let source = root.join(".rulecheck/rules/demo.rule.ts");
    let candidate_file = root.join("config/app.toml");
    fs::write(&candidate_file, "legacy_mode = true\n").unwrap();
    fs::create_dir(root.join("tools")).unwrap();
    fs::write(
        root.join("tools/build.mjs"),
        "throw new Error('candidate tool')",
    )
    .unwrap();
    let weakened_rule = EXAMPLE.replace("legacy_mode = true", "legacy_variant = true");
    fs::write(&source, &weakened_rule).unwrap();
    let weakened = commit(root);

    fs::write(&source, "invalid and staged rule").unwrap();
    fs::write(&candidate_file, "legacy_mode = false\n").unwrap();
    git(root, &["add", "-A"]);
    let report = check_ci(root, &base, &weakened, 1);
    let phases = &report["phases"];
    assert_eq!(phases["base-check"]["outcome"], "violation");
    assert_eq!(phases["proposed-test"]["outcome"], "pass");
    assert_eq!(phases["proposed-check"]["outcome"], "pass");
    assert_eq!(phases["proposed-check"]["policySha"], weakened);
    assert_ne!(
        phases["base-check"]["policyDigest"],
        phases["proposed-check"]["policyDigest"]
    );
    let change = &report["effectivePolicyDiff"]["changes"][0];
    assert_eq!(change["id"], "config/no-legacy-mode");
    assert_eq!(change["after"]["pattern"]["value"], "legacy_variant = true");

    fs::write(&candidate_file, "legacy_mode = true\n").unwrap();
    fs::write(
        &source,
        weakened_rule.replace("no-legacy-mode", "no-legacy-variant"),
    )
    .unwrap();
    let removal = commit(root);
    let reviews = tempfile::NamedTempFile::new().unwrap();
    fs::write(
        reviews.path(),
        json!({"owner": "pmdroid", "state": "APPROVED", "sha": removal}).to_string(),
    )
    .unwrap();
    let report = check_ci_with_reviews(root, &base, &removal, 1, Some(reviews.path()));
    assert_eq!(report["phases"]["base-check"]["outcome"], "violation");
    assert_eq!(report["phases"]["proposed-check"]["outcome"], "pass");
    assert_eq!(report["phases"]["proposed-owner-review"]["outcome"], "pass");
    assert_eq!(
        report["phases"]["proposed-owner-review"]["reviewSha"],
        removal
    );
    assert!(report["effectivePolicyDiff"]["changes"][0]["after"].is_null());
    assert!(report["effectivePolicyDiff"]["changes"][1]["before"].is_null());
}

#[test]
fn exact_ci_requires_a_current_owner_approval_for_a_policy_change_in_shadow() {
    let repo = tempfile::tempdir().unwrap();
    let root = repo.path();
    let base = init_exact_repo(root);
    fs::write(
        root.join(".rulecheck/rules/demo.rule.ts"),
        EXAMPLE.replace("Remove legacy_mode", "Delete legacy_mode"),
    )
    .unwrap();
    let candidate = commit(root);
    let reviews = tempfile::NamedTempFile::new().unwrap();
    let current = json!({"owner": "pmdroid", "state": "APPROVED", "sha": candidate});
    let commented = json!({"owner": "pmdroid", "state": "COMMENTED", "sha": candidate});
    let dismissed = json!({"owner": "pmdroid", "state": "DISMISSED", "sha": candidate});
    let stale = json!({"owner": "pmdroid", "state": "APPROVED", "sha": base});
    let other = json!({"owner": "someone", "state": "APPROVED", "sha": candidate});
    for (entries, exit) in [
        (format!("{stale}\n{other}"), 1),
        (format!("{current}\n{commented}"), 0),
        (dismissed.to_string(), 1),
    ] {
        fs::write(reviews.path(), entries).unwrap();
        let report = check_ci_with_reviews(root, &base, &candidate, exit, Some(reviews.path()));
        assert_eq!(report["phases"]["base-check"]["outcome"], "pass");
        assert_eq!(report["phases"]["proposed-check"]["outcome"], "pass");
        assert_eq!(report["phases"]["proposed-owner-review"]["required"], true);
    }
    let missing = root.join("unavailable-reviews.jsonl");
    let report = check_ci_with_reviews(root, &base, &candidate, 2, Some(&missing));
    assert_eq!(
        report["phases"]["proposed-owner-review"]["outcome"],
        "error"
    );
    let report = check_ci_with_reviews(root, &base, &base, 0, Some(&missing));
    assert_eq!(report["phases"]["proposed-owner-review"]["required"], false);
}

#[test]
fn exact_ci_reports_errors_without_falling_back_to_checkout_files() {
    let repo = tempfile::tempdir().unwrap();
    let root = repo.path();
    let base = init_exact_repo(root);
    let source = root.join(".rulecheck/rules/demo.rule.ts");
    let candidate_file = root.join("config/app.toml");

    fs::remove_file(&source).unwrap();
    let missing = commit(root);
    fs::write(&source, EXAMPLE).unwrap();
    let report = check_ci(root, &base, &missing, 2);
    assert_eq!(report["phases"]["base-check"]["outcome"], "pass");
    assert_eq!(report["phases"]["proposed-build"]["outcome"], "error");
    assert!(report["effectivePolicyDiff"].is_null());
    let report = check_ci(root, &missing, &base, 2);
    assert_eq!(report["phases"]["base-build"]["outcome"], "error");
    assert_eq!(report["phases"]["proposed-check"]["outcome"], "pass");

    fs::write(
        &source,
        format!("{EXAMPLE}\nthrow new Error('rule modules are data');"),
    )
    .unwrap();
    let invalid_source = commit(root);
    let report = check_ci(root, &base, &invalid_source, 2);
    assert_eq!(report["phases"]["proposed-build"]["outcome"], "error");

    fs::write(&source, EXAMPLE.replace("config/no-legacy-mode", "INVALID")).unwrap();
    let invalid_policy = commit(root);
    let report = check_ci(root, &base, &invalid_policy, 2);
    assert_eq!(report["phases"]["proposed-validate"]["outcome"], "error");
    assert!(report["phases"]["proposed-validate"]["policyDigest"].is_null());

    fs::write(&source, EXAMPLE.replace("line: 2", "line: 3")).unwrap();
    let failed_fixture = commit(root);
    let report = check_ci(root, &base, &failed_fixture, 1);
    assert_eq!(report["phases"]["proposed-test"]["outcome"], "violation");
    assert_eq!(report["phases"]["proposed-check"]["outcome"], "pass");

    fs::write(&source, EXAMPLE).unwrap();
    fs::remove_file(&candidate_file).unwrap();
    let empty_scope = commit(root);
    fs::write(&candidate_file, "legacy_mode = false\n").unwrap();
    let report = check_ci(root, &base, &empty_scope, 2);
    assert_eq!(report["phases"]["base-check"]["outcome"], "error");
    assert!(
        report["phases"]["base-check"]["stderr"]
            .as_str()
            .unwrap()
            .contains("empty text scope")
    );

    fs::remove_file(&source).unwrap();
    symlink("other.ts", &source).unwrap();
    fs::remove_file(&candidate_file).unwrap();
    symlink("other.txt", &candidate_file).unwrap();
    let links = commit(root);
    let report = check_ci(root, &base, &links, 2);
    for phase in ["base-check", "proposed-build"] {
        assert!(
            report["phases"][phase]["stderr"]
                .as_str()
                .unwrap()
                .contains("regular Git blob")
        );
    }
}

#[test]
fn displays_help() {
    let output = cli(&["--help"], b"", Path::new(ROOT));
    expect(&output, 0);
    let help = String::from_utf8(output.stdout).unwrap();
    for command in ["build", "schema", "validate", "list", "test", "check"] {
        assert!(help.contains(command));
    }
}

#[test]
fn displays_version() {
    let output = cli(&["--version"], b"", Path::new(ROOT));
    expect(&output, 0);
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!("rulecheck {}\n", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn build_and_check_the_demo_end_to_end() {
    let repo = tempfile::tempdir().unwrap();
    let root = repo.path();
    git(root, &["init", "-q"]);
    fs::create_dir_all(root.join(".rulecheck/rules")).unwrap();
    fs::write(root.join(".rulecheck/rules/demo.rule.ts"), EXAMPLE).unwrap();
    fs::create_dir(root.join("config")).unwrap();
    let candidate = root.join("config/app.toml");
    fs::write(&candidate, "legacy_mode = false\n").unwrap();
    git(root, &["add", "config/app.toml"]);

    let output = build(root);
    expect(&output, 0);
    assert_eq!(output.stdout, build(root).stdout);
    let policy = output.stdout;
    expect(&cli(&["validate"], &policy, root), 0);

    let output = cli(&["schema"], b"", root);
    expect(&output, 0);
    let schema: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(schema["properties"]["version"]["const"], 1);

    let output = cli(&["test", "--id-prefix", "config/"], &policy, root);
    expect(&output, 0);
    assert_eq!(output.stdout, b"2 fixtures passed\n");
    let output = cli(&["list", "--id-prefix", "config/"], &policy, root);
    expect(&output, 0);
    assert_eq!(output.stdout, b"config/no-legacy-mode\n");
    expect(&cli(&["check", "--id-prefix", "unknown"], &policy, root), 2);
    expect(&cli(&["check"], &policy, root), 0);

    fs::write(candidate, "name = \"app\"\nlegacy_mode = true\n").unwrap();
    let output = cli(&["check"], &policy, root);
    expect(&output, 1);
    assert_eq!(
        output.stdout,
        b"config/app.toml:2:1: [config/no-legacy-mode] Remove legacy_mode or set it to false.\n"
    );

    let output = cli(&["check"], b"{\"version\":42,\"rules\":[]}", root);
    expect(&output, 2);
    assert!(String::from_utf8_lossy(&output.stderr).contains("unsupported policy version 42"));
}

#[test]
fn builder_sorts_sources_and_rejects_nonliteral_syntax() {
    let repo = tempfile::tempdir().unwrap();
    let root = repo.path();
    git(root, &["init", "-q"]);
    let rules = root.join(".rulecheck/rules");
    fs::create_dir_all(rules.join("nested")).unwrap();
    let last = rules.join("z.rule.ts");
    fs::write(&last, EXAMPLE.replace("config/no-legacy-mode", "a/rule")).unwrap();
    fs::write(rules.join("nested/a.rule.ts"), EXAMPLE).unwrap();
    symlink(&last, rules.join("ignored.rule.ts")).unwrap();
    let output = build(root);
    expect(&output, 0);
    let policy: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(policy["rules"].as_array().unwrap().len(), 2);
    assert_eq!(policy["rules"][0]["id"], "config/no-legacy-mode");
    assert_eq!(policy["rules"][1]["id"], "a/rule");

    for (source, message) in [
        (
            EXAMPLE.replace("value: legacySetting", "value: makePattern()"),
            "plain literal",
        ),
        (
            EXAMPLE.replace("@rulecheck/sdk", "./other.ts"),
            "SDK marker import",
        ),
        (
            EXAMPLE.replace("value: legacySetting", "value: later"),
            "earlier literal const",
        ),
        (
            format!("{EXAMPLE}\nexport default defineRule({{}});"),
            "one export default",
        ),
    ] {
        fs::write(&last, source).unwrap();
        let output = build(root);
        expect(&output, 2);
        assert!(output.stdout.is_empty());
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(error.contains(".rulecheck/rules/z.rule.ts:"), "{error}");
        assert!(error.contains(message), "{error}");
    }
}

#[test]
fn scoped_regex_and_forbidden_paths_share_fixture_matching_and_ignore_links() {
    let repo = tempfile::tempdir().unwrap();
    let root = repo.path();
    git(root, &["init", "-q"]);
    fs::create_dir(root.join("vendor")).unwrap();
    fs::write(root.join("vendor/banned.txt"), "bad1").unwrap();
    git(
        root,
        &[
            "update-index",
            "--add",
            "--cacheinfo",
            "160000,1111111111111111111111111111111111111111,vendor",
        ],
    );
    fs::write(root.join("outside.txt"), "bad1").unwrap();
    symlink("outside.txt", root.join("linked.txt")).unwrap();
    git(root, &["add", "linked.txt"]);

    let path_rule = json!({
        "id": "path/banned", "remediation": "Remove this file.", "kind": "forbidden-path",
        "paths": ["**/banned.txt", "linked.txt"],
        "fixtures": [
            {"path": "ok.txt", "expect": null},
            {"path": "banned.txt", "expect": {"line": 1, "column": 1}}
        ]
    });
    let text_rule = json!({
        "id": "text/bad", "remediation": "Use a supported value.", "kind": "text",
        "pattern": {"kind": "regex", "value": "bad[0-9]+"}, "paths": ["*.txt"],
        "fixtures": [
            {"path": "ok.txt", "content": "good1", "expect": null},
            {"path": "banned.txt", "content": "é bad17", "expect": {"line": 1, "column": 3}}
        ]
    });
    let paths_only = json!({"version": 1, "rules": [&path_rule]}).to_string();
    expect(&cli(&["check"], paths_only.as_bytes(), root), 0);

    let policy = json!({"version": 1, "rules": [text_rule, path_rule]}).to_string();
    let policy = policy.as_bytes();
    expect(&cli(&["test"], policy, root), 0);
    let output = cli(&["check"], policy, root);
    expect(&output, 2);
    assert!(String::from_utf8_lossy(&output.stderr).contains("[text/bad] empty text scope"));

    fs::write(root.join("z.txt"), "bad3").unwrap();
    fs::write(root.join("banned.txt"), "é bad17").unwrap();
    git(root, &["add", "z.txt", "banned.txt"]);
    let output = cli(&["check"], policy, root);
    expect(&output, 1);
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        concat!(
            "banned.txt:1:1: [path/banned] Remove this file.\n",
            "banned.txt:1:3: [text/bad] Use a supported value.\n",
            "z.txt:1:1: [text/bad] Use a supported value.\n"
        )
    );

    fs::remove_file(root.join("banned.txt")).unwrap();
    symlink("outside.txt", root.join("banned.txt")).unwrap();
    let output = cli(&["check"], policy, root);
    expect(&output, 2);
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("without following symlinks"));
}
