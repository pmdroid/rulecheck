import { defineRule } from "@pmdroid/rulecheck";

// Rust items and closures whose `body` field holds the function body. A
// comment counts when it sits anywhere inside that body, including nested
// blocks, closures, methods in an impl and macro arguments.
const functionBody = {
  kinds: ["function_item", "closure_expression"],
  field: "body",
};

export default defineRule({
  id: "source/no-rust-function-body-comments",
  kind: "syntax",
  paths: ["src/**/*.rs"],
  selector: {
    language: "rust",
    // The Rust grammar has two comment kinds. This rule reports `//` comments,
    // including `///` doc comments; a second rule with node "block_comment"
    // and the same `inside` covers `/* */`.
    node: "line_comment",
    inside: functionBody,
    ignore: [
      // Clippy's undocumented_unsafe_blocks lint requires a SAFETY comment.
      { kind: "regex", value: "^//\\s*SAFETY:" },
      // Tracked work markers stay.
      { kind: "regex", value: "\\b(?:TODO|FIXME)\\b" },
    ],
  },
  remediation: "Move the explanation out of the function body or into a name.",
  fixtures: [
    { path: "src/lib.rs", content: "fn f() -> i32 {\n    // explain\n    1\n}\n", expect: { line: 2, column: 5 } },
    { path: "src/lib.rs", content: "fn f(x: i32) {\n    if x > 0 {\n        loop {\n            // deep\n            break;\n        }\n    }\n}\n", expect: { line: 4, column: 13 } },
    { path: "src/lib.rs", content: "pub fn f(xs: &[i32]) -> Vec<i32> {\n    xs.iter()\n        .map(|x| {\n            // in a closure\n            x + 1\n        })\n        .collect()\n}\n", expect: { line: 4, column: 13 } },
    { path: "src/lib.rs", content: "struct A;\n\nimpl A {\n    // between items\n    fn run(&self) {\n        // in a method\n    }\n}\n", expect: { line: 6, column: 9 } },
    { path: "src/lib.rs", content: "fn f() -> Vec<i32> {\n    vec![\n        // in a macro\n        1,\n    ]\n}\n", expect: { line: 3, column: 9 } },
    { path: "src/lib.rs", content: "fn f() -> i32 {\n    let x = 1; // trailing\n    x\n}\n", expect: { line: 2, column: 16 } },
    { path: "src/lib.rs", content: "fn f() -> // return type\n    i32 {\n    1\n}\n", expect: null },
    { path: "src/lib.rs", content: "fn f() -> Result<\n    // inner\n    i32,\n    (),\n> {\n    Ok(1)\n}\n", expect: null },
    { path: "src/lib.rs", content: "fn f(\n    // parameter\n    x: i32,\n) -> i32 {\n    x\n}\n", expect: null },
    { path: "src/lib.rs", content: "//! Crate docs.\n\n/// Item docs.\n// top level\nfn f() {}\n", expect: null },
    { path: "src/lib.rs", content: "fn f() -> [&'static str; 3] {\n    let a = \"// not a comment\";\n    let b = r#\"// nor this\"#;\n    let c = \"\n// nor this line\n\";\n    [a, b, c]\n}\n", expect: null },
    { path: "src/lib.rs", content: "fn f(p: *const i32) -> i32 {\n    // SAFETY: the caller passes a valid pointer.\n    unsafe { *p }\n}\n", expect: null },
    { path: "src/lib.rs", content: "fn f() -> i32 {\n    // TODO: split this up\n    // FIXME(pas): wrong for 0\n    1\n}\n", expect: null },
    { path: "src/lib.rs", content: "fn f() -> i32 {\n    // TODO: later\n    let x = 1; // the real comment\n    x\n}\n", expect: { line: 3, column: 16 } },
    { path: "src/lib.rs", content: "fn f() -> i32 {\n    /* block */\n    1\n}\n", expect: null },
    { path: "tests/it.rs", content: "fn f() {\n    // out of scope\n}\n", expect: null },
  ],
});
