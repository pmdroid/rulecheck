//! Pins how Rulecheck explains JavaScript regex constructs without changing
//! the Rust regex dialect: compile errors gain a porting hint, a failing
//! fixture of a pattern with a nested class or class set operator gains a
//! note, and every pattern keeps its Rust meaning. The cases also back the
//! porting table in docs/src/content/docs/authoring.md. All examples are
//! synthetic.
use serde_json::{Value, json};
use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn cli(args: &[&str], policy: &Value) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_rulecheck-evaluator"))
        .current_dir(ROOT)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("run rulecheck");
    let _ = child
        .stdin
        .take()
        .unwrap()
        .write_all(policy.to_string().as_bytes());
    child.wait_with_output().unwrap()
}

fn text(output: &Output) -> (String, String) {
    (
        String::from_utf8(output.stdout.clone()).unwrap(),
        String::from_utf8(output.stderr.clone()).unwrap(),
    )
}

fn rule(id: &str, pattern: &str, fixtures: Value) -> Value {
    json!({
        "id": id,
        "kind": "text",
        "paths": ["src/**/*.ts"],
        "remediation": "Replace the operator.",
        "pattern": { "kind": "regex", "value": pattern },
        "fixtures": fixtures,
    })
}

fn at(line: usize, column: usize) -> Value {
    json!({ "line": line, "column": column })
}

fn fixture(content: &str, expect: Value) -> Value {
    json!({ "path": "src/app.ts", "content": content, "expect": expect })
}

fn policy(rules: Vec<Value>) -> Value {
    json!({ "version": 1, "rules": rules })
}

#[test]
fn javascript_only_constructs_fail_with_a_porting_hint() {
    let look = "hint: the Rust regex dialect has no look-around.";
    let backref = "hint: the Rust regex dialect has no backreferences";
    let class = "hint: in the Rust regex dialect [ inside a character class opens a nested class";
    let cases = [
        ("legacy(?=_mode)", "look-around", look),
        ("legacy(?!_ok)", "look-around", look),
        ("(?<=\\.)run\\(", "look-around", look),
        ("(?<!\\.)run\\(", "look-around", look),
        ("(['\"])x\\1", "backreferences are not supported", backref),
        (
            "(?<q>['\"])x\\k<q>",
            "unrecognized escape sequence",
            backref,
        ),
        ("[^[]", "unclosed character class", class),
        ("[[\\]]", "unclosed character class", class),
        ("[^]", "unclosed character class", class),
    ];
    for (pattern, error, hint) in cases {
        let output = cli(
            &["validate"],
            &policy(vec![rule(
                "a/port",
                pattern,
                json!([fixture("", Value::Null)]),
            )]),
        );
        let (_, stderr) = text(&output);
        assert_eq!(output.status.code(), Some(2), "{pattern}: {stderr}");
        assert!(
            stderr.starts_with("rulecheck: [a/port] invalid regex: regex parse error:\n"),
            "{pattern}: {stderr}"
        );
        assert!(
            stderr.contains(&format!("error: {error}")),
            "{pattern}: {stderr}"
        );
        assert!(stderr.contains(&format!("\n{hint}")), "{pattern}: {stderr}");
    }

    // Other syntax errors keep the regex crate's message alone.
    let output = cli(
        &["validate"],
        &policy(vec![rule(
            "a/port",
            "a{,3}",
            json!([fixture("", Value::Null)]),
        )]),
    );
    let (_, stderr) = text(&output);
    assert_eq!(output.status.code(), Some(2), "{stderr}");
    assert!(stderr.contains("repetition quantifier"), "{stderr}");
    assert!(!stderr.contains("hint:"), "{stderr}");

    // A requires regex and a skipIfContent regex get the same hint.
    let mut requires = rule("a/requires", "start\\(", json!([fixture("", Value::Null)]));
    requires["requires"] = json!({ "kind": "regex", "value": "stop(?=\\()" });
    let mut skip = rule("a/skip", "start\\(", json!([fixture("", Value::Null)]));
    skip["skipIfContent"] = json!({ "kind": "regex", "value": "(['\"])gen\\1", "firstBytes": 64 });
    for (rule, prefix, hint) in [
        (
            requires,
            "rulecheck: [a/requires] requires: invalid regex: regex parse error:",
            look,
        ),
        (
            skip,
            "rulecheck: [a/skip] invalid skipIfContent regex: regex parse error:",
            backref,
        ),
    ] {
        let output = cli(&["validate"], &policy(vec![rule]));
        let (_, stderr) = text(&output);
        assert_eq!(output.status.code(), Some(2), "{stderr}");
        assert!(stderr.starts_with(prefix), "{stderr}");
        assert!(stderr.contains(&format!("\n{hint}")), "{stderr}");
    }

    // The same hint applies to a syntax selector's ignore regex.
    let syntax = json!({
        "id": "a/syntax",
        "kind": "syntax",
        "paths": ["src/**/*.ts"],
        "remediation": "Remove the comment.",
        "selector": {
            "language": "typescript",
            "node": "comment",
            "ignore": [{ "kind": "regex", "value": "TODO(?!:)" }],
        },
        "fixtures": [fixture("// c\n", at(1, 1))],
    });
    let output = cli(&["validate"], &policy(vec![syntax]));
    let (_, stderr) = text(&output);
    assert_eq!(output.status.code(), Some(2), "{stderr}");
    assert!(
        stderr
            .starts_with("rulecheck: [a/syntax] invalid selector ignore regex: regex parse error:"),
        "{stderr}"
    );
    assert!(stderr.contains(&format!("\n{look}")), "{stderr}");
}

#[test]
fn documented_look_ahead_error_is_exact() {
    let output = cli(
        &["validate"],
        &policy(vec![rule(
            "source/no-console-output",
            "\\bconsole\\.(?!error\\b)\\w+\\(",
            json!([fixture("", Value::Null)]),
        )]),
    );
    let (_, stderr) = text(&output);
    assert_eq!(output.status.code(), Some(2));
    let expected = "\
rulecheck: [source/no-console-output] invalid regex: regex parse error:
    \\bconsole\\.(?!error\\b)\\w+\\(
               ^^^
error: look-around, including look-ahead and look-behind, is not supported
hint: the Rust regex dialect has no look-around. Match a token edge with \\b, \\b{start} or \\b{end}; consume the next character, as in name(?:[^_]|$), instead of a look-ahead; or move the condition into paths, exclude or a syntax selector
";
    assert_eq!(stderr, expected);
    let docs = std::fs::read_to_string(Path::new(ROOT).join("docs/src/content/docs/authoring.md"))
        .unwrap();
    assert!(docs.contains(expected.trim_end()), "authoring.md is stale");
}

/// Fixture content and the expected first match, as (line, column).
type Case = (&'static str, Option<(usize, usize)>);

/// Patterns that compile in both dialects keep their Rust meaning, and the
/// rewrites in the porting table match what the JavaScript pattern matched.
#[test]
fn rust_semantics_and_documented_rewrites_hold() {
    let cases: &[(&str, &[Case])] = &[
        // Set operators and nested classes are Rust set operations.
        ("[a&&b]", &[("a&b", None)]),
        ("[a--b]", &[("-a", Some((1, 2))), ("b", None)]),
        ("[a~~b]", &[("~ab", Some((1, 2)))]),
        ("[[a]]", &[("[a]", Some((1, 2))), ("[]", None)]),
        ("[[a-z]&&[^aeiou]]", &[("ab", Some((1, 2)))]),
        // Escaped, they are the literal characters JavaScript reads.
        ("[a\\&\\&b]", &[("&", Some((1, 1)))]),
        ("[a\\-\\-b]", &[("-", Some((1, 1)))]),
        ("[a\\~\\~b]", &[("~", Some((1, 1)))]),
        (
            "[\\[a]\\]",
            &[("[]", Some((1, 1))), ("xa]", Some((1, 2))), ("a", None)],
        ),
        ("[^\\[]", &[("[", None), ("[x", Some((1, 2)))]),
        ("[\\[\\]]", &[("x]", Some((1, 2)))]),
        ("(?s:.)", &[("\n", Some((1, 1)))]),
        ("a\\{,3\\}", &[("a{,3}", Some((1, 1)))]),
        ("a{0,3}b", &[("aab", Some((1, 1)))]),
        // Unicode classes and boundaries, and their ASCII forms.
        ("\\d", &[("\u{663}", Some((1, 1)))]),
        ("[0-9]", &[("\u{663}", None)]),
        ("\\w", &[("é", Some((1, 1)))]),
        ("(?-u:\\w)", &[("é", None)]),
        ("\\bfoo", &[("éfoo", None)]),
        ("(?-u:\\b)foo", &[("éfoo", Some((1, 2)))]),
        (
            "\\b{start}foo\\b{end}",
            &[("a foo b", Some((1, 3))), ("foobar", None)],
        ),
        (".", &[("\r", Some((1, 1)))]),
        ("[^\\n\\r\\u{2028}\\u{2029}]", &[("\r\u{2028}", None)]),
    ];
    let mut rules = Vec::new();
    let mut count = 0;
    for (index, (pattern, fixtures)) in cases.iter().enumerate() {
        let fixtures: Vec<Value> = fixtures
            .iter()
            .map(|(content, expect)| {
                count += 1;
                fixture(
                    content,
                    expect.map_or(Value::Null, |(line, column)| at(line, column)),
                )
            })
            .collect();
        rules.push(rule(&format!("a/case-{index}"), pattern, json!(fixtures)));
    }
    let output = cli(&["test"], &policy(rules));
    let (stdout, stderr) = text(&output);
    assert_eq!(output.status.code(), Some(0), "{stdout}{stderr}");
    assert_eq!(stdout, format!("{count} fixtures passed\n"));
}

#[test]
fn failing_fixture_explains_nested_classes_and_set_operators() {
    let failing = |pattern: &str| {
        let output = cli(
            &["test"],
            &policy(vec![rule(
                "source/no-and-operator",
                pattern,
                json!([fixture("a&&b", at(1, 1))]),
            )]),
        );
        let (stdout, stderr) = text(&output);
        assert_eq!(output.status.code(), Some(1), "{stdout}{stderr}");
        stdout
    };

    let documented = "src/app.ts:1:1: [source/no-and-operator] fixture 1 expected a violation at 1:1, found no violation. Replace the operator. Update the rule or fixture. This pattern uses the && set operator inside a character class, which the Rust regex dialect applies as set operations but JavaScript without the v flag reads as literal characters; to match the operator characters literally, escape each one with a backslash.\n";
    assert_eq!(failing("[&&|]{2}"), documented);
    let docs = std::fs::read_to_string(Path::new(ROOT).join("docs/src/content/docs/authoring.md"))
        .unwrap();
    assert!(
        docs.contains(documented.trim_end()),
        "authoring.md is stale"
    );

    assert!(failing("[[x]--[y]]").ends_with(
        " This pattern uses a nested character class and the -- set operator inside a \
             character class, which the Rust regex dialect applies as set operations but \
             JavaScript without the v flag reads as literal characters; to match [ and the \
             operator characters literally, escape each one with a backslash.\n"
    ));
    assert!(failing("[[x]]").contains("to match [ literally"));
    assert!(failing("[x&&y]|[x~~z]").contains("uses the && set operator and the ~~ set operator"));

    // No class construct, no note: the line ends as before.
    assert!(failing("x").ends_with("Replace the operator. Update the rule or fixture.\n"));
    // An escaped operator is a literal and passes, so there is nothing to explain.
    let output = cli(
        &["test"],
        &policy(vec![rule(
            "source/no-and-operator",
            "[\\&]\\&",
            json!([fixture("a&&b", at(1, 2))]),
        )]),
    );
    assert_eq!(text(&output).0, "1 fixtures passed\n");

    // A requires regex gets the same note: here it never matches, so the
    // rule reports a trigger that JavaScript would have found satisfied.
    let mut requires = rule(
        "source/no-and-operator",
        "start\\(",
        json!([fixture("start(); a&b", Value::Null)]),
    );
    requires["requires"] = json!({ "kind": "regex", "value": "[a&&b]" });
    let output = cli(&["test"], &policy(vec![requires]));
    let (stdout, _) = text(&output);
    assert_eq!(output.status.code(), Some(1), "{stdout}");
    assert!(
        stdout.contains("Update the rule or fixture. This pattern uses the && set operator"),
        "{stdout}"
    );

    // A syntax rule's ignore regex gets the same note.
    let syntax = json!({
        "id": "a/syntax",
        "kind": "syntax",
        "paths": ["src/**/*.ts"],
        "remediation": "Remove the comment.",
        "selector": {
            "language": "typescript",
            "node": "comment",
            "ignore": [{ "kind": "literal", "value": "keep" }, { "kind": "regex", "value": "[a&&b]" }],
        },
        "fixtures": [fixture("// a&b\n", Value::Null)],
    });
    let output = cli(&["test"], &policy(vec![syntax]));
    let (stdout, _) = text(&output);
    assert_eq!(output.status.code(), Some(1), "{stdout}");
    assert!(stdout.contains("found a violation at 1:1. Remove the comment. Update the rule or fixture. This pattern uses the && set operator"), "{stdout}");
}

/// Explaining a pattern adds one bounded parse: patterns are at most 4096
/// bytes and 64 levels deep, so the largest allowed patterns load at once,
/// and deeper nesting fails explicitly.
#[test]
fn class_analysis_is_bounded() {
    let wide = "[[a]&&[b]]".repeat(409);
    let mut deep = "[".repeat(63);
    deep.push('a');
    deep.push_str(&"]".repeat(63));
    let started = Instant::now();
    for pattern in [&wide, &deep] {
        assert!(pattern.len() <= 4096);
        let output = cli(
            &["test"],
            &policy(vec![rule(
                "a/wide",
                pattern,
                json!([fixture("a", Value::Null)]),
            )]),
        );
        assert!(matches!(output.status.code(), Some(0 | 1)), "{output:?}");
    }
    assert!(started.elapsed() < Duration::from_secs(20));

    let mut deeper = "[".repeat(80);
    deeper.push('a');
    deeper.push_str(&"]".repeat(80));
    let output = cli(
        &["validate"],
        &policy(vec![rule(
            "a/deep",
            &deeper,
            json!([fixture("a", Value::Null)]),
        )]),
    );
    let (_, stderr) = text(&output);
    assert_eq!(output.status.code(), Some(2), "{stderr}");
    assert!(stderr.contains("maximum number of nested"), "{stderr}");
    assert!(!stderr.contains("hint:"), "{stderr}");
}
