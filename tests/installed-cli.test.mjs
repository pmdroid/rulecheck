import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { copyFileSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { test } from "node:test";

const configured = process.env.RULECHECK_INSTALLED_BIN;
const cli = configured && resolve(configured);
const rule = fileURLToPath(new URL("../.rulecheck/rules/no-legacy-mode.rule.ts", import.meta.url));

test("Cargo-installed frontend evaluates rules with a pinned compiler", { skip: !cli }, (t) => {
  const tmp = mkdtempSync(join(tmpdir(), "rulecheck-installed-"));
  t.after(() => rmSync(tmp, { recursive: true, force: true }));
  const cwd = join(tmp, "repo");
  const cache = join(tmp, "cache");
  const shim = join(tmp, "shim");
  mkdirSync(shim);
  const env = { ...process.env, XDG_CACHE_HOME: cache };
  const run = (cmd, args, expected, extraEnv = {}) => {
    const result = spawnSync(cmd, args, {
      cwd, env: { ...env, ...extraEnv }, encoding: "utf8", timeout: 90_000,
    });
    assert.ifError(result.error);
    assert.equal(result.status, expected, `${cmd} ${args.join(" ")}\n${result.stderr}\n${result.stdout}`);
    return result;
  };
  const write = (path, data) => {
    const full = join(cwd, path);
    mkdirSync(dirname(full), { recursive: true });
    writeFileSync(full, data);
  };
  write(".npmrc", "registry=http://127.0.0.1:1/\n");
  write("config/app.toml", "legacy_mode = false\n");
  write("node_modules/typescript/package.json", '{"name":"typescript","version":"5.9.3","main":"index.js"}');
  write("node_modules/typescript/index.js", "throw new Error('project TypeScript must not execute');\n");
  const rulePath = join(cwd, ".rulecheck/rules/legacy.rule.ts");
  mkdirSync(dirname(rulePath), { recursive: true });
  copyFileSync(rule, rulePath);
  run("git", ["init", "-q"], 0);
  run("git", ["add", "--", ".rulecheck", "config/app.toml"], 0);
  run("git", ["-c", "user.name=Rulecheck", "-c", "user.email=rulecheck@example.invalid",
    "-c", "commit.gpgsign=false", "commit", "-qm", "Add passing rule fixture"], 0);
  const sha = run("git", ["rev-parse", "HEAD"], 0).stdout.trim();
  assert.match(run(cli, ["--version"], 0, { PATH: shim }).stdout, /^rulecheck [0-9]+\.[0-9]+\.[0-9]+/);
  assert.match(run(cli, ["test"], 0, { NODE_OPTIONS: "--require=/must-not-be-loaded.cjs" }).stdout,
    /7 fixtures passed/);
  writeFileSync(join(shim, "npm"), "#!/bin/sh\nexit 99\n", { mode: 0o755 });
  const warm = { PATH: `${shim}:${process.env.PATH}` };
  assert.equal(run(cli, ["check"], 0, warm).stdout, "");
  write("config/app.toml", "legacy_mode=true\n");
  const result = JSON.parse(run(cli, ["check", "--format", "json"], 1, warm).stdout);
  assert.equal(result.findings[0].ruleId, "config/no-legacy-mode");
  assert.match(result.tools.builder, /TypeScript 5\.9\.3/);
  assert.equal(run(cli, ["check", "--commit", sha], 0, warm).stdout, "");
  write(".rulecheck/rules/broken.rule.ts", "export const x = 1;\n");
  const invalid = JSON.parse(run(cli, ["check", "--format", "json"], 2, warm).stdout);
  assert.equal(invalid.outcome, "error");
  assert.match(invalid.diagnostics[0].message, /Expected the SDK marker import/);
});
