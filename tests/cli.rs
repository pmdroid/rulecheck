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

fn git(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
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
