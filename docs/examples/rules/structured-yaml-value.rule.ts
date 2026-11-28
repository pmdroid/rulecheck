import { defineRule } from "@pmdroid/rulecheck";

// A workflow must not grant write access to repository contents at the top
// level, where every job inherits it.
export default defineRule({
  id: "ci/no-contents-write",
  kind: "structured",
  paths: [".github/workflows/*.yml", ".github/workflows/*.yaml"],
  structure: {
    format: "yaml",
    path: [{ key: "permissions" }, { key: "contents" }],
    assert: { kind: "not-equals", value: "write" },
  },
  remediation: "Grant contents: write only to the job that needs it, or use read.",
  fixtures: [
    // The finding points at the value.
    {
      path: ".github/workflows/release.yml",
      content: "on: push\npermissions:\n  contents: write\n",
      expect: { line: 3, column: 13 },
    },
    // Scalars compare by their text: quoting doesn't hide the value.
    {
      path: ".github/workflows/release.yml",
      content: "permissions:\n  contents: \"write\"\n",
      expect: { line: 2, column: 13 },
    },
    { path: ".github/workflows/release.yml", content: "permissions:\n  contents: read\n", expect: null },
    // A missing key or a missing permissions block selects nothing.
    { path: ".github/workflows/release.yml", content: "permissions:\n  issues: write\n", expect: null },
    { path: ".github/workflows/release.yml", content: "on: push\n", expect: null },
    // Job-level permissions are a different path.
    {
      path: ".github/workflows/release.yml",
      content: "jobs:\n  publish:\n    permissions:\n      contents: write\n",
      expect: null,
    },
    // "write" in a comment or another key's value is not this value.
    {
      path: ".github/workflows/release.yml",
      content: "permissions:\n  contents: read # not write\n  pull-requests: write\n",
      expect: null,
    },
  ],
});
