use std::process::Command;

#[test]
fn displays_help() {
    let output = Command::new(env!("CARGO_BIN_EXE_rulecheck"))
        .arg("--help")
        .output()
        .expect("run rulecheck");
    assert!(output.status.success());
    assert!(
        String::from_utf8(output.stdout)
            .unwrap()
            .contains("rule evaluation is not implemented")
    );
}

#[test]
fn displays_version() {
    let output = Command::new(env!("CARGO_BIN_EXE_rulecheck"))
        .arg("--version")
        .output()
        .expect("run rulecheck");
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!("rulecheck {}\n", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn unimplemented_check_is_not_a_success() {
    let output = Command::new(env!("CARGO_BIN_EXE_rulecheck"))
        .arg("check")
        .output()
        .expect("run rulecheck");
    assert_eq!(output.status.code(), Some(2));
}
