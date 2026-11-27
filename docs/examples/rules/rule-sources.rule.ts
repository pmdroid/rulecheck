import { defineRule } from "@pmdroid/rulecheck";

export default defineRule({
  id: "source/no-debugger",
  kind: "text",
  paths: ["**/*.ts"],
  exclude: [".rulecheck/**"],
  pattern: { kind: "literal", value: "debugger;" },
  remediation: "Remove the debugger statement.",
  fixtures: [
    { path: "src/app.ts", content: "debugger;\n", expect: { line: 1, column: 1 } },
    { path: "scripts/release.ts", content: "debugger;\n", expect: { line: 1, column: 1 } },
    { path: ".rulecheck/rules/rule-sources.rule.ts", content: "debugger;\n", expect: null },
  ],
});
