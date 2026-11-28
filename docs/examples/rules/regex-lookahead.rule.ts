import { defineRule } from "@pmdroid/rulecheck";

// JavaScript: /\bconsole\.(?!error\b)\w+\(/
// The Rust regex dialect has no look-ahead, so name the banned methods instead
// of excluding the allowed one. The match starts at the same place.
export default defineRule({
  id: "source/no-console-output",
  kind: "text",
  paths: ["src/**/*.ts"],
  pattern: { kind: "regex", value: "\\bconsole\\.(?:log|info|warn|debug|trace|table)\\(" },
  remediation: "Use the project logger; console.error is allowed for fatal errors.",
  fixtures: [
    { path: "src/app.ts", content: "console.error(failure);\n", expect: null },
    // The JavaScript original reports a method it doesn't know; a list of banned methods doesn't.
    { path: "src/app.ts", content: "console.errorCount();\n", expect: null },
    { path: "src/app.ts", content: "run();\n  console.warn(note);\n", expect: { line: 2, column: 3 } },
    { path: "src/app.ts", content: "myconsole.log(note);\n", expect: null },
  ],
});
