import { defineRule } from "@pmdroid/rulecheck";

// JavaScript: /\bflag\((['"])beta-[\w-]+\1\)/
// The Rust regex dialect has no backreferences, so list each quote style.
export default defineRule({
  id: "source/no-beta-flags",
  kind: "text",
  paths: ["src/**/*.ts"],
  pattern: { kind: "regex", value: "\\bflag\\((?:'beta-[\\w-]+'|\"beta-[\\w-]+\")\\)" },
  remediation: "Ship the feature or remove the beta flag check.",
  fixtures: [
    { path: "src/app.ts", content: "if (flag('beta-search')) run();\n", expect: { line: 1, column: 5 } },
    { path: "src/app.ts", content: "if (flag(\"beta-search\")) run();\n", expect: { line: 1, column: 5 } },
    // Mismatched quotes are not a string literal, as with \1 in JavaScript.
    { path: "src/app.ts", content: "if (flag('beta-search\")) run();\n", expect: null },
    { path: "src/app.ts", content: "if (flag('search')) run();\n", expect: null },
    { path: "src/app.ts", content: "if (myflag('beta-search')) run();\n", expect: null },
  ],
});
