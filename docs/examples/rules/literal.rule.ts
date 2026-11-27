import { defineRule } from "@pmdroid/rulecheck";

export default defineRule({
  id: "config/no-legacy-spelling",
  kind: "text",
  paths: ["config/*.toml"],
  pattern: { kind: "literal", value: "legacy_mode = true" },
  remediation: "Remove the legacy setting or set it to false.",
  fixtures: [
    { path: "config/app.toml", content: "legacy_mode = false\n", expect: null },
    { path: "config/app.toml", content: "# settings\nlegacy_mode = true\n", expect: { line: 2, column: 1 } },
  ],
});
