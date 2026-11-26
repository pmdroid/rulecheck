import { defineRule } from "@rulecheck/sdk";

const legacySetting = "legacy_mode = true";

export default defineRule({
  id: "config/no-legacy-mode",
  kind: "text",
  paths: ["config/*.toml"],
  pattern: { kind: "literal", value: legacySetting },
  remediation: "Remove legacy_mode or set it to false.",
  fixtures: [
    { path: "config/app.toml", content: "legacy_mode = false\n", expect: null },
    {
      path: "config/app.toml",
      content: "name = \"app\"\nlegacy_mode = true\n",
      expect: { line: 2, column: 1 },
    },
  ],
});
