import { defineRule } from "@rulecheck/sdk";

export default defineRule({
  id: "source/no-legacy-marker",
  kind: "text",
  paths: ["src/**/*.ts"],
  pattern: { kind: "literal", value: "legacyMarker" },
  remediation: "Replace the legacy marker in source.",
  fixtures: [
    { path: "src/app.ts", content: "legacyMarker();\n", expect: { line: 1, column: 1 } },
    { path: "src/app.ts", content: "currentMarker();\n", expect: null },
    { path: "generated/app.ts", content: "legacyMarker();\n", expect: null },
  ],
});
