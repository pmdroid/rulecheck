import { defineRule } from "@rulecheck/sdk";

export default defineRule({
  id: "source/no-console-log",
  kind: "text",
  paths: ["src/**/*.ts"],
  exclude: ["src/generated/**", "src/**/*.test.ts"],
  pattern: { kind: "literal", value: "console.log(" },
  remediation: "Use the project logger instead of console.log.",
  fixtures: [
    { path: "src/app.ts", content: "console.log(user);\n", expect: { line: 1, column: 1 } },
    { path: "src/generated/client.ts", content: "console.log(user);\n", expect: null },
    { path: "src/app.test.ts", content: "console.log(user);\n", expect: null },
    { path: "src/Generated/client.ts", content: "console.log(user);\n", expect: { line: 1, column: 1 } },
  ],
});
