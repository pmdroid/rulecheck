import { defineRule } from "@pmdroid/rulecheck";

// Go functions, methods and function literals whose `body` field holds the
// function body. A comment counts when it sits anywhere inside that body,
// including nested blocks and closures.
const functionBody = {
  kinds: ["function_declaration", "method_declaration", "func_literal"],
  field: "body",
};

export default defineRule({
  id: "source/no-go-function-body-comments",
  kind: "syntax",
  paths: ["**/*.go"],
  exclude: ["vendor/**"],
  selector: {
    language: "go",
    // The Go grammar has one comment kind for both `//` and `/* */`.
    node: "comment",
    inside: functionBody,
    ignore: [
      // Compiler and linter directives stay.
      { kind: "regex", value: "^//(?:go:|nolint\\b|lint:)" },
      // So do tracked work markers.
      { kind: "regex", value: "\\b(?:TODO|FIXME)\\b" },
    ],
  },
  remediation: "Move the explanation out of the function body or into a name.",
  fixtures: [
    { path: "report/total.go", content: "package report\n\nfunc f() int {\n\t// explain\n\treturn 1\n}\n", expect: { line: 4, column: 2 } },
    { path: "report/total.go", content: "package report\n\nfunc f() int {\n\t/* explain */\n\treturn 1\n}\n", expect: { line: 4, column: 2 } },
    { path: "report/total.go", content: "package report\n\nfunc f(xs []int) int {\n\tfor _, x := range xs {\n\t\tif x > 0 {\n\t\t\t// deep\n\t\t\treturn x\n\t\t}\n\t}\n\treturn 0\n}\n", expect: { line: 6, column: 4 } },
    { path: "report/total.go", content: "package report\n\ntype T struct{}\n\nfunc (T) Run() {\n\t// in a method\n}\n", expect: { line: 6, column: 2 } },
    { path: "report/total.go", content: "package report\n\nvar g = func() {\n\t// in a literal\n}\n", expect: { line: 4, column: 2 } },
    { path: "report/total.go", content: "package report\n\nfunc f() int {\n\tx := 1 // trailing\n\treturn x\n}\n", expect: { line: 4, column: 9 } },
    { path: "report/total.go", content: "package report\n\nfunc f(\n\t// parameter\n\tx int,\n) /* result */ int {\n\treturn x\n}\n", expect: null },
    { path: "report/total.go", content: "// Package report sums values.\npackage report\n\n// F does nothing.\nfunc F() {}\n\n/* top level */\n", expect: null },
    { path: "report/total.go", content: "package report\n\nfunc f() string {\n\ta := \"// not a comment\"\n\tb := `/* nor this\n// nor this line */`\n\tc := '/'\n\treturn a + b + string(c)\n}\n", expect: null },
    { path: "report/total.go", content: "package report\n\nimport \"os\"\n\nfunc f() {\n\tos.Remove(\"x\") //nolint:errcheck\n\t// TODO: handle the error\n}\n", expect: null },
    { path: "report/total.go", content: "package report\n\nfunc f() int {\n\t// FIXME: wrong for 0\n\tx := 1 // the real comment\n\treturn x\n}\n", expect: { line: 5, column: 9 } },
    { path: "vendor/lib/lib.go", content: "package lib\n\nfunc f() {\n\t// out of scope\n}\n", expect: null },
  ],
});
