import { defineRule } from "@pmdroid/rulecheck";

export default defineRule({
  id: "config/no-legacy-settings",
  kind: "text",
  paths: ["config/*.toml"],
  pattern: {
    kind: "regex",
    value: "(?m)^[ \\t]*(?<key>legacy_[a-z_]+)[ \\t]*=[ \\t]*(?<value>[^\\r\\n#]*[^\\s#])",
  },
  // Rendered for each finding from that match's named groups.
  message: "{key} is set to {value}.",
  remediation: "Remove legacy_* settings; the defaults replace them.",
  fixtures: [
    { path: "config/app.toml", content: "cache = true\n", expectAll: [] },
    {
      path: "config/app.toml",
      content: "legacy_cache = true\nname = \"app\"\nlegacy_port = 8080 # old\n",
      expectAll: [{ line: 1, column: 1 }, { line: 3, column: 1 }],
    },
  ],
});
