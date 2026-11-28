//! Runs a small adoption pilot (tools/pilot.mjs) through an installed wrapper: one
//! run and per-prefix runs report the same counts by rule and path as the
//! generator's oracle, the builder starts once per wrapper run, and the baseline
//! rejects a same-file addition, ignores a stopped scan and tightens on clean-up.
//! Timing is reported, not asserted.

use serde_json::Value;
use std::fs;
use std::os::unix::fs::symlink;
use std::path::Path;
use std::process::Command;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

#[test]
fn small_pilot_matches_its_oracle_and_ratchets_the_baseline() {
    // An installation laid out like a checkout, with the evaluator under test.
    let dir = tempfile::tempdir().unwrap();
    let install = dir.path().join("rulecheck");
    for path in ["bin", "tools", "target/release"] {
        fs::create_dir_all(install.join(path)).unwrap();
    }
    for path in ["bin/rulecheck.mjs", "tools/build.mjs", "package.json"] {
        fs::copy(Path::new(ROOT).join(path), install.join(path)).unwrap();
    }
    symlink(
        Path::new(ROOT).join("node_modules"),
        install.join("node_modules"),
    )
    .unwrap();
    fs::copy(
        env!("CARGO_BIN_EXE_rulecheck-evaluator"),
        install.join("target/release/rulecheck-evaluator"),
    )
    .unwrap();

    // 2 services of 5 x 256 KiB: 2.5 MiB, over the 1 MiB budget the stopped-scan
    // checks use, and with every seeded count from 0 to 3.
    let output = Command::new("node")
        .arg(Path::new(ROOT).join("tools/pilot.mjs"))
        .args([
            "--prefixes",
            "2",
            "--files",
            "5",
            "--file-kib",
            "256",
            "--runs",
            "1",
        ])
        .arg("--install")
        .arg(&install)
        .env_remove("RULECHECK_WRAPPER_ACTIVE")
        .output()
        .expect("run tools/pilot.mjs");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(0), "{stdout}\n{stderr}");
    let report: Value = serde_json::from_str(&stdout).unwrap();

    let failed: Vec<_> = report["checks"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|check| check["pass"] != true)
        .collect();
    assert!(failed.is_empty(), "{failed:#?}");
    let names: Vec<_> = report["checks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|check| check["name"].as_str().unwrap())
        .collect();
    for name in [
        "one run counts equal the oracle by rule and path",
        "per-prefix counts equal the oracle by rule and path",
        "--commit counts equal the oracle by rule and path",
        "recorded baseline equals the oracle by rule and path",
        "same file has two findings of one rule",
        "one additional finding in the same file fails",
        "an incomplete scan cannot pass",
        "an incomplete scan cannot record or change the allowance",
        "the clean-up lowers only that path's count by one",
        "strict mode passes with the tightened baseline",
    ] {
        assert!(names.contains(&name), "missing check {name:?} in {names:?}");
    }
    assert_eq!(report["passed"], true);

    // 2 services and shared/: 7 rules, 3 prefixes. Seeds per service: debug
    // 0+1+2+0+3, fixme in files 0 and 3, one token in the first service, and
    // one scratch file in the first service.
    assert_eq!(report["snapshot"]["prefixes"], 3);
    assert_eq!(report["snapshot"]["rules"], 7);
    assert_eq!(report["snapshot"]["findings"], 2 * (6 + 2) + 1 + 1);
    assert_eq!(report["snapshot"]["scopedTextBytes"], 10 * 256 * 1024);
    assert_eq!(report["oneRun"]["invocations"], 1);
    assert_eq!(report["oneRun"]["builderStarts"], 1);
    assert_eq!(report["oneRun"]["completed"], true);
    assert_eq!(report["perPrefix"]["invocations"], 3);
    assert_eq!(report["perPrefix"]["builderStarts"], 3);
    assert_eq!(report["perPrefix"]["completed"], true);
    // shared/ reads every service again.
    assert_eq!(
        report["perPrefix"]["scopedTextBytesRead"],
        2 * 10 * 256 * 1024
    );
    assert_eq!(
        report["sameFileTarget"]["path"],
        "services/svc01/src/0002.txt"
    );
}
