import { defineRule } from "@rulecheck/sdk";

const marker = "OLD_NAME";

export default defineRule({
  id: "source/no-old-name",
  kind: "text",
  paths: ["packages/core/**/*.ts", "services/api/**/*.ts"],
  pattern: { kind: "literal", value: marker },
  remediation: "Use NEW_NAME instead.",
  fixtures: [
    { path: "packages/core/src/index.ts", content: "// π OLD_NAME OLD_NAME\n", expect: { line: 1, column: 6 } },
    { path: "services/api/index.ts", content: "// OLD_NAME\n", expect: { line: 1, column: 4 } },
    { path: "services/api/index.ts", content: "// old_name\n", expect: null },
    { path: "services/web/index.ts", content: "// OLD_NAME\n", expect: null },
  ],
});
