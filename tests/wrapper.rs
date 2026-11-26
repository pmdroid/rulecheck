//! The installed `rulecheck` wrapper, run from a separate repository through PATH.
mod common;

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use tempfile::TempDir;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");
const EXAMPLE: &str = include_str!("../.rulecheck/rules/no-legacy-mode.rule.ts");
const FINDING: &str =
    "config/app.toml:2:1: [config/no-legacy-mode] Remove legacy_mode or set it to false.\n";

/// A local installation laid out like a Rulecheck checkout: the shipped wrapper and
/// builder, the pinned dependencies, the release evaluator, and a PATH symlink.
struct Install {
    dir: TempDir,
}

impl Install {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("rulecheck");
        for path in ["bin", "tools", "target/release"] {
            fs::create_dir_all(root.join(path)).unwrap();
        }
        for path in ["bin/rulecheck.mjs", "tools/build.mjs", "package.json"] {
            fs::copy(Path::new(ROOT).join(path), root.join(path)).unwrap();
        }
        symlink(
            Path::new(ROOT).join("node_modules"),
            root.join("node_modules"),
        )
        .unwrap();
        fs::copy(
            env!("CARGO_BIN_EXE_rulecheck"),
            root.join("target/release/rulecheck"),
        )
        .unwrap();
        fs::create_dir(dir.path().join("bin")).unwrap();
        symlink(
            root.join("bin/rulecheck.mjs"),
            dir.path().join("bin/rulecheck"),
        )
        .unwrap();
        Self { dir }
    }

    fn root(&self) -> PathBuf {
        self.dir.path().join("rulecheck")
    }

    fn run(&self, cwd: &Path, args: &[&str]) -> Output {
        let path = std::env::join_paths(
            std::iter::once(self.dir.path().join("bin"))
                .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
        )
        .unwrap();
        Command::new("rulecheck")
            .current_dir(cwd)
            .args(args)
            .env("PATH", path)
            .env_remove("RULECHECK_WRAPPER_ACTIVE")
            .output()
            .expect("run the installed rulecheck wrapper")
    }
}

fn git(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
}

fn expect(output: &Output, code: i32, stdout: &str) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert_eq!(output.status.code(), Some(code), "stderr: {stderr}");
    assert_eq!(String::from_utf8_lossy(&output.stdout), stdout, "{stderr}");
    stderr
}

fn snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut files = BTreeMap::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            let kind = fs::symlink_metadata(&path).unwrap().file_type();
            if kind.is_dir() {
                pending.push(path);
            } else if kind.is_file() {
                files.insert(path.clone(), fs::read(path).unwrap());
            } else {
                files.insert(path.clone(), Vec::new());
            }
        }
    }
    files
}

/// A target repository whose package scripts, local packages, and local builder
/// would each leave a marker file if anything loaded or ran them.
fn hostile_repo(markers: &Path) -> TempDir {
    let repo = tempfile::tempdir().unwrap();
    let root = repo.path();
    git(root, &["init", "-q"]);
    let mark = |name: &str| markers.join(name).display().to_string();
    let module = |name: &str| format!("require('node:fs').writeFileSync({:?}, '');\n", mark(name));
    let script = |name: &str| {
        format!(
            "node -e \"require('fs').writeFileSync('{}', '')\"",
            mark(name)
        )
    };
    let files = [
        (".rulecheck/rules/demo.rule.ts".to_owned(), EXAMPLE.to_owned()),
        ("config/app.toml".to_owned(), "legacy_mode = false\n".to_owned()),
        (
            "package.json".to_owned(),
            serde_json::json!({
                "name": "target",
                "type": "commonjs",
                "scripts": Object::from_iter(
                    ["preinstall", "install", "postinstall", "prepare", "build", "test", "rulecheck"]
                        .map(|name| (name.to_owned(), script(name).into())),
                ),
            })
            .to_string(),
        ),
        (
            "node_modules/@rulecheck/sdk/package.json".to_owned(),
            r#"{"name":"@rulecheck/sdk","version":"0.0.0","main":"index.js"}"#.to_owned(),
        ),
        ("node_modules/@rulecheck/sdk/index.js".to_owned(), module("sdk")),
        (
            "node_modules/typescript/package.json".to_owned(),
            r#"{"name":"typescript","version":"5.9.3","main":"index.js"}"#.to_owned(),
        ),
        ("node_modules/typescript/index.js".to_owned(), module("typescript")),
        (
            "tools/build.mjs".to_owned(),
            format!(
                "import {{ writeFileSync }} from 'node:fs';\nwriteFileSync({:?}, '');\n",
                mark("builder")
            ),
        ),
        ("target/release/rulecheck".to_owned(), format!("#!/bin/sh\n{}\n", script("evaluator"))),
    ];
    for (path, contents) in files {
        let path = root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }
    git(root, &["add", "-A"]);
    repo
}

type Object = serde_json::Map<String, serde_json::Value>;

#[test]
fn installed_check_loads_rules_from_another_repository_without_running_its_code() {
    let install = Install::new();
    let markers = tempfile::tempdir().unwrap();
    let repo = hostile_repo(markers.path());
    let root = repo.path();
    let root_arg = root.to_str().unwrap();
    let before = snapshot(root);
    let installed = snapshot(&install.root().join("bin"));

    expect(&install.run(root, &["check"]), 0, "");
    expect(&install.run(root, &["test"]), 0, "2 fixtures passed\n");
    expect(&install.run(root, &["list"]), 0, "config/no-legacy-mode\n");
    expect(&install.run(root, &["validate"]), 0, "");
    let elsewhere = markers.path();
    expect(
        &install.run(elsewhere, &["check", "--root", root_arg]),
        0,
        "",
    );

    let candidate = root.join("config/app.toml");
    fs::write(&candidate, "name = \"app\"\nlegacy_mode = true\n").unwrap();
    expect(&install.run(root, &["check"]), 1, FINDING);
    expect(
        &install.run(elsewhere, &["check", "--root", root_arg]),
        1,
        FINDING,
    );
    let args = ["check", "--root", root_arg, "--id-prefix", "config/"];
    expect(&install.run(elsewhere, &args), 1, FINDING);
    let args = ["check", "--root", root_arg, "--id-prefix", "unknown/"];
    let error = expect(&install.run(elsewhere, &args), 2, "");
    assert!(
        error.contains("no rule IDs match prefix \"unknown/\""),
        "{error}"
    );

    fs::write(&candidate, "legacy_mode = false\n").unwrap();
    assert_eq!(snapshot(root), before);
    assert_eq!(snapshot(&install.root().join("bin")), installed);
    assert!(!install.root().join("policy.json").exists());
    assert_eq!(fs::read_dir(markers.path()).unwrap().count(), 0);
}

#[test]
fn installed_check_reports_invalid_rules_and_failed_builders() {
    let install = Install::new();
    let markers = tempfile::tempdir().unwrap();
    let repo = hostile_repo(markers.path());
    let root = repo.path();
    let rule = root.join(".rulecheck/rules/demo.rule.ts");

    fs::write(
        &rule,
        EXAMPLE.replace("value: legacySetting", "value: makePattern()"),
    )
    .unwrap();
    let error = expect(&install.run(root, &["check"]), 2, "");
    assert!(
        error.contains(".rulecheck/rules/demo.rule.ts:9:38: Unsupported value"),
        "{error}"
    );
    assert!(error.contains("could not build policy"), "{error}");

    fs::write(&rule, EXAMPLE.replace("config/no-legacy-mode", "INVALID")).unwrap();
    let error = expect(&install.run(root, &["check"]), 2, "");
    assert!(error.contains("invalid rule ID \"INVALID\""), "{error}");

    fs::write(&rule, EXAMPLE).unwrap();
    let builder = install.root().join("tools/build.mjs");
    fs::write(&builder, "process.stdout.write('{}'); process.exit(3);\n").unwrap();
    let error = expect(&install.run(root, &["check"]), 2, "");
    assert!(error.contains("could not build policy"), "{error}");

    fs::copy(Path::new(ROOT).join("tools/build.mjs"), &builder).unwrap();
    fs::remove_file(install.root().join("node_modules")).unwrap();
    let error = expect(&install.run(root, &["check"]), 2, "");
    assert!(error.contains("needs TypeScript 5.9.3"), "{error}");
    assert!(error.contains("npm ci --ignore-scripts"), "{error}");

    let usage = expect(&install.run(root, &["check", "--bogus", "x"]), 2, "");
    assert!(usage.contains("unexpected argument \"--bogus\""), "{usage}");
    assert_eq!(fs::read_dir(markers.path()).unwrap().count(), 0);
}

#[test]
fn installed_wrapper_never_launches_itself_as_the_evaluator() {
    let install = Install::new();
    let markers = tempfile::tempdir().unwrap();
    let repo = hostile_repo(markers.path());
    let evaluator = install.root().join("target/release/rulecheck");

    fs::remove_file(&evaluator).unwrap();
    let error = expect(&install.run(repo.path(), &["check"]), 2, "");
    assert!(error.contains("cargo build --locked --release"), "{error}");

    symlink(install.root().join("bin/rulecheck.mjs"), &evaluator).unwrap();
    let output = install.run(repo.path(), &["check"]);
    let error = expect(&output, 2, "");
    assert!(error.contains("must be the Rust evaluator"), "{error}");
    assert_eq!(fs::read_dir(markers.path()).unwrap().count(), 0);
}

fn json_result(output: &Output, code: i32) -> serde_json::Value {
    assert_eq!(output.status.code(), Some(code), "{output:?}");
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let schema = serde_json::from_str(include_str!("../schema/result-v1.json")).unwrap();
    let errors = common::violations(&schema, &result);
    assert!(errors.is_empty(), "{errors:?}\n{result:#}");
    assert_eq!(result["exitCode"], code);
    result
}

#[test]
fn installed_check_writes_a_json_result_even_when_the_build_fails() {
    let install = Install::new();
    let markers = tempfile::tempdir().unwrap();
    let repo = hostile_repo(markers.path());
    let root = repo.path();
    let builder = "rulecheck-builder 0.1.0 (TypeScript 5.9.3)";
    let evaluator = format!("rulecheck {}", env!("CARGO_PKG_VERSION"));

    fs::write(
        root.join("config/app.toml"),
        "name = \"app\"\nlegacy_mode = true\n",
    )
    .unwrap();
    let result = json_result(&install.run(root, &["check", "--format", "json"]), 1);
    assert_eq!(
        result["tools"],
        serde_json::json!({ "builder": builder, "evaluator": evaluator })
    );
    assert_eq!(result["findings"][0]["byteStart"], 13);
    assert_eq!(result["findings"][0]["byteEnd"], 31);
    // Human output is unchanged by the machine format.
    expect(
        &install.run(root, &["check", "--format", "human"]),
        1,
        FINDING,
    );

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
            "candidate",
        ],
    );
    let output = Command::new("git")
        .current_dir(root)
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    let sha = String::from_utf8(output.stdout).unwrap().trim().to_owned();
    let args = ["check", "--commit", &sha, "--format", "json"];
    let exact = json_result(&install.run(root, &args), 1);
    assert_eq!(
        exact["source"],
        serde_json::json!({ "mode": "commit", "commit": sha })
    );
    assert_eq!(exact["findings"], result["findings"]);

    // --untracked reaches the evaluator, is recorded in the source, and is a
    // usage error with --commit before anything is built.
    fs::write(root.join("config/new.toml"), "legacy_mode = true\n").unwrap();
    expect(&install.run(root, &["check"]), 1, FINDING);
    let added =
        "config/new.toml:1:1: [config/no-legacy-mode] Remove legacy_mode or set it to false.\n";
    expect(
        &install.run(root, &["check", "--untracked"]),
        1,
        &format!("{FINDING}{added}"),
    );
    let output = install.run(root, &["check", "--format", "json", "--untracked"]);
    let untracked = json_result(&output, 1);
    assert_eq!(
        untracked["source"],
        serde_json::json!({ "mode": "working-tree", "commit": null, "untracked": true })
    );
    assert_eq!(untracked["findings"][1]["path"], "config/new.toml");
    assert!(result["source"].get("untracked").is_none());
    for args in [
        &["check", "--commit", &sha, "--untracked"][..],
        &["check", "--untracked", "--commit", &sha, "--format", "json"],
    ] {
        let usage = expect(&install.run(root, args), 2, "");
        assert_eq!(
            usage,
            "rulecheck: --untracked applies to working-tree checks only; \
             a commit has no untracked files, so drop --untracked or --commit\n"
        );
    }
    for args in [
        &["test", "--untracked"][..],
        &["check", "--untracked", "--untracked"],
    ] {
        let usage = expect(&install.run(root, args), 2, "");
        assert!(
            usage.contains("unexpected argument \"--untracked\""),
            "{usage}"
        );
    }
    fs::remove_file(root.join("config/new.toml")).unwrap();

    fs::write(
        root.join(".rulecheck/rules/demo.rule.ts"),
        EXAMPLE.replace("value: legacySetting", "value: makePattern()"),
    )
    .unwrap();
    let output = install.run(root, &["check", "--format", "json"]);
    let failed = json_result(&output, 2);
    assert_eq!(failed["outcome"], "error");
    assert_eq!(failed["findings"], serde_json::json!([]));
    assert_eq!(failed["policy"]["sha256"], serde_json::Value::Null);
    assert_eq!(failed["tools"]["evaluator"], evaluator.as_str());
    assert!(failed["source"].get("untracked").is_none());
    let output = install.run(root, &["check", "--untracked", "--format", "json"]);
    assert_eq!(json_result(&output, 2)["source"]["untracked"], true);
    let messages: Vec<_> = failed["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["message"].as_str().unwrap())
        .collect();
    assert_eq!(messages.len(), 2, "{messages:?}");
    assert!(
        messages[0].contains("demo.rule.ts:9:38: Unsupported value"),
        "{messages:?}"
    );
    assert!(
        messages[1].starts_with("could not build policy"),
        "{messages:?}"
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("could not build policy"));

    let usage = expect(&install.run(root, &["check", "--format", "xml"]), 2, "");
    assert!(usage.contains("--format requires human or json"), "{usage}");
    let usage = expect(&install.run(root, &["test", "--format", "json"]), 2, "");
    assert!(usage.contains("unexpected argument"), "{usage}");
    assert_eq!(fs::read_dir(markers.path()).unwrap().count(), 0);
}

/// Each error names what to fix: the rule file and fixture, the commit argument, or
/// the checkout root. None of them is blamed on the policy the user never sees.
#[test]
fn installed_cli_points_errors_at_the_real_cause() {
    let install = Install::new();
    let markers = tempfile::tempdir().unwrap();
    let repo = hostile_repo(markers.path());
    let root = repo.path();
    let output = install.run(root, &["help"]);
    let text = String::from_utf8(output.stdout.clone()).unwrap();
    assert_eq!(expect(&output, 0, &text), "");
    assert!(text.starts_with("rulecheck: deterministic repository checks\n"));
    assert!(
        text.contains("rulecheck help     Print this help"),
        "{text}"
    );
    assert_eq!(install.run(root, &["--help"]).stdout, text.as_bytes());

    let usage = expect(&install.run(root, &[]), 2, "");
    assert!(usage.starts_with(&text), "{usage}");
    assert!(
        usage.ends_with("\nrulecheck: missing command; run rulecheck help\n"),
        "{usage}"
    );

    for (commit, resolve) in [("HEAD", "HEAD"), ("a0504e7", "a0504e7"), ("x y", "HEAD")] {
        let expected = format!(
            "rulecheck: --commit requires a full 40- or 64-character Git commit SHA, \
             not \"{commit}\"; resolve it first, e.g. --commit \"$(git rev-parse {resolve})\"\n"
        );
        for format in ["human", "json"] {
            let args = ["check", "--commit", commit, "--format", format];
            assert_eq!(expect(&install.run(root, &args), 2, ""), expected);
        }
        assert_eq!(
            expect(&install.run(root, &["validate", "--commit", commit]), 2, ""),
            expected
        );
    }

    let subdir = root.join("config");
    let top = String::from_utf8(
        Command::new("git")
            .current_dir(root)
            .args(["rev-parse", "--show-toplevel"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    let expected = format!(
        "rulecheck: {} is a subdirectory of a Git checkout, not its root; \
         run from the checkout root or pass --root (--root {})\n",
        fs::canonicalize(&subdir).unwrap().display(),
        top.trim()
    );
    for args in [&["check"][..], &["check", "--format", "json"], &["test"]] {
        assert_eq!(expect(&install.run(&subdir, args), 2, ""), expected);
    }

    let rule = root.join(".rulecheck/rules/demo.rule.ts");
    fs::write(
        &rule,
        EXAMPLE.replace("line: 2, column: 1", "line: 2, col: 1"),
    )
    .unwrap();
    let error = expect(&install.run(root, &["validate"]), 2, "");
    assert!(
        error.starts_with(
            "rulecheck build: .rulecheck/rules/demo.rule.ts:16:26: \
             Unknown property col in fixture 2 expect. Use line and column.\n"
        ),
        "{error}"
    );
    assert!(!error.contains("invalid policy JSON"), "{error}");
    assert_eq!(fs::read_dir(markers.path()).unwrap().count(), 0);
}
