#!/usr/bin/env node
// Public `rulecheck` entry point. It builds policy with the builder shipped next to
// this file and streams it to the Rust evaluator in the same installation. It never
// loads code from the checked repository and never looks up `rulecheck` on PATH.
import { spawnSync } from "node:child_process";
import { constants, accessSync, readFileSync, realpathSync } from "node:fs";
import { createRequire } from "node:module";
import { join, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

const install = realpathSync(fileURLToPath(new URL("..", import.meta.url)));
const builder = join(install, "tools", "build.mjs");
const evaluator = join(install, "target", "release", "rulecheck-evaluator");
const guard = "RULECHECK_WRAPPER_ACTIVE";
const maxPolicy = 4 * 1024 * 1024; // MAX_POLICY in src/policy.rs
const maxResult = 64 * 1024 * 1024;
const timeout = 120_000;

const HELP = `\
rulecheck: deterministic repository checks

  rulecheck check [--root DIR] [--commit SHA | --untracked] [--id-prefix PREFIX] [--all-matches]
                  [--text-budget MIB] [--baseline FILE [--strict-baseline]] [--format human|json]
                     Build policy from .rulecheck/rules/**/*.rule.ts and scan the checkout
  rulecheck baseline [--root DIR] [--commit SHA | --untracked] [--id-prefix PREFIX]
                  [--text-budget MIB] [--baseline FILE [--write]]
                     Record the finding counts of a complete scan as a baseline
  rulecheck test [--root DIR] [--commit SHA] [--id-prefix PREFIX]
                     Run the rule fixtures
  rulecheck list [--root DIR] [--commit SHA] [--id-prefix PREFIX]
                     List rule IDs
  rulecheck validate [--root DIR] [--commit SHA]
                     Build and validate the policy
  rulecheck schema   Print the version 1 JSON schema
  rulecheck help     Print this help

--root selects the Git checkout root (default: current directory). --commit takes a
full 40- or 64-character SHA; resolve HEAD or a short SHA with git rev-parse first.
By default check scans the files in the Git index; --untracked also scans non-ignored
untracked files and cannot be combined with --commit. --all-matches reports every
text match instead of the first per rule and file; syntax rules cannot be counted
yet, so selecting one exits 2. --text-budget sets how much scoped text one check
reads, in MiB: 1 to 1024, default 64. A check that reaches it reports partial findings
and exits 2; narrowing rule scopes is preferred to raising it.
--baseline FILE counts every match and reports only rule and path counts above the
baseline; --strict-baseline also fails counts below it. baseline prints the counts
of a complete scan, or with --write replaces FILE; with --id-prefix only those
rules' entries change. A relative FILE is resolved from the current directory.
Rules are parsed as static data by the builder in ${install}; rule modules are never executed.
The low-level evaluator that reads policy from stdin is ${evaluator}.
--format json writes one versioned result document (schema/result-v1.json) to stdout.
The builder and the evaluator each stop after ${timeout / 1000} s.

Exit codes: 0 clean, 1 violation or fixture mismatch, 2 cannot judge.`;

class UsageError extends Error {}

const FULL_SHA = /^(?:[a-f0-9]{40}|[a-f0-9]{64})$/i;
const ROOT_HINT = "run from the checkout root or pass --root";
const UNTRACKED_COMMIT = "--untracked applies to working-tree checks only; "
  + "a commit has no untracked files, so drop --untracked or --commit";
// Flags that take no value; every other flag takes exactly one.
const SWITCHES = ["--untracked", "--all-matches", "--strict-baseline", "--write"];
const MAX_TEXT_BUDGET = 1024;
const TEXT_BUDGET_HINT = `--text-budget requires a whole number of MiB from 1 to ${MAX_TEXT_BUDGET} (default 64)`;

function fail(message) {
  console.error(`rulecheck: ${message}`);
  return 2;
}

function parse(command, args) {
  const options = new Map();
  const allowed = command === "validate" ? ["--root", "--commit"] : ["--root", "--commit", "--id-prefix"];
  if (command === "check") {
    allowed.push("--format", "--untracked", "--all-matches", "--text-budget", "--baseline", "--strict-baseline");
  }
  if (command === "baseline") allowed.push("--untracked", "--text-budget", "--baseline", "--write");
  for (let index = 0; index < args.length; index += 1) {
    const flag = args[index];
    const value = SWITCHES.includes(flag) ? true : args[++index];
    if (!allowed.includes(flag) || options.has(flag) || value === undefined || value === "") {
      throw new UsageError(`unexpected argument ${JSON.stringify(flag)}; run rulecheck help`);
    }
    options.set(flag, value);
  }
  if (options.has("--untracked") && options.has("--commit")) throw new UsageError(UNTRACKED_COMMIT);
  if (options.has("--strict-baseline") && !options.has("--baseline")) {
    throw new UsageError("--strict-baseline requires --baseline FILE");
  }
  if (options.has("--write") && !options.has("--baseline")) {
    throw new UsageError("--write requires --baseline FILE, the file to regenerate");
  }
  // The evaluator runs in the installation directory, so pass an absolute path.
  if (options.has("--baseline")) options.set("--baseline", resolve(options.get("--baseline")));
  if (!["human", "json", undefined].includes(options.get("--format"))) {
    throw new UsageError("--format requires human or json");
  }
  const budget = options.get("--text-budget");
  if (budget !== undefined && !(/^[0-9]+$/.test(budget) && Number(budget) >= 1 && Number(budget) <= MAX_TEXT_BUDGET)) {
    throw new UsageError(TEXT_BUDGET_HINT);
  }
  const commit = options.get("--commit");
  if (commit !== undefined && !FULL_SHA.test(commit)) {
    throw new UsageError(`--commit requires a full 40- or 64-character Git commit SHA, not ${JSON.stringify(commit)}; `
      + `resolve it first, e.g. --commit "$(git rev-parse ${/^[\w./@^~-]+$/.test(commit) ? commit : "HEAD"})"`);
  }
  return options;
}

// A working-tree scan reads .rulecheck/rules under --root, so --root must be the top
// of the checkout. Reported before the build so the message names the real cause.
function checkRoot(root, env) {
  const git = (args) => spawnSync("git", ["--no-replace-objects", "-C", root, ...args], { env, encoding: "utf8", timeout });
  const prefix = git(["rev-parse", "--show-prefix"]);
  if (prefix.error || prefix.status !== 0) throw new UsageError(`${root} is not a Git checkout; ${ROOT_HINT}`);
  if (prefix.stdout.trim()) {
    const top = git(["rev-parse", "--show-toplevel"]).stdout?.trim();
    throw new UsageError(`${root} is a subdirectory of a Git checkout, not its root; ${ROOT_HINT}`
      + (top ? ` (--root ${top})` : ""));
  }
}

// The builder imports TypeScript; accept only the copy installed with Rulecheck.
// Returns the builder version string that `node tools/build.mjs --version` prints.
function checkTypeScript() {
  const { version, dependencies } = JSON.parse(readFileSync(join(install, "package.json"), "utf8"));
  let installed = null;
  try {
    const modules = realpathSync(join(install, "node_modules"));
    const resolved = realpathSync(createRequire(builder).resolve("typescript"));
    if (resolved.startsWith(modules + sep)) {
      installed = JSON.parse(readFileSync(join(modules, "typescript", "package.json"), "utf8")).version;
    }
  } catch {
    // Reported below with the installation step that fixes it.
  }
  if (installed !== dependencies.typescript) {
    throw new Error(`the builder needs TypeScript ${dependencies.typescript} in ${install}/node_modules; `
      + `run npm ci --ignore-scripts there once`);
  }
  return `rulecheck-builder ${version} (TypeScript ${installed})`;
}

function checkEvaluator() {
  try {
    accessSync(evaluator, constants.X_OK);
  } catch {
    throw new Error(`evaluator not found at ${evaluator}; run cargo build --locked --release in ${install} once`);
  }
}

function status(result) {
  if (result.error) return null;
  return [0, 1, 2].includes(result.status) ? result.status : 2;
}

function spawnError(result) {
  return result.error.code === "ETIMEDOUT"
    ? `${evaluator} did not finish within ${timeout / 1000} s; narrow the rule scopes`
    : `cannot start ${evaluator}: ${result.error.message}`;
}

// A check result that the evaluator could not write, in the schema/result-v1.json
// shape. Unknown values stay null; the policy was never evaluated.
function errorResult(env, builder, options, messages) {
  const version = spawnSync(evaluator, ["--version"], { env, encoding: "utf8", timeout });
  const commit = options.get("--commit") ?? null;
  return {
    command: "check",
    diagnostics: messages.map((message) => ({ message, ruleId: null })),
    exitCode: 2,
    findings: [],
    outcome: "error",
    policy: { idPrefix: options.get("--id-prefix") ?? null, ruleIds: [], sha256: null, version: null },
    resultVersion: 1,
    source: {
      commit,
      mode: commit ? "commit" : "working-tree",
      ...(options.has("--untracked") ? { untracked: true } : {}),
      ...(options.has("--all-matches") || options.has("--baseline") ? { matches: "all" } : {}),
      ...(options.has("--text-budget") ? { textBudgetMiB: Number(options.get("--text-budget")) } : {}),
      ...(options.has("--baseline") ? { baselineSha256: null } : {}),
      ...(options.has("--strict-baseline") ? { strictBaseline: true } : {}),
    },
    tools: { builder, evaluator: version.status === 0 ? version.stdout.trim() : `rulecheck (${evaluator})` },
  };
}

function writeJson(value) {
  process.stdout.write(`${JSON.stringify(value, null, 2)}\n`);
}

function main(argv) {
  if (process.env[guard]) {
    return fail(`${evaluator} started the rulecheck wrapper again; it must be the Rust evaluator binary`);
  }
  const env = { ...process.env, [guard]: "1" };
  const [command, ...args] = argv;

  if (command === undefined) {
    console.error(HELP);
    return fail("missing command; run rulecheck help");
  }
  if (["help", "-h", "--help"].includes(command) && args.length === 0) {
    console.log(HELP);
    return 0;
  }
  checkEvaluator();
  if (command === "schema" || (["-V", "--version"].includes(command) && args.length === 0)) {
    const result = spawnSync(evaluator, [command, ...args], { stdio: ["ignore", "inherit", "inherit"], env });
    return status(result) ?? fail(`cannot start ${evaluator}: ${result.error.message}`);
  }
  if (!["check", "baseline", "test", "list", "validate"].includes(command)) {
    throw new UsageError(`unknown command ${JSON.stringify(command)}; run rulecheck help`);
  }

  const options = parse(command, args);
  const root = resolve(options.get("--root") ?? ".");
  const commit = options.get("--commit");
  const json = options.get("--format") === "json";
  if (!commit) checkRoot(root, env);
  const builderVersion = checkTypeScript();

  const source = ["--root", root, ...(commit ? ["--commit", commit] : [])];
  const build = spawnSync(process.execPath, [builder, ...source], {
    cwd: install,
    env,
    stdio: ["ignore", "pipe", "pipe"],
    maxBuffer: maxPolicy + 1024,
    timeout,
    killSignal: "SIGKILL",
  });
  if (build.stderr?.length) process.stderr.write(build.stderr);
  if (build.error || build.status !== 0) {
    const rules = commit ? `.rulecheck/rules at commit ${commit} in ${root}` : `${root}/.rulecheck/rules`;
    const message = `could not build policy from ${rules}${build.error ? `: ${build.error.message}` : ""}`;
    if (json) {
      const lines = build.stderr?.toString().split("\n").filter((line) => line.trim()) ?? [];
      writeJson(errorResult(env, builderVersion, options, [...lines, message]));
    }
    return fail(message);
  }

  const evaluate = [command, ...(["check", "baseline"].includes(command) ? source : [])];
  if (options.has("--id-prefix")) evaluate.push("--id-prefix", options.get("--id-prefix"));
  if (options.has("--untracked")) evaluate.push("--untracked");
  if (options.has("--all-matches")) evaluate.push("--all-matches");
  if (options.has("--text-budget")) evaluate.push("--text-budget", options.get("--text-budget"));
  if (options.has("--baseline")) evaluate.push("--baseline", options.get("--baseline"));
  for (const flag of ["--strict-baseline", "--write"]) {
    if (options.has(flag)) evaluate.push(flag);
  }
  if (json) evaluate.push("--format", "json");
  const result = spawnSync(evaluator, evaluate, {
    cwd: install,
    env,
    input: build.stdout,
    stdio: ["pipe", json ? "pipe" : "inherit", "inherit"],
    maxBuffer: maxResult,
    timeout,
    killSignal: "SIGKILL",
  });
  const code = status(result);
  if (!json) return code ?? fail(spawnError(result));

  // Record the builder that produced the policy; keep everything else as written.
  let document = null;
  try {
    document = code === null ? null : JSON.parse(result.stdout);
  } catch {
    // Reported below as an error result.
  }
  if (document?.resultVersion !== 1 || document.exitCode !== code) {
    const message = code === null ? spawnError(result) : `${evaluator} did not write a version 1 result`;
    writeJson(errorResult(env, builderVersion, options, [message]));
    return fail(message);
  }
  document.tools.builder = builderVersion;
  writeJson(document);
  return code;
}

try {
  process.exitCode = main(process.argv.slice(2));
} catch (error) {
  process.exitCode = fail(error.message);
}
