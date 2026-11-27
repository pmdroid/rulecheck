import { defineRule } from "@rulecheck/sdk";

// The same function-body selector as the comment recipe, with a different
// node kind. No evaluator change is needed for a new syntax rule.
const functionBody = {
  kinds: [
    "function_declaration",
    "function_expression",
    "generator_function_declaration",
    "generator_function",
    "arrow_function",
    "method_definition",
  ],
  field: "body",
};

export default defineRule({
  id: "source/no-non-null-assertion",
  kind: "syntax",
  paths: ["src/**/*.ts", "src/**/*.tsx"],
  selector: { language: "typescript", node: "non_null_expression", inside: functionBody },
  remediation: "Check for null or undefined instead of asserting with !.",
  fixtures: [
    { path: "src/app.ts", content: "function f(m: Map<string, number>) {\n  return m.get(\"a\")!;\n}\n", expect: { line: 2, column: 10 } },
    { path: "src/app.tsx", content: "export const App = (p: { n?: string }) => <p>{p.n!}</p>;\n", expect: { line: 1, column: 47 } },
    { path: "src/app.ts", content: "function f(a: number, b: number) {\n  return a !== b && !a;\n}\n", expect: null },
    { path: "src/app.ts", content: "function f() {\n  // value!\n  return \"done!\";\n}\n", expect: null },
    { path: "src/app.ts", content: "class A {\n  name!: string;\n}\n", expect: null },
  ],
});
