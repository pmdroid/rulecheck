import { defineRule } from "@pmdroid/rulecheck";

// Every job in a workflow must declare timeout-minutes, so a stuck job cannot
// hold a runner for the default six hours.
export default defineRule({
  id: "ci/job-timeout",
  kind: "structured",
  paths: [".github/workflows/*.yml", ".github/workflows/*.yaml"],
  structure: {
    format: "yaml",
    // jobs, then each job under it. The assertion runs once per job.
    path: [{ key: "jobs" }, { each: "mapping" }],
    assert: { kind: "has-key", key: "timeout-minutes" },
  },
  remediation: "Add timeout-minutes to the job.",
  fixtures: [
    // The finding points at the job's key.
    {
      path: ".github/workflows/ci.yml",
      content: "on: push\njobs:\n  build:\n    runs-on: ubuntu-latest\n    steps:\n      - run: make\n",
      expect: { line: 3, column: 3 },
    },
    {
      path: ".github/workflows/ci.yml",
      content: "on: push\njobs:\n  build:\n    runs-on: ubuntu-latest\n    timeout-minutes: 10\n    steps:\n      - run: make\n",
      expect: null,
    },
    // Each job is checked: only the second one lacks the key.
    {
      path: ".github/workflows/ci.yml",
      content: "jobs:\n  lint:\n    timeout-minutes: 5\n  test:\n    runs-on: ubuntu-latest\n",
      expectAll: [{ line: 4, column: 3 }],
    },
    // Quoting the key doesn't change it. A key in a comment or a string isn't a key.
    {
      path: ".github/workflows/ci.yml",
      content: "jobs:\n  build:\n    \"timeout-minutes\": 10\n",
      expect: null,
    },
    {
      path: ".github/workflows/ci.yml",
      content: "jobs:\n  build: # timeout-minutes: 10\n    run: echo timeout-minutes\n",
      expect: { line: 2, column: 3 },
    },
    // Flow style is the same structure.
    {
      path: ".github/workflows/ci.yml",
      content: "jobs: {build: {runs-on: ubuntu-latest}, lint: {timeout-minutes: 5}}\n",
      expect: { line: 1, column: 8 },
    },
    // A workflow without jobs has nothing to check here.
    { path: ".github/workflows/ci.yml", content: "on: push\n", expect: null },
    // Each document of a multi-document file is checked on its own.
    {
      path: ".github/workflows/ci.yml",
      content: "jobs:\n  a:\n    timeout-minutes: 5\n---\njobs:\n  b: {}\n",
      expect: { line: 6, column: 3 },
    },
    // Other YAML files are out of scope.
    { path: "deploy/ci.yml", content: "jobs:\n  build: {}\n", expect: null },
  ],
});
