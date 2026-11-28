import { defineRule } from "@pmdroid/rulecheck";

export default defineRule({
  id: "source/interval-needs-clear",
  kind: "text",
  paths: ["src/**/*.ts"],
  // The trigger: findings point at these matches.
  pattern: { kind: "regex", value: "\\bsetInterval\\(" },
  // The requirement: text that must appear somewhere in the same file.
  requires: { kind: "literal", value: "clearInterval(" },
  remediation: "Keep the interval handle and call clearInterval when the work stops.",
  fixtures: [
    // No trigger: nothing is required.
    { path: "src/clock.ts", content: "export const now = () => Date.now();\n", expect: null },
    // Trigger and requirement in the same file, in either order.
    {
      path: "src/poll.ts",
      content: "const id = setInterval(poll, 1000);\nexport const stop = () => clearInterval(id);\n",
      expect: null,
    },
    {
      path: "src/poll.ts",
      content: "export const stop = () => clearInterval(id);\nconst id = setInterval(poll, 1000);\n",
      expect: null,
    },
    // Trigger without the requirement: the first trigger match is reported.
    {
      path: "src/poll.ts",
      content: "// polls twice\nsetInterval(poll, 1000);\nsetInterval(flush, 5000);\n",
      expect: { line: 2, column: 1 },
    },
    // With --all-matches every trigger match is reported.
    {
      path: "src/poll.ts",
      content: "// polls twice\nsetInterval(poll, 1000);\nsetInterval(flush, 5000);\n",
      expectAll: [{ line: 2, column: 1 }, { line: 3, column: 1 }],
    },
  ],
});
