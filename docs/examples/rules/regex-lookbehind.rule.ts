import { defineRule } from "@pmdroid/rulecheck";

// JavaScript: /(?<![.\w$])parseInt\(/
// The Rust regex dialect has no look-behind, so consume the character before
// the name. A finding then starts one column early, at that character, unless
// the name starts the line. \n is excluded so a match never starts on the line above.
export default defineRule({
  id: "source/no-global-parse-int",
  kind: "text",
  paths: ["src/**/*.ts"],
  pattern: { kind: "regex", value: "(?m)(?:^|[^.\\w$\\n])parseInt\\(" },
  remediation: "Use Number.parseInt with an explicit radix.",
  fixtures: [
    { path: "src/app.ts", content: "const n = Number.parseInt(text, 10);\n", expect: null },
    { path: "src/app.ts", content: "const n = $parseInt(text);\n", expect: null },
    { path: "src/app.ts", content: "const n = myparseInt(text);\n", expect: null },
    // The name is at column 11; the finding starts at the space before it.
    { path: "src/app.ts", content: "const n = parseInt(text);\n", expect: { line: 1, column: 10 } },
    { path: "src/app.ts", content: "run();\nparseInt(text);\n", expect: { line: 2, column: 1 } },
  ],
});
