import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { copyFileSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { test } from "node:test";

const root = fileURLToPath(new URL("../", import.meta.url));
const evaluator = resolve(process.env.RULECHECK_BIN ?? join(root, "target/debug/rulecheck"));
const recipe = (clean, bad, finding) => ({ clean, bad, finding });
const recipes = {
  literal: recipe({ "config/app.toml": "legacy_mode = false\n" },
    { "config/app.toml": "# settings\nlegacy_mode = true\n" },
    "config/app.toml:2:1: [config/no-legacy-spelling] Remove the legacy setting or set it to false."),
  regex: recipe({ "config/app.toml": "debug = false\n" },
    { "config/app.toml": "debug=true # temporary\n" },
    "config/app.toml:1:1: [config/no-debug-true] Set debug to false before committing."),
  path: recipe({ "config/app.toml": "# normal configuration\n" },
    { "config/local.override.toml": "" },
    "config/local.override.toml:1:1: [config/no-local-overrides] Move the local override outside the tracked checkout."),
  scope: recipe({ "src/app.ts": "currentMarker();\n", "generated/app.ts": "legacyMarker();\n" },
    { "src/app.ts": "legacyMarker();\n" },
    "src/app.ts:1:1: [source/no-legacy-marker] Replace the legacy marker in source."),
  exclude: recipe({
    "src/app.ts": "logger.info(user);\n",
    "src/generated/client.ts": "console.log(user);\n",
    "src/app.test.ts": "console.log(user);\n",
  }, { "src/app.ts": "export function run(user) {\n  console.log(user);\n}\n" },
  "src/app.ts:2:3: [source/no-console-log] Use the project logger instead of console.log."),
  "rule-sources": recipe({ "scripts/release.ts": 'export const version = "1.0.0";\n' },
    { "scripts/release.ts": "export function release() {\n  debugger;\n}\n" },
    "scripts/release.ts:2:3: [source/no-debugger] Remove the debugger statement."),
  "multi-scope": recipe({
    "packages/core/src/index.ts": "// NEW_NAME\n",
    "services/api/index.ts": "// old_name\n",
    "services/web/index.ts": "// OLD_NAME\n",
  }, {
    "packages/core/src/index.ts": "// π OLD_NAME OLD_NAME\n",
    "services/api/index.ts": "// OLD_NAME\n",
  }, "packages/core/src/index.ts:1:6: [source/no-old-name] Use NEW_NAME instead.\n"
    + "services/api/index.ts:1:4: [source/no-old-name] Use NEW_NAME instead."),
  "path-globs": recipe({
    "config/team/safe-1a.toml": "", "config/team/local-aa.toml": "",
    "config/team/local-1ab.toml": "", "examples/docker-compose.override.yml": "",
  }, {
    "config/private-2b.toml": "", "config/team/local-1a.toml": "", "docker-compose.override.yml": "",
  }, "config/private-2b.toml:1:1: [config/no-machine-specific-files] Keep machine-specific files out of Git.\n"
    + "config/team/local-1a.toml:1:1: [config/no-machine-specific-files] Keep machine-specific files out of Git.\n"
    + "docker-compose.override.yml:1:1: [config/no-machine-specific-files] Keep machine-specific files out of Git."),
  syntax: recipe({
    "src/total.ts": "// Totals for the report.\nexport function total(xs: number[]): number {\n  // TODO: use reduce\n  let sum = 0;\n  for (const x of xs) sum += x;\n  return sum;\n}\n",
    "src/hint.tsx": "export const Hint = () => <p>// type a path</p>;\n",
  }, { "src/total.ts": "export function total(xs: number[]): number {\n  // add them up\n  return xs.reduce((a, b) => a + b, 0);\n}\n" },
  "src/total.ts:2:3: [source/no-function-body-comments] Move the explanation out of the function body or into a name."),
  "syntax-reuse": recipe({ "src/first.ts": "export function first(xs: string[]): string {\n  return xs[0] ?? \"\";\n}\n" },
    { "src/first.ts": "export function first(xs: string[]): string {\n  return xs[0]!;\n}\n" },
    "src/first.ts:2:10: [source/no-non-null-assertion] Check for null or undefined instead of asserting with !."),
  "syntax-rust": recipe({
    "src/lib.rs": '//! Totals for the report.\n/// Sums the values.\npub fn total(xs: &[i32]) -> i32 {\n    // TODO: overflow check\n    let sep = "// none";\n    let _ = sep;\n    xs.iter().sum()\n}\n\npub fn read(p: *const i32) -> i32 {\n    // SAFETY: callers pass a valid pointer.\n    unsafe { *p }\n}\n',
  }, { "src/lib.rs": "pub fn total(xs: &[i32]) -> i32 {\n    // add them up\n    xs.iter().sum()\n}\n" },
  "src/lib.rs:2:5: [source/no-rust-function-body-comments] Move the explanation out of the function body or into a name."),
};

function run(bin, args, { cwd, input, status = 0 } = {}) {
  const result = spawnSync(bin, args, { cwd, input, encoding: "utf8", timeout: 20_000 });
  assert.ifError(result.error);
  assert.equal(result.status, status, `${bin} ${args.join(" ")}\n${result.stderr}\n${result.stdout}`);
  return result;
}

for (const [name, { clean, bad, finding }] of Object.entries(recipes)) {
  test(name, (t) => {
    const cwd = mkdtempSync(join(tmpdir(), "rulecheck-recipe-"));
    t.after(() => rmSync(cwd, { recursive: true, force: true }));
    const git = (...args) => run("git", args, { cwd }).stdout.trimEnd();
    const write = (files) => {
      for (const [path, data] of Object.entries(files)) {
        const file = join(cwd, path);
        mkdirSync(dirname(file), { recursive: true });
        writeFileSync(file, data);
      }
    };
    git("init", "-q");
    const rulePath = join(cwd, ".rulecheck/rules", `${name}.rule.ts`);
    mkdirSync(dirname(rulePath), { recursive: true });
    copyFileSync(join(root, "docs/examples/rules", `${name}.rule.ts`), rulePath);
    const input = run(process.execPath, [join(root, "tools/build.mjs"), "--root", cwd], { cwd }).stdout;
    run(evaluator, ["validate"], { input, cwd });
    run(evaluator, ["test"], { input, cwd });
    const scan = ({ policy = input, sha, expected = "" } = {}) => {
      const args = ["check", "--root", cwd, ...(sha ? ["--commit", sha] : [])];
      const actual = run(evaluator, args, { input: policy, cwd, status: expected ? 1 : 0 });
      assert.equal(actual.stdout.trimEnd(), expected);
      assert.equal(actual.stderr, "");
    };
    write(clean);
    git("add", "-f", "--", ".");
    if (name === "rule-sources") {
      const unexcluded = JSON.parse(input);
      for (const rule of unexcluded.rules) delete rule.exclude;
      const policy = JSON.stringify(unexcluded);
      git("-c", "user.name=Rulecheck", "-c", "user.email=rulecheck@example.invalid",
        "-c", "commit.gpgsign=false", "commit", "-qm", "Add rule");
      const sha = git("rev-parse", "HEAD");
      const expected = ".rulecheck/rules/rule-sources.rule.ts:8:39: [source/no-debugger] Remove the debugger statement.";
      scan({ policy, expected });
      scan({ policy, expected, sha });
      scan({ sha });
    }
    scan();
    write(bad);
    git("add", "-f", "--", ".");
    scan({ expected: finding });
    if (name === "scope") {
      write(clean);
      scan();
    }
  });
}

test("invalid policy exits 2", () => {
  const result = run(evaluator, ["validate"], { status: 2, input: '{"version":42,"rules":[]}' });
  assert.match(result.stderr, /unsupported policy version 42/);
});
