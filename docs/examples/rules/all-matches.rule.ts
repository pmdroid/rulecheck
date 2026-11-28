import { defineRule } from "@pmdroid/rulecheck";

export default defineRule({
  id: "source/no-todo-markers",
  kind: "text",
  paths: ["src/**/*.ts"],
  pattern: { kind: "regex", value: "\\b(?:TODO|FIXME)\\b" },
  remediation: "Resolve the marker or move it to the issue tracker.",
  fixtures: [
    { path: "src/app.ts", content: "export const ready = true;\n", expectAll: [] },
    // The first match, as rulecheck check reports it by default.
    { path: "src/app.ts", content: "// TODO: π FIXME\nrun(); // TODO\n", expect: { line: 1, column: 4 } },
    // Every match, as rulecheck check --all-matches reports it.
    {
      path: "src/app.ts",
      content: "// TODO: π FIXME\nrun(); // TODO\n",
      expectAll: [{ line: 1, column: 4 }, { line: 1, column: 12 }, { line: 2, column: 11 }],
    },
  ],
});
