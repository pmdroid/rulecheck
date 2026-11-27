import { defineRule } from "@rulecheck/sdk";

// Function-like nodes in the TypeScript and TSX grammars whose `body` field
// holds the function body. A comment counts when it sits anywhere inside that
// body, including nested blocks, callbacks and JSX.
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
  id: "source/no-function-body-comments",
  kind: "syntax",
  paths: ["src/**/*.ts", "src/**/*.tsx"],
  selector: {
    language: "typescript",
    node: "comment",
    inside: functionBody,
    ignore: [
      // Explicit tool directives stay, whatever comment style they use.
      { kind: "regex", value: "^(?://|/\\*)\\s*(?:@ts-(?:expect-error|ignore|nocheck)|eslint-(?:disable|enable)|prettier-ignore|biome-ignore|(?:istanbul|c8|v8) ignore)" },
      // So do tracked work markers.
      { kind: "regex", value: "\\b(?:TODO|FIXME)\\b" },
    ],
  },
  remediation: "Move the explanation out of the function body or into a name.",
  fixtures: [
    { path: "src/app.ts", content: "function f() {\n  // explain\n  return 1;\n}\n", expect: { line: 2, column: 3 } },
    { path: "src/app.ts", content: "function f(x: number) {\n  if (x) {\n    for (;;) {\n      /* deep */ break;\n    }\n  }\n}\n", expect: { line: 4, column: 7 } },
    { path: "src/app.ts", content: "export const f = () => {\n  return [1].map((x) => {\n    // nested callback\n    return x;\n  });\n};\n", expect: { line: 3, column: 5 } },
    { path: "src/app.ts", content: "class A {\n  // between members\n  run(): void {\n    // in a method\n  }\n}\n", expect: { line: 4, column: 5 } },
    { path: "src/app.ts", content: "function f(): /* return type */ number {\n  return 1;\n}\n", expect: null },
    { path: "src/app.ts", content: "function f(): Promise</* inner */ number> {\n  return Promise.resolve(1);\n}\n", expect: null },
    { path: "src/app.ts", content: "function f(/* parameter */ x: number) {\n  return x;\n}\n", expect: null },
    { path: "src/app.ts", content: "// top level\nfunction f() {\n  return 1;\n}\n/* after */\n", expect: null },
    { path: "src/app.ts", content: "function f() {\n  const a = \"// not a comment\";\n  const b = `/* nor this */`;\n  const c = /\\/\\/ regex/;\n  return [a, b, c];\n}\n", expect: null },
    { path: "src/app.ts", content: "function f() {\n  // eslint-disable-next-line no-console\n  console.log(1);\n  // @ts-expect-error: checked at runtime\n  g(1);\n  /* istanbul ignore next */\n  h();\n  // prettier-ignore\n  const m = [1,0];\n}\n", expect: null },
    { path: "src/app.ts", content: "function f() {\n  // TODO: split this up\n  // FIXME(pas): wrong for 0\n  return 1;\n}\n", expect: null },
    { path: "src/app.ts", content: "function f() {\n  // TODO: later\n  const x = 1; // the real comment\n  return x;\n}\n", expect: { line: 3, column: 16 } },
    { path: "src/app.tsx", content: "export function App() {\n  return (\n    <div>\n      {/* in JSX */}\n      <p>// text, not a comment</p>\n    </div>\n  );\n}\n", expect: { line: 4, column: 8 } },
    { path: "src/app.tsx", content: "export function App() {\n  return <p>// text, not a comment</p>;\n}\n", expect: null },
    { path: "src/app.tsx", content: "export const App = () => <div title=\"/* not a comment */\" />;\n", expect: null },
    { path: "scripts/build.ts", content: "function f() {\n  // out of scope\n}\n", expect: null },
  ],
});
