import { defineRule } from "@pmdroid/rulecheck";

export default defineRule({
  id: "config/no-debug-true",
  kind: "text",
  paths: ["config/*.toml"],
  pattern: { kind: "regex", value: "(?m)^[ \\t]*debug[ \\t]*=[ \\t]*true[ \\t]*(?:#.*)?$" },
  remediation: "Set debug to false before committing.",
  fixtures: [
    { path: "config/app.toml", content: "debug = false\n", expect: null },
    { path: "config/app.toml", content: "debug=true # temporary\n", expect: { line: 1, column: 1 } },
  ],
});
