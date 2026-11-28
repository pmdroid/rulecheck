import { defineRule } from "@pmdroid/rulecheck";

// Python attaches a comment before a function's first statement to the
// `function_definition` node itself, not to its `body` block, so this selector
// uses no `field`. A comment counts anywhere inside the definition: its body,
// nested blocks and methods, and also the parameter list and the `def` line.
const functionDefinition = {
  kinds: ["function_definition"],
};

export default defineRule({
  id: "source/no-python-function-comments",
  kind: "syntax",
  paths: ["src/**/*.py", "src/**/*.pyi"],
  selector: {
    language: "python",
    node: "comment",
    inside: functionDefinition,
    ignore: [
      // Tool directives stay.
      { kind: "regex", value: "^#\\s*(?:noqa\\b|type:|pragma:|pyright:|mypy:)" },
      // So do tracked work markers.
      { kind: "regex", value: "\\b(?:TODO|FIXME)\\b" },
    ],
  },
  remediation: "Move the explanation into a docstring or a name.",
  fixtures: [
    { path: "src/app/total.py", content: "def f():\n    # explain\n    return 1\n", expect: { line: 2, column: 5 } },
    { path: "src/app/total.py", content: "def f(x):\n    if x:\n        # deep\n        return x\n    return 0\n", expect: { line: 3, column: 9 } },
    { path: "src/app/total.py", content: "class C:\n    # class body\n    def run(self):\n        pass\n        # in a method\n", expect: { line: 5, column: 9 } },
    { path: "src/app/total.py", content: "def f():\n    x = 1  # trailing\n    return x\n", expect: { line: 2, column: 12 } },
    { path: "src/app/total.py", content: "def f(\n    x,  # parameter\n):\n    return x\n", expect: { line: 2, column: 9 } },
    { path: "src/app/total.py", content: "#!/usr/bin/env python3\n# Module header.\n@decorate  # decorator\ndef f():\n    \"\"\"Docs.\"\"\"\n\n\nx = lambda: 1  # lambda\n", expect: null },
    { path: "src/app/total.py", content: "def f(x):\n    \"\"\"Docstring with # inside.\"\"\"\n    a = \"# not a comment\"\n    b = f\"{x} # nor this\"\n    c = '''\n# nor this line\n'''\n    return a + b + c\n", expect: null },
    { path: "src/app/total.py", content: "def f():\n    import os  # noqa: F401\n    x = g()  # type: ignore[no-untyped-call]\n    # TODO: split this up\n    return x\n", expect: null },
    { path: "src/app/total.py", content: "def f():\n    # FIXME: wrong for 0\n    x = 1  # the real comment\n    return x\n", expect: { line: 3, column: 12 } },
    { path: "src/app/total.pyi", content: "def f() -> int: ...  # stub note\n", expect: { line: 1, column: 22 } },
    { path: "tests/test_total.py", content: "def test_f():\n    # out of scope\n    pass\n", expect: null },
  ],
});
