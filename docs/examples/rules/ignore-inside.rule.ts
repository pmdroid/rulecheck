import { defineRule } from "@pmdroid/rulecheck";

export default defineRule({
  id: "source/no-eval",
  kind: "text",
  paths: ["src/**/*.ts", "src/**/*.tsx"],
  pattern: { kind: "regex", value: "\\beval\\(" },
  // Skip a match that lies wholly inside a comment or a string literal.
  ignoreInside: { language: "typescript", contexts: ["comment", "string"] },
  remediation: "Parse the input instead of calling eval.",
  fixtures: [
    // Mentions in comments and strings are not calls.
    {
      path: "src/run.ts",
      content: "// never call eval(input)\nexport const banned = \"eval(\";\n",
      expect: null,
    },
    // A skipped mention does not hide the call after it on the same line.
    {
      path: "src/run.ts",
      content: "const s = \"eval(\"; eval(s);\n",
      expect: { line: 1, column: 20 },
    },
    // A template substitution is code, so its call is reported.
    {
      path: "src/run.ts",
      content: "const t = `eval(${eval(code)})`;\n",
      expect: { line: 1, column: 19 },
    },
    // With --all-matches every call outside comments and strings is reported.
    {
      path: "src/run.tsx",
      content: "/* eval(a) */ eval(b);\neval(c); // eval(d)\n",
      expectAll: [{ line: 1, column: 15 }, { line: 2, column: 1 }],
    },
  ],
});
