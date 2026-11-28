//! Pins the machine-readable `check --format json` result, its byte coordinates,
//! fail-closed exits, the input bounds, schema agreement for shipped examples,
//! and the reproducibility tool.
mod common;

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn policy_schema() -> Value {
    serde_json::from_str(include_str!("../schema/policy-v1.json")).unwrap()
}

fn result_schema() -> Value {
    serde_json::from_str(include_str!("../schema/result-v1.json")).unwrap()
}

fn cli(args: &[&str], policy: &[u8], root: &Path) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_rulecheck-evaluator"))
        .current_dir(root)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("run rulecheck");
    // The evaluator may reject oversized input before reading all of it.
    let _ = child.stdin.take().unwrap().write_all(policy);
    child.wait_with_output().unwrap()
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
            "Rulecheck result fixture",
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

fn policy(rules: &[Value]) -> Vec<u8> {
    let policy = json!({ "version": 1, "rules": rules });
    let errors = common::violations(&policy_schema(), &policy);
    assert!(errors.is_empty(), "{errors:?}");
    policy.to_string().into_bytes()
}

fn text_rule(id: &str, paths: &[&str], kind: &str, value: &str) -> Value {
    json!({
        "id": id, "kind": "text", "paths": paths,
        "pattern": { "kind": kind, "value": value },
        "remediation": format!("Fix {id}."),
        "fixtures": [{ "path": paths[0].replace("**", "x").replace('*', "x"), "content": "", "expect": null }]
    })
}

fn path_rule(id: &str, paths: &[&str]) -> Value {
    json!({
        "id": id, "kind": "forbidden-path", "paths": paths,
        "remediation": format!("Remove {id}."),
        "fixtures": [{ "path": "elsewhere.txt", "expect": null }]
    })
}

/// Runs `check --format json`, checks the exit code against the document and
/// the result schema, and returns the parsed result plus stderr.
fn check_json(args: &[&str], policy: &[u8], root: &Path, code: i32) -> (Value, String) {
    let mut full = vec!["check", "--format", "json"];
    full.extend_from_slice(args);
    let output = cli(&full, policy, root);
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert_eq!(output.status.code(), Some(code), "stderr: {stderr}");
    let result: Value = serde_json::from_slice(&output.stdout).expect("one JSON document");
    let errors = common::violations(&result_schema(), &result);
    assert!(errors.is_empty(), "{errors:?}\n{result:#}");
    assert_eq!(result["exitCode"], code);
    assert_eq!(
        result["outcome"],
        ["pass", "violation", "error"][code as usize]
    );
    (result, stderr)
}

fn human(args: &[&str], policy: &[u8], root: &Path, code: i32) -> String {
    let mut full = vec!["check"];
    full.extend_from_slice(args);
    let output = cli(&full, policy, root);
    assert_eq!(output.status.code(), Some(code), "{output:?}");
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn json_findings_carry_utf8_byte_spans_in_human_order() {
    let repo = repo();
    let root = repo.path();
    let app = "// π naïve\nlet a = OLD_NAME; let b = OLD_NAME;\n";
    write(root, "src/app.ts", app.as_bytes());
    write(root, "src/z.ts", "🦀🦀 debug=true\n".as_bytes());
    write(root, "secrets/key.pem", b"");
    let rules = policy(&[
        text_rule("a/old-name", &["src/**/*.ts"], "literal", "OLD_NAME"),
        text_rule("b/debug", &["src/**/*.ts"], "regex", r"debug\s*=\s*true"),
        text_rule("c/naive", &["src/**/*.ts"], "regex", "na.ve"),
        path_rule("d/no-pem", &["**/*.pem"]),
    ]);
    git(root, &["add", "-A"]);

    let lines = human(&[], &rules, root, 1);
    assert_eq!(
        lines,
        "secrets/key.pem:1:1: [d/no-pem] Remove d/no-pem.\n\
         src/app.ts:1:6: [c/naive] Fix c/naive.\n\
         src/app.ts:2:9: [a/old-name] Fix a/old-name.\n\
         src/z.ts:1:4: [b/debug] Fix b/debug.\n"
    );

    let (result, stderr) = check_json(&[], &rules, root, 1);
    assert_eq!(stderr, "");
    let findings = result["findings"].as_array().unwrap();
    // The machine order and coordinates are the human ones.
    let rendered: String = findings
        .iter()
        .map(|f| {
            format!(
                "{}:{}:{}: [{}] {}\n",
                f["path"].as_str().unwrap(),
                f["line"],
                f["column"],
                f["ruleId"].as_str().unwrap(),
                f["remediation"].as_str().unwrap()
            )
        })
        .collect();
    assert_eq!(rendered, lines);

    // Byte spans are zero-based and half-open over the file's UTF-8 bytes.
    let spans: Vec<_> = findings
        .iter()
        .map(|f| (f["byteStart"].clone(), f["byteEnd"].clone()))
        .collect();
    assert_eq!(
        spans,
        [
            (Value::Null, Value::Null),
            (json!(6), json!(12)),
            (json!(21), json!(29)),
            (json!(9), json!(19)),
        ]
    );
    assert_eq!(&app.as_bytes()[6..12], "naïve".as_bytes());
    assert_eq!(&app[21..29], "OLD_NAME");
    assert_eq!(&"🦀🦀 debug=true\n"[9..19], "debug=true");

    let digest = format!("sha256:{:x}", Sha256::digest(&rules));
    assert_eq!(
        result["policy"],
        json!({ "version": 1, "sha256": digest, "idPrefix": null,
                "ruleIds": ["a/old-name", "b/debug", "c/naive", "d/no-pem"] })
    );
    assert_eq!(
        result["source"],
        json!({ "mode": "working-tree", "commit": null })
    );
    assert_eq!(
        result["tools"],
        json!({ "evaluator": format!("rulecheck {}", env!("CARGO_PKG_VERSION")), "builder": null })
    );
    assert!(result["diagnostics"].as_array().unwrap().is_empty());

    // Commit mode records the exact commit, and a prefix narrows the rule set.
    let sha = commit(root);
    let (exact, _) = check_json(&["--commit", &sha, "--id-prefix", "a/"], &rules, root, 1);
    assert_eq!(exact["source"], json!({ "mode": "commit", "commit": sha }));
    assert_eq!(exact["policy"]["idPrefix"], "a/");
    assert_eq!(exact["policy"]["ruleIds"], json!(["a/old-name"]));
    assert_eq!(exact["findings"], json!([findings[2]]));

    // A clean scan is a pass with no findings, and --format human is the default.
    write(root, "src/app.ts", b"let a = NEW_NAME;\n");
    write(root, "src/z.ts", b"debug = false\n");
    fs::remove_file(root.join("secrets/key.pem")).unwrap();
    let (clean, _) = check_json(&[], &rules, root, 0);
    assert_eq!(clean["findings"], json!([]));
    assert_eq!(human(&["--format", "human"], &rules, root, 0), "");
}

#[test]
fn json_results_fail_closed_with_diagnostics() {
    let repo = repo();
    let root = repo.path();
    write(root, "src/app.ts", b"OLD_NAME\n");
    git(root, &["add", "-A"]);
    let rules = policy(&[
        text_rule("a/old-name", &["src/**/*.ts"], "literal", "OLD_NAME"),
        text_rule("b/unscoped", &["lib/**/*.ts"], "literal", "OLD_NAME"),
    ]);

    // An empty text scope cannot be judged even though the other rule found a
    // violation. The findings are complete and still reported.
    let (result, stderr) = check_json(&[], &rules, root, 2);
    assert_eq!(result["findings"].as_array().unwrap().len(), 1);
    assert_eq!(
        result["diagnostics"],
        json!([
            { "ruleId": "b/unscoped", "message": "empty text scope: track a matching file or update the rule's path globs" },
            { "ruleId": null, "message": "one or more text rules matched no repository files" }
        ])
    );
    assert!(stderr.contains("[b/unscoped] empty text scope"), "{stderr}");
    assert_eq!(
        human(&[], &rules, root, 2),
        "src/app.ts:1:1: [a/old-name] Fix a/old-name.\n"
    );

    // A scoped file that cannot be read as UTF-8 stops the scan without findings.
    write(root, "src/bad.ts", &[0xff, 0xfe]);
    git(root, &["add", "-A"]);
    let only = policy(&[text_rule(
        "a/old-name",
        &["src/**/*.ts"],
        "literal",
        "OLD_NAME",
    )]);
    let (result, _) = check_json(&[], &only, root, 2);
    assert_eq!(result["findings"], json!([]));
    let message = result["diagnostics"][0]["message"].as_str().unwrap();
    assert!(
        message.contains("src/bad.ts: expected UTF-8 text"),
        "{message}"
    );

    // Invalid policy keeps the digest of the bytes read but has no version or rules.
    let (result, _) = check_json(&[], b"{\"version\":2,\"rules\":[]}", root, 2);
    assert_eq!(result["policy"]["version"], Value::Null);
    assert_eq!(result["policy"]["ruleIds"], json!([]));
    assert!(
        result["policy"]["sha256"]
            .as_str()
            .unwrap()
            .starts_with("sha256:")
    );

    // A missing commit or unknown prefix is an error result, not a pass.
    let missing = "0".repeat(40);
    let (result, _) = check_json(&["--commit", &missing], &only, root, 2);
    assert_eq!(result["source"]["commit"], missing);
    let (result, _) = check_json(&["--id-prefix", "zz/"], &only, root, 2);
    assert!(
        result["diagnostics"][0]["message"]
            .as_str()
            .unwrap()
            .contains("no rule IDs match")
    );

    // --untracked is recorded in the source only when it is passed.
    assert!(result["source"].get("untracked").is_none());
    write(root, "src/new.ts", b"OLD_NAME\n");
    let (result, _) = check_json(&["--untracked"], &only, root, 2);
    assert_eq!(
        result["source"],
        json!({ "mode": "working-tree", "commit": null, "untracked": true })
    );
    fs::remove_file(root.join("src/bad.ts")).unwrap();
    let (result, _) = check_json(&["--untracked"], &only, root, 1);
    assert_eq!(result["source"]["untracked"], true);
    let paths: Vec<_> = result["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|finding| finding["path"].as_str().unwrap())
        .collect();
    assert_eq!(paths, ["src/app.ts", "src/new.ts"]);
    let (result, _) = check_json(&[], &only, root, 1);
    assert_eq!(
        result["source"],
        json!({ "mode": "working-tree", "commit": null })
    );
    let mut bad = result.clone();
    bad["source"]["untracked"] = json!(false);
    assert!(!common::violations(&result_schema(), &bad).is_empty());

    // Bad arguments never produce a document.
    let output = cli(&["check", "--format", "xml"], &only, root);
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("--format requires human or json"));
    let output = cli(&["test", "--format", "json"], &only, root);
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn input_bounds_fail_closed() {
    let repo = repo();
    let root = repo.path();
    write(root, "src/app.ts", b"ok\n");
    git(root, &["add", "-A"]);
    let rule = text_rule("a/marker", &["src/**/*.ts"], "literal", "MARKER");
    let marker = policy(std::slice::from_ref(&rule));

    // Policy input over 4 MiB: unread, so there is no digest.
    let mut large = vec![b' '; 4 * 1024 * 1024 + 1];
    large[0] = b'{';
    let (result, _) = check_json(&[], &large, root, 2);
    assert_eq!(result["policy"]["sha256"], Value::Null);
    assert!(
        result["diagnostics"][0]["message"]
            .as_str()
            .unwrap()
            .contains("exceeds 4194304 bytes")
    );

    // More than 1024 rules.
    let many: Vec<_> = (0..1025)
        .map(|n| text_rule(&format!("a/r{n}"), &["src/**/*.ts"], "literal", "MARKER"))
        .collect();
    let body = json!({ "version": 1, "rules": many }).to_string();
    let (result, _) = check_json(&[], body.as_bytes(), root, 2);
    assert!(
        result["diagnostics"][0]["message"]
            .as_str()
            .unwrap()
            .contains("1 to 1024 rules")
    );

    // A scoped text file over 1 MiB.
    write(root, "src/big.ts", &vec![b'a'; 1024 * 1024 + 1]);
    git(root, &["add", "-A"]);
    let (result, _) = check_json(&[], &marker, root, 2);
    let message = result["diagnostics"][0]["message"].as_str().unwrap();
    assert!(
        message.contains("src/big.ts") && message.contains("exceeds 1048576 bytes"),
        "{message}"
    );
    fs::remove_file(root.join("src/big.ts")).unwrap();

    // More than 64 MiB of scoped text across files of 1 MiB each.
    let chunk = vec![b'a'; 1024 * 1024];
    for n in 0..65 {
        write(root, &format!("src/bulk/{n:02}.ts"), &chunk);
    }
    git(root, &["add", "-A"]);
    let (result, _) = check_json(&[], &marker, root, 2);
    let message = result["diagnostics"][1]["message"].as_str().unwrap();
    assert!(message.contains("over the 64 MiB text budget"), "{message}");
    // Other scoped files under src/ sort first, so 63 files of 1 MiB fit.
    assert_eq!(result["partial"]["stoppedAt"], "src/bulk/63.ts");
    fs::remove_dir_all(root.join("src/bulk")).unwrap();

    // More than 10,000 findings.
    for n in 0..10_001 {
        write(root, &format!("src/many/{n:05}.ts"), b"MARKER\n");
    }
    git(root, &["add", "-A"]);
    let (result, _) = check_json(&[], &policy(&[rule]), root, 2);
    assert_eq!(result["findings"], json!([]));
    let message = result["diagnostics"][0]["message"].as_str().unwrap();
    assert!(message.contains("more than 10,000 findings"), "{message}");

    // A rule source over 128 KiB is rejected by the builder.
    let rules = tempfile::tempdir().unwrap();
    git(rules.path(), &["init", "-q"]);
    let source = format!(
        "import {{ defineRule }} from \"@rulecheck/sdk\";\n// {}\n",
        "x".repeat(128 * 1024)
    );
    write(
        rules.path(),
        ".rulecheck/rules/big.rule.ts",
        source.as_bytes(),
    );
    let output = Command::new("node")
        .arg(Path::new(ROOT).join("tools/build.mjs"))
        .arg("--root")
        .arg(rules.path())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("under 128 KiB"));
}

fn build(root: &Path) -> Vec<u8> {
    let output = Command::new("node")
        .arg(Path::new(ROOT).join("tools/build.mjs"))
        .arg("--root")
        .arg(root)
        .output()
        .expect("run build; install Node.js and run npm ci");
    assert!(output.status.success(), "{output:?}");
    output.stdout
}

#[test]
fn shipped_rules_and_recipes_match_the_policy_schema() {
    let schema = policy_schema();
    let mut sources: Vec<_> = fs::read_dir(Path::new(ROOT).join("docs/examples/rules"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    sources.push(Path::new(ROOT).join(".rulecheck/rules/no-legacy-mode.rule.ts"));
    sources.sort();
    assert!(sources.len() >= 8, "{sources:?}");

    for source in sources {
        let repo = repo();
        let target = repo.path().join(".rulecheck/rules/example.rule.ts");
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::copy(&source, &target).unwrap();
        let built = build(repo.path());
        let value: Value = serde_json::from_slice(&built).unwrap();
        let errors = common::violations(&schema, &value);
        assert!(errors.is_empty(), "{}: {errors:?}", source.display());
        // The evaluator accepts what the schema accepts, and every fixture passes.
        let output = cli(&["test"], &built, repo.path());
        assert_eq!(
            output.status.code(),
            Some(0),
            "{}: {output:?}",
            source.display()
        );
    }

    // Structural mistakes that the evaluator rejects are rejected by the schema too.
    let valid = text_rule("a/marker", &["src/**/*.ts"], "literal", "MARKER");
    let mut cases = Vec::new();
    let mut missing_pattern = valid.clone();
    missing_pattern.as_object_mut().unwrap().remove("pattern");
    cases.push(missing_pattern);
    let mut path_with_pattern = path_rule("a/path", &["**/*.pem"]);
    path_with_pattern["pattern"] = json!({ "kind": "literal", "value": "x" });
    cases.push(path_with_pattern);
    let mut path_with_content = path_rule("a/path", &["**/*.pem"]);
    path_with_content["fixtures"][0]["content"] = json!("");
    cases.push(path_with_content);
    let mut text_without_content = valid.clone();
    text_without_content["fixtures"][0]
        .as_object_mut()
        .unwrap()
        .remove("content");
    cases.push(text_without_content);
    let mut zero_line = valid.clone();
    zero_line["fixtures"][0]["expect"] = json!({ "line": 0, "column": 1 });
    cases.push(zero_line);
    let mut empty_exclude = valid.clone();
    empty_exclude["exclude"] = json!([]);
    cases.push(empty_exclude);
    let mut bad_id = valid.clone();
    bad_id["id"] = json!("Bad/ID");
    cases.push(bad_id);
    let mut extra = valid;
    extra["severity"] = json!("error");
    cases.push(extra);

    for rule in cases {
        let policy = json!({ "version": 1, "rules": [rule] });
        assert!(
            !common::violations(&schema, &policy).is_empty(),
            "schema accepted {policy}"
        );
        let output = cli(
            &["validate"],
            policy.to_string().as_bytes(),
            Path::new(ROOT),
        );
        assert_eq!(output.status.code(), Some(2), "evaluator accepted {policy}");
    }
}

#[test]
fn reproduction_builds_twice_and_compares_results() {
    let repo = repo();
    let root = repo.path();
    fs::create_dir_all(root.join(".rulecheck/rules")).unwrap();
    for recipe in ["multi-scope", "path-globs"] {
        fs::copy(
            Path::new(ROOT).join(format!("docs/examples/rules/{recipe}.rule.ts")),
            root.join(format!(".rulecheck/rules/{recipe}.rule.ts")),
        )
        .unwrap();
    }
    write(
        root,
        "packages/core/src/index.ts",
        "// π OLD_NAME OLD_NAME\n".as_bytes(),
    );
    write(root, "services/api/index.ts", b"// old_name\n");
    write(root, "config/private-2b.toml", b"");
    write(root, "docker-compose.override.yml", b"");
    let sha = commit(root);

    let run = |args: &[&str]| {
        Command::new("node")
            .arg(Path::new(ROOT).join("tools/reproduce.mjs"))
            .args(args)
            .output()
            .unwrap()
    };
    let output = run(&[
        root.to_str().unwrap(),
        &sha,
        env!("CARGO_BIN_EXE_rulecheck-evaluator"),
    ]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["outcome"], "reproducible");
    assert_eq!(report["commit"], sha);
    assert_eq!(
        report["identical"],
        json!({ "policyBytes": true, "tests": true, "findingsInOrder": true, "result": true })
    );
    let runs = report["runs"].as_array().unwrap();
    assert_eq!(runs.len(), 2);
    for run in runs {
        assert_eq!(run["checkOutcome"], "violation");
        assert_eq!(run["checkExitCode"], 1);
        assert_eq!(run["testExitCode"], 0);
        assert_eq!(run["findings"], 3);
        for step in ["build", "test", "check"] {
            assert!(run["wallTimeMs"][step].is_u64(), "{run}");
        }
    }
    assert_eq!(runs[0]["policySha256"], runs[1]["policySha256"]);

    // A step that cannot run is exit 2, never a reproducible pass.
    let output = run(&[
        root.to_str().unwrap(),
        &"0".repeat(40),
        env!("CARGO_BIN_EXE_rulecheck-evaluator"),
    ]);
    assert_eq!(output.status.code(), Some(2));
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["outcome"], "error");
    assert!(
        report["error"]
            .as_str()
            .unwrap()
            .contains("builder exited 2")
    );
    let output = run(&[
        root.to_str().unwrap(),
        "HEAD",
        env!("CARGO_BIN_EXE_rulecheck-evaluator"),
    ]);
    assert_eq!(output.status.code(), Some(2));
}
