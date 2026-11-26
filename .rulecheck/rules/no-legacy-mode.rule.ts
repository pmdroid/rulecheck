import { defineRule } from "@rulecheck/sdk";

// A line-anchored text check, not a TOML parser: it matches a bare
// `legacy_mode = true` assignment with any spaces or tabs around `=`, an
// optional trailing comment and LF or CRLF endings. Comments and strings that
// only mention the setting do not match.
const legacySetting = "(?m)^[ \\t]*legacy_mode[ \\t]*=[ \\t]*true[ \\t]*(?:#[^\\r\\n]*)?\\r?$";

export default defineRule({
  id: "config/no-legacy-mode",
  kind: "text",
  paths: ["config/*.toml"],
  pattern: { kind: "regex", value: legacySetting },
  remediation: "Remove legacy_mode or set it to false.",
  provenance: { finding: "https://linear.app/kyku/issue/PAS-2456#comment-667ffed4-9034-4b80-941d-fd39feec836c" },
  fixtures: [
    { path: "config/app.toml", content: "legacy_mode = false\n", expect: null },
    { path: "config/app.toml", content: "legacy_mode=false\n", expect: null },
    { path: "config/app.toml", content: "# legacy_mode = true\n", expect: null },
    { path: "config/app.toml", content: "note = \"legacy_mode = true\"\n", expect: null },
    { path: "config/app.toml", content: "legacy_mode=true\n", expect: { line: 1, column: 1 } },
    { path: "config/app.toml", content: "name = \"app\"\nlegacy_mode = true\n", expect: { line: 2, column: 1 } },
    { path: "config/app.toml", content: "\tlegacy_mode\t=\ttrue # old mode\r\n", expect: { line: 1, column: 1 } },
  ],
});
