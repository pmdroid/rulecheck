import { defineRule } from "@pmdroid/rulecheck";

export default defineRule({
  id: "docs/flags-match-config",
  kind: "consistency",
  // The selected inventory. Both scopes pick files inside it.
  paths: ["docs/flags.md", "config/flags/*.json"],
  consistency: {
    form: "same-values",
    // Each documented flag is a list item such as "- `beta_search`: ...".
    left: { name: "documented", paths: ["docs/flags.md"], extract: "(?m)^- `([a-z0-9_]+)`" },
    // Each configured flag is a key such as "beta_search": true.
    right: { name: "configured", paths: ["config/flags/*.json"], extract: "\"([a-z0-9_]+)\"\\s*:" },
  },
  remediation: "Document every configured flag in docs/flags.md and remove entries for flags that are gone.",
  fixtures: [
    // The same flags on both sides, in any order and split across files: no finding.
    {
      files: [
        { path: "docs/flags.md", content: "# Flags\n\n- `beta_search`: new search.\n- `dark_mode`: theme.\n" },
        { path: "config/flags/a.json", content: "{ \"dark_mode\": false }\n" },
        { path: "config/flags/b.json", content: "{ \"beta_search\": true, \"dark_mode\": true }\n" },
      ],
      expectFindings: [],
    },
    // A flag that is configured but not documented is reported where it is
    // configured, and a documented flag that no file configures is reported in the docs.
    {
      files: [
        { path: "docs/flags.md", content: "# Flags\n\n- `beta_search`: new search.\n- `old_banner`: retired.\n" },
        { path: "config/flags/a.json", content: "{\n  \"beta_search\": true,\n  \"dark_mode\": false\n}\n" },
      ],
      expectFindings: [
        { path: "config/flags/a.json", line: 3, column: 4 },
        { path: "docs/flags.md", line: 4, column: 4 },
      ],
    },
  ],
});
