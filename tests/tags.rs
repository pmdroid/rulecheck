//! Rule tags (PAS-2674): bounded, stable labels for ADR and domain coverage that
//! the schema, SDK, builder and evaluator validate alike, that never change what
//! a rule matches, and that `list --format json` reports in a deterministic
//! rule inventory (schema/inventory-v1.json). Human `list` output is unchanged.

mod common;

use serde_json::{Value, json};
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

const ROOT: &str = env!("CARGO_MANIFEST_DIR");
// The frozen literal rule keeps builder positions stable while the live rule changes.
const EXAMPLE: &str = include_str!("fixtures/literal-no-legacy-mode.rule.ts");
const FINDING: &str = "https://github.com/pmdroid/rulecheck/pull/1#discussion_r1";

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

/// Runs a command and returns stdout (stderr for exit 2) after checking the exit code.
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

fn repo() -> tempfile::TempDir {
    let repo = tempfile::tempdir().unwrap();
    git(repo.path(), &["init", "-q"]);
    repo
}

fn write(root: &Path, path: &str, content: &str) {
    let path = root.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

fn build(root: &Path) -> Output {
    Command::new("node")
        .arg(Path::new(ROOT).join("tools/build.mjs"))
        .arg("--root")
        .arg(root)
        .output()
        .expect("run build; install Node.js and run npm ci")
}

/// The frozen example rule with a tags property on line 10, column 3.
fn with_tags(tags: &str) -> String {
    let source = EXAMPLE.replace(
        "  remediation:",
        &format!("  tags: {tags},\n  remediation:"),
    );
    assert_ne!(source, EXAMPLE);
    source
}

fn schema(name: &str) -> Value {
    serde_json::from_str(&fs::read_to_string(Path::new(ROOT).join("schema").join(name)).unwrap())
        .unwrap()
}

fn marker_rule(id: &str) -> Value {
    json!({
        "id": id, "kind": "text", "paths": ["src/**/*.ts"],
        "pattern": { "kind": "literal", "value": "legacyMarker" },
        "remediation": "Remove the marker.",
        "fixtures": [{ "path": "src/app.ts", "content": "legacyMarker();\n", "expect": { "line": 1, "column": 1 } }]
    })
}

fn policy(rules: &[Value]) -> Vec<u8> {
    json!({ "version": 1, "rules": rules })
        .to_string()
        .into_bytes()
}

#[test]
fn tags_are_metadata_that_matching_fixtures_findings_and_baselines_ignore() {
    let repo = repo();
    let root = repo.path();
    let source = root.join(".rulecheck/rules/demo.rule.ts");
    write(
        root,
        "config/app.toml",
        "name = \"app\"\nlegacy_mode = true\n",
    );
    write(root, ".rulecheck/rules/demo.rule.ts", EXAMPLE);
    git(root, &["add", "-A"]);
    let plain = run(build(root), 0);
    fs::write(&source, with_tags("[\"domain/config\", \"adr/0001\"]")).unwrap();
    let tagged = run(build(root), 0);

    // The builder passes tags through as written; without them the output is unchanged.
    let field = "\"tags\":[\"domain/config\",\"adr/0001\"],";
    assert!(tagged.contains(field), "{tagged}");
    assert_eq!(tagged.replace(field, ""), plain);
    let value: Value = serde_json::from_str(&tagged).unwrap();
    assert_eq!(
        common::violations(&schema("policy-v1.json"), &value),
        Vec::<String>::new()
    );

    let finding =
        "config/app.toml:2:1: [config/no-legacy-mode] Remove legacy_mode or set it to false.\n";
    for args in [
        &["validate"][..],
        &["test"],
        &["list"],
        &["check"],
        &["check", "--all-matches"],
        &["baseline"],
    ] {
        let code = if args[0] == "check" { 1 } else { 0 };
        let with = run(cli(args, tagged.as_bytes(), root), code);
        assert_eq!(
            with,
            run(cli(args, plain.as_bytes(), root), code),
            "{args:?}"
        );
        if args[0] == "check" {
            assert_eq!(with, finding);
        }
    }
    // JSON findings are identical and never carry tags; only the policy digest differs.
    let json = |policy: &str| -> Value {
        serde_json::from_str(&run(
            cli(&["check", "--format", "json"], policy.as_bytes(), root),
            1,
        ))
        .unwrap()
    };
    let (plain_result, tagged_result) = (json(&plain), json(&tagged));
    assert_eq!(plain_result["findings"], tagged_result["findings"]);
    assert!(!tagged_result.to_string().contains("domain/config"));
    assert_ne!(
        plain_result["policy"]["sha256"],
        tagged_result["policy"]["sha256"]
    );
}

#[test]
fn schema_builder_and_evaluator_reject_the_same_tags() {
    let repo = repo();
    let root = repo.path();
    let policy_schema = schema("policy-v1.json");

    // The largest accepted list: 16 distinct tags of 64 bytes.
    let longest: Vec<String> = (0..16)
        .map(|n| format!("domain/{n:02}{}", "x".repeat(55)))
        .collect();
    assert!(longest.iter().all(|tag| tag.len() == 64));
    let mut rule = marker_rule("source/no-marker");
    rule["tags"] = json!(longest);
    let accepted = policy(&[rule]);
    let value: Value = serde_json::from_slice(&accepted).unwrap();
    assert_eq!(
        common::violations(&policy_schema, &value),
        Vec::<String>::new()
    );
    assert_eq!(run(cli(&["validate"], &accepted, root), 0), "");

    let too_many: Vec<String> = (0..17).map(|n| format!("adr/{n:04}")).collect();
    for (tags, error) in [
        (
            json!([]),
            "[source/no-marker] provide 1 to 16 tags or omit tags",
        ),
        (
            json!(too_many),
            "[source/no-marker] provide 1 to 16 tags or omit tags",
        ),
        (
            json!(["adr/0007", "domain/billing", "adr/0007"]),
            "[source/no-marker] tags lists \"adr/0007\" twice",
        ),
        (json!(["ADR-7"]), "[source/no-marker] invalid tag \"ADR-7\""),
        (
            json!(["adr//7"]),
            "[source/no-marker] invalid tag \"adr//7\"",
        ),
        (
            json!(["adr/7 "]),
            "[source/no-marker] invalid tag \"adr/7 \"",
        ),
        (json!([format!("domain/{}", "x".repeat(58))]), "invalid tag"),
        (json!("adr/0007"), "invalid type: string"),
        (json!([7]), "invalid type: integer"),
    ] {
        let mut rule = marker_rule("source/no-marker");
        rule["tags"] = tags.clone();
        let rejected = policy(&[rule]);
        let value: Value = serde_json::from_slice(&rejected).unwrap();
        assert!(
            !common::violations(&policy_schema, &value).is_empty(),
            "the schema accepts tags {tags}"
        );
        let stderr = run(cli(&["validate"], &rejected, root), 2);
        assert!(stderr.contains(error), "tags {tags}: {stderr}");
    }

    // The builder reports the same mistakes at the tag in the rule file.
    let source = root.join(".rulecheck/rules/demo.rule.ts");
    write(root, ".rulecheck/rules/demo.rule.ts", EXAMPLE);
    for (tags, message) in [
        (
            "[]",
            "10:3: rule tags must be an array of 1 to 16 tags. Change tags to an array of 1 to 16 tags.",
        ),
        (
            "\"adr/0007\"",
            "10:3: rule tags must be an array of 1 to 16 tags. Change tags to an array of 1 to 16 tags.",
        ),
        (
            "[\"adr/0007\", \"adr/0007\"]",
            "10:22: Duplicate tag \"adr/0007\" in tags. Remove the repeated tag.",
        ),
        (
            "[\"adr/0007\", \"ADR-7\"]",
            "10:22: tag 2 must be at most 64 lowercase letters, digits, hyphens and slashes. \
             Write a tag shaped like a rule ID, such as \"adr/0007\" or \"domain/billing\".",
        ),
    ] {
        fs::write(&source, with_tags(tags)).unwrap();
        let stderr = run(build(root), 2);
        assert_eq!(
            stderr,
            format!("rulecheck build: .rulecheck/rules/demo.rule.ts:{message}\n"),
            "tags {tags}"
        );
    }
}

#[test]
fn list_json_is_a_deterministic_inventory_and_human_list_is_unchanged() {
    let repo = repo();
    let root = repo.path();
    let mut billing = marker_rule("billing/no-float");
    billing["tags"] = json!(["domain/billing", "adr/0007"]);
    billing["provenance"] = json!({ "finding": FINDING });
    let mut reordered = billing.clone();
    reordered["tags"] = json!(["adr/0007", "domain/billing"]);
    let untagged = marker_rule("api/no-marker");
    let mut config = marker_rule("config/no-marker");
    config["kind"] = json!("forbidden-path");
    config.as_object_mut().unwrap().remove("pattern");
    config["paths"] = json!(["config/*.local.toml"]);
    config["tags"] = json!(["domain/config"]);
    config["fixtures"] =
        json!([{ "path": "config/dev.local.toml", "expect": { "line": 1, "column": 1 } }]);
    let bytes = policy(&[billing, untagged.clone(), config.clone()]);

    assert_eq!(
        run(cli(&["list"], &bytes, root), 0),
        "api/no-marker\nbilling/no-float\nconfig/no-marker\n"
    );
    let text = run(cli(&["list", "--format", "json"], &bytes, root), 0);
    assert_eq!(
        text,
        run(cli(&["list", "--format", "json"], &bytes, root), 0)
    );
    assert_eq!(
        run(cli(&["list", "--format", "human"], &bytes, root), 0),
        run(cli(&["list"], &bytes, root), 0)
    );
    let inventory: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(
        common::violations(&schema("inventory-v1.json"), &inventory),
        Vec::<String>::new()
    );
    let digest = format!(
        "sha256:{:x}",
        <sha2::Sha256 as sha2::Digest>::digest(&bytes)
    );
    assert_eq!(
        inventory,
        json!({
            "inventoryVersion": 1,
            "tools": { "evaluator": format!("rulecheck {}", env!("CARGO_PKG_VERSION")) },
            "policy": { "version": 1, "sha256": digest, "idPrefix": null },
            "rules": [
                { "id": "api/no-marker", "kind": "text", "tags": [], "provenance": null, "remediation": "Remove the marker." },
                {
                    "id": "billing/no-float", "kind": "text", "tags": ["adr/0007", "domain/billing"],
                    "provenance": { "finding": FINDING }, "remediation": "Remove the marker."
                },
                { "id": "config/no-marker", "kind": "forbidden-path", "tags": ["domain/config"], "provenance": null, "remediation": "Remove the marker." }
            ]
        })
    );

    // Tag order and rule order in the policy don't change the listed rules.
    let shuffled = policy(&[config, reordered, untagged]);
    let again: Value =
        serde_json::from_str(&run(cli(&["list", "--format", "json"], &shuffled, root), 0)).unwrap();
    assert_eq!(again["rules"], inventory["rules"]);

    // --id-prefix selects rules as it does for human output.
    let selected: Value = serde_json::from_str(&run(
        cli(
            &["list", "--format", "json", "--id-prefix", "billing/"],
            &bytes,
            root,
        ),
        0,
    ))
    .unwrap();
    assert_eq!(selected["policy"]["idPrefix"], "billing/");
    assert_eq!(selected["rules"], json!([inventory["rules"][1]]));

    // Bad arguments and policies never produce a document.
    for args in [
        &["list", "--format", "xml"][..],
        &["list", "--format", "json", "--format", "json"],
        &["validate", "--format", "json"],
        &["list", "--format", "json", "--id-prefix", "unknown/"],
    ] {
        let output = cli(args, &bytes, root);
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert!(output.stdout.is_empty(), "{args:?}");
    }
    let output = cli(
        &["list", "--format", "json"],
        b"{\"version\":1,\"rules\":[]}",
        root,
    );
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
}

#[test]
fn installed_list_json_feeds_the_coverage_report() {
    // An installation laid out like a checkout, with the evaluator under test.
    let dir = tempfile::tempdir().unwrap();
    let install = dir.path().join("rulecheck");
    for path in ["bin", "tools", "target/release"] {
        fs::create_dir_all(install.join(path)).unwrap();
    }
    for path in ["bin/rulecheck.mjs", "tools/build.mjs", "package.json"] {
        fs::copy(Path::new(ROOT).join(path), install.join(path)).unwrap();
    }
    std::os::unix::fs::symlink(
        Path::new(ROOT).join("node_modules"),
        install.join("node_modules"),
    )
    .unwrap();
    fs::copy(
        env!("CARGO_BIN_EXE_rulecheck-evaluator"),
        install.join("target/release/rulecheck-evaluator"),
    )
    .unwrap();

    let repo = repo();
    let root = repo.path();
    write(
        root,
        ".rulecheck/rules/demo.rule.ts",
        &with_tags("[\"domain/config\", \"adr/0001\"]"),
    );
    git(root, &["add", "-A"]);
    let wrapper = |args: &[&str]| {
        Command::new("node")
            .arg(install.join("bin/rulecheck.mjs"))
            .args(args)
            .arg("--root")
            .arg(root)
            .env_remove("RULECHECK_WRAPPER_ACTIVE")
            .output()
            .unwrap()
    };
    assert_eq!(run(wrapper(&["list"]), 0), "config/no-legacy-mode\n");
    let inventory = run(wrapper(&["list", "--format", "json"]), 0);
    let document: Value = serde_json::from_str(&inventory).unwrap();
    assert_eq!(
        common::violations(&schema("inventory-v1.json"), &document),
        Vec::<String>::new()
    );
    assert_eq!(
        document["rules"][0]["tags"],
        json!(["adr/0001", "domain/config"])
    );
    assert!(run(wrapper(&["test", "--format", "json"]), 2).contains("unexpected argument"));

    let coverage = |args: &[&str]| {
        let mut child = Command::new("node")
            .arg(Path::new(ROOT).join("tools/coverage.mjs"))
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(inventory.as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    };
    assert_eq!(
        run(
            coverage(&["--require", "adr/0001", "--require", "adr/0002"]),
            1
        ),
        "covered adr/0001: config/no-legacy-mode\n\
         missing adr/0002: no rule has this tag\n\
         1 of 2 required tags covered\n"
    );
    assert_eq!(
        run(coverage(&["--require", "domain/config"]), 0),
        "covered domain/config: config/no-legacy-mode\n1 of 1 required tags covered\n"
    );
    assert!(run(coverage(&["--require", "Domain"]), 2).contains("is not a tag"));
}
