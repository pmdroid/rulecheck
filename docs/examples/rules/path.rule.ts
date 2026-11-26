import { defineRule } from "@rulecheck/sdk";

export default defineRule({
  id: "config/no-local-overrides",
  kind: "forbidden-path",
  paths: ["config/*.override.toml"],
  remediation: "Move the local override outside the tracked checkout.",
  fixtures: [
    { path: "config/app.toml", expect: null },
    { path: "config/local.override.toml", expect: { line: 1, column: 1 } },
  ],
});
