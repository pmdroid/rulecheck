// Runs the documented commands in the layout the docs say they need: the
// README and quickstart demos and every command in the CLI reference against a
// Cargo- or release-archive-installed `rulecheck` (RULECHECK_INSTALLED_BIN), and the low-level
// pipeline against this checkout's builder and an evaluator (RULECHECK_EVALUATOR).
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { delimiter, dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { test } from "node:test";

const root = fileURLToPath(new URL("../", import.meta.url));
const installed = process.env.RULECHECK_INSTALLED_BIN && resolve(process.env.RULECHECK_INSTALLED_BIN);
const evaluator = process.env.RULECHECK_EVALUATOR && resolve(process.env.RULECHECK_EVALUATOR);
const read = (path) => readFileSync(join(root, path), "utf8");

// The first fenced block of `lang` after `heading` in a Markdown file.
function block(path, heading, lang) {
  const text = read(path);
  const at = text.indexOf(`\n${heading}\n`);
  assert.notEqual(at, -1, `${path} has no ${heading}`);
  const match = text.slice(at).match(new RegExp("```" + lang + "\\n([\\s\\S]*?)\\n```"));
  assert.ok(match, `${path} ${heading} has no ${lang} block`);
  return match[1];
}

function sandbox(t) {
  const dir = realpathSync(mkdtempSync(join(tmpdir(), "rulecheck-docs-")));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  for (const sub of ["tmp", "cache"]) mkdirSync(join(dir, sub));
  return {
    dir,
    env: {
      ...process.env,
      TMPDIR: join(dir, "tmp"),
      XDG_CACHE_HOME: join(dir, "cache"),
      PATH: `${dirname(installed)}${delimiter}${process.env.PATH}`,
    },
  };
}

function bash(script, cwd, env) {
  const result = spawnSync("bash", ["-c", script], { cwd, env, encoding: "utf8", timeout: 240_000 });
  assert.ifError(result.error);
  return result;
}

// A demo checkout with the README rule, staged and clean.
function demo(dir, env) {
  const repo = join(dir, "repo");
  mkdirSync(join(repo, ".rulecheck/rules"), { recursive: true });
  mkdirSync(join(repo, "config"));
  const script = block("README.md", "## Check a repository", "bash");
  const rule = script.match(/<<'RULE'\n([\s\S]*?)\nRULE\n/)[1];
  writeFileSync(join(repo, ".rulecheck/rules/no-legacy-mode.rule.ts"), `${rule}\n`);
  writeFileSync(join(repo, "config/app.toml"), "legacy_mode = false\n");
  const setup = bash("git init -q && git add -A", repo, env);
  assert.equal(setup.status, 0, setup.stderr);
  return repo;
}

test("README demo prints what its comments say with the installed CLI", { skip: !installed }, (t) => {
  const { dir, env } = sandbox(t);
  const script = block("README.md", "## Check a repository", "bash");
  // Each `# text` comment line and `echo "$?"  # N` is the next expected line.
  const expected = [];
  for (const line of script.split("\n")) {
    const comment = line.match(/^# (.*)$/) ?? line.match(/^echo "\$\?"\s+# (\d)$/);
    if (comment) expected.push(comment[1]);
  }
  assert.ok(expected.length >= 6, "README demo lost its expected-output comments");
  // Send the whole script's stderr to stdout, so builder errors appear in order.
  const result = bash(`exec 2>&1\n${script}`, dir, env);
  assert.equal(result.status, 0, result.stdout);
  const actual = result.stdout.split("\n").filter((line, i, all) => i < all.length - 1);
  // The last line is the directory `cd -` prints.
  assert.equal(actual.pop(), dir);
  const pattern = (line) => new RegExp(`^${line.split("...").map((part) =>
    part.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")).join(".*").replace("/tmp/.*", ".*")}$`);
  assert.equal(actual.length, expected.length, result.stdout);
  expected.forEach((line, i) => assert.match(actual[i], pattern(line), result.stdout));
});

test("quickstart demo finds the violation with the installed CLI", { skip: !installed }, (t) => {
  const { dir, env } = sandbox(t);
  const result = bash(block("docs/src/content/docs/quickstart.md", "## Try a rule", "sh"), dir, env);
  assert.equal(result.status, 1, result.stderr);
  assert.equal(result.stdout, "2 fixtures passed\n"
    + "config/app.toml:1:1: [config/no-legacy-mode] Remove legacy_mode or set it to false.\n");
});

test("every command in the CLI reference runs from the installed CLI", { skip: !installed }, (t) => {
  const { dir, env } = sandbox(t);
  const repo = demo(dir, env);
  // The command table is the first table in the reference, before the exit codes.
  const reference = read("docs/src/content/docs/cli.md").split("\n| Exit |")[0];
  const commands = [...reference.matchAll(/^\| `rulecheck ([a-z]+)\b/gm)].map((match) => match[1]);
  assert.deepEqual(commands, ["check", "baseline", "test", "list", "validate", "schema", "help"]);
  const run = (args, expected) => {
    const result = spawnSync(installed, args, { cwd: repo, env, encoding: "utf8", timeout: 120_000 });
    assert.ifError(result.error);
    assert.equal(result.status, expected, `rulecheck ${args.join(" ")}\n${result.stderr}`);
    return result.stdout;
  };
  const help = run(["help"], 0);
  for (const command of commands) {
    assert.match(help, new RegExp(`^  rulecheck ${command}\\b`, "m"), `help lists ${command}`);
  }
  // Help names the installed evaluator, not the temporary copy the wrapper runs from.
  const companion = realpathSync(join(dirname(installed), "rulecheck-evaluator"));
  assert.ok(help.includes(`reads policy from stdin is ${companion}.`), help);
  assert.ok(!help.includes(env.TMPDIR), help);
  assert.match(help, /builder bundled with this installation/);

  assert.equal(run(["check"], 0), "");
  assert.equal(run(["check", "--untracked"], 0), "");
  assert.match(run(["baseline"], 0), /"baselineVersion"/);
  assert.equal(run(["test"], 0), "2 fixtures passed\n");
  assert.equal(run(["list"], 0), "config/no-legacy-mode\n");
  assert.equal(run(["validate"], 0), "");
  assert.equal(JSON.parse(run(["schema"], 0)).title, "Rulecheck policy v1");
  assert.match(run(["--version"], 0), /^rulecheck \d+\.\d+\.\d+/);

  // The documented reason for --untracked: a new file is not scanned until staged.
  writeFileSync(join(repo, "config/new.toml"), "legacy_mode = true\n");
  assert.equal(run(["check"], 0), "");
  assert.equal(run(["check", "--untracked"], 1),
    "config/new.toml:1:1: [config/no-legacy-mode] Remove legacy_mode or set it to false.\n");

  // The source-only tools are not part of the installation.
  for (const tool of ["build.mjs", "check-ci.mjs", "reproduce.mjs"]) {
    assert.ok(!existsSync(join(dirname(installed), tool)), `${tool} is not installed`);
  }
});

test("README low-level pipeline runs from a checkout", { skip: !evaluator }, (t) => {
  const dir = realpathSync(mkdtempSync(join(tmpdir(), "rulecheck-docs-")));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  const repo = demo(dir, process.env);
  const pipeline = block("README.md", "### Low-level pipeline", "bash")
    .replaceAll("/path/to/checkout", repo)
    .replaceAll("target/release/rulecheck-evaluator", evaluator);
  const clean = bash(pipeline, root, process.env);
  assert.equal(clean.status, 0, clean.stderr);
  writeFileSync(join(repo, "config/app.toml"), "legacy_mode = true\n");
  const found = bash(pipeline, root, process.env);
  assert.equal(found.status, 1, found.stderr);
  assert.equal(found.stdout,
    "config/app.toml:1:1: [config/no-legacy-mode] Remove legacy_mode or set it to false.\n");
});
