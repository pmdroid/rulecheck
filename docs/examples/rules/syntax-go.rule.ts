import { defineRule } from "@rulecheck/sdk";

// The comment rule from the TypeScript recipe, for Go. Only the language, the
// grammar's function kinds and the directive spellings change. A tab counts as
// one column.
export default defineRule({
  id: "source/no-go-function-body-comments",
  kind: "syntax",
  paths: ["src/**/*.go"],
  selector: {
    language: "go",
    node: "comment",
    inside: { kinds: ["function_declaration", "method_declaration", "func_literal"], field: "body" },
    ignore: [
      // Explicit tool directives stay.
      { kind: "regex", value: "^//(?:go:|line |\\s*nolint\\b|\\s*lint:ignore\\b|\\s*#nosec\\b)" },
      // So do tracked work markers.
      { kind: "regex", value: "\\b(?:TODO|FIXME)\\b" },
    ],
  },
  remediation: "Move the explanation out of the function body or into a name.",
  fixtures: [
    { path: "src/app.go", content: "package app\n\nfunc f() int {\n\t// explain\n\treturn 1\n}\n", expect: { line: 4, column: 2 } },
    { path: "src/app.go", content: "package app\n\nfunc f(x int) int {\n\tif x > 0 {\n\t\tfor {\n\t\t\t/* deep */ break\n\t\t}\n\t}\n\treturn x\n}\n", expect: { line: 6, column: 4 } },
    { path: "src/app.go", content: "package app\n\nfunc f() func() {\n\treturn func() {\n\t\t// in a func literal\n\t}\n}\n", expect: { line: 5, column: 3 } },
    { path: "src/app.go", content: "package app\n\ntype T struct{}\n\nfunc (T) M() {\n\treturn\n\t// last line of the body\n}\n", expect: { line: 7, column: 2 } },
    { path: "src/app.go", content: "package app\n\nvar handler = func() {\n\t// package-level func literal\n}\n", expect: { line: 4, column: 2 } },
    { path: "src/app.go", content: "//go:build linux\n\n// Package app does things.\npackage app\n\n// f returns one.\n//\n//go:noinline\nfunc f() int {\n\treturn 1\n}\n\n/* after */\n", expect: null },
    { path: "src/app.go", content: "package app\n\nfunc f(a int /* parameter */) /* result */ int {\n\treturn a\n}\n\ntype T struct {\n\tA int // field\n}\n", expect: null },
    { path: "src/app.go", content: "package app\n\nfunc f() string {\n\ta := \"// not a comment\"\n\tb := `\n// nor this\n/* nor this */`\n\tc := '/'\n\treturn a + b + string(c)\n}\n", expect: null },
    { path: "src/app.go", content: "package app\n\nimport \"os\"\n\nfunc f() {\n\t//nolint:errcheck\n\tos.Remove(\"x\")\n\tos.Remove(\"y\") // #nosec G104\n\t//lint:ignore SA4006 kept for clarity\n\tx := 1\n\t_ = x\n}\n", expect: null },
    { path: "src/app.go", content: "package app\n\nfunc f() int {\n\t// TODO: split this up\n\t// FIXME(pas): wrong for 0\n\treturn 1\n}\n", expect: null },
    { path: "src/app.go", content: "package app\n\nfunc f() int {\n\t// TODO: later\n\tx := 1 // the real comment\n\treturn x\n}\n", expect: { line: 5, column: 9 } },
    { path: "scripts/build.go", content: "package main\n\nfunc main() {\n\t// out of scope\n}\n", expect: null },
  ],
});
