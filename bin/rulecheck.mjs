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
const evaluator = join(install, "target", "release", "rulecheck");
const guard = "RULECHECK_WRAPPER_ACTIVE";
const maxPolicy = 1024 * 1024;

const HELP = `\
rulecheck: deterministic repository checks

  rulecheck check [--root DIR] [--commit SHA] [--id-prefix PREFIX]
                     Build policy from .rulecheck/rules/**/*.rule.ts and scan the checkout
  rulecheck test [--root DIR] [--commit SHA] [--id-prefix PREFIX]
                     Run the rule fixtures
  rulecheck list [--root DIR] [--commit SHA] [--id-prefix PREFIX]
                     List rule IDs
  rulecheck validate [--root DIR] [--commit SHA]
                     Build and validate the policy
  rulecheck schema   Print the version 1 JSON schema

--root selects the Git checkout root (default: current directory). Rules are parsed
as static data by the builder in ${install}; rule modules are never executed.
The low-level evaluator that reads policy from stdin is ${evaluator}.

Exit codes: 0 clean, 1 violation or fixture mismatch, 2 cannot judge.`;

class UsageError extends Error {}

function fail(message) {
  console.error(`rulecheck: ${message}`);
  return 2;
}

function parse(command, args) {
  const options = new Map();
  const allowed = command === "validate" ? ["--root", "--commit"] : ["--root", "--commit", "--id-prefix"];
  for (let index = 0; index < args.length; index += 2) {
    const [flag, value] = args.slice(index, index + 2);
    if (!allowed.includes(flag) || options.has(flag) || value === undefined || value === "") {
      throw new UsageError(`unexpected argument ${JSON.stringify(flag)}; run rulecheck --help`);
    }
    options.set(flag, value);
  }
  return options;
}

// The builder imports TypeScript; accept only the copy installed with Rulecheck.
function checkTypeScript() {
  const { dependencies } = JSON.parse(readFileSync(join(install, "package.json"), "utf8"));
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

function main(argv) {
  if (process.env[guard]) {
    return fail(`${evaluator} started the rulecheck wrapper again; it must be the Rust evaluator binary`);
  }
  const env = { ...process.env, [guard]: "1" };
  const [command = "--help", ...args] = argv;

  if (["-h", "--help"].includes(command) && args.length === 0) {
    console.log(HELP);
    return 0;
  }
  checkEvaluator();
  if (command === "schema" || (["-V", "--version"].includes(command) && args.length === 0)) {
    const result = spawnSync(evaluator, [command, ...args], { stdio: ["ignore", "inherit", "inherit"], env });
    return status(result) ?? fail(`cannot start ${evaluator}: ${result.error.message}`);
  }
  if (!["check", "test", "list", "validate"].includes(command)) {
    throw new UsageError(`unknown command ${JSON.stringify(command)}; run rulecheck --help`);
  }

  const options = parse(command, args);
  const root = resolve(options.get("--root") ?? ".");
  const commit = options.get("--commit");
  checkTypeScript();

  const source = ["--root", root, ...(commit ? ["--commit", commit] : [])];
  const build = spawnSync(process.execPath, [builder, ...source], {
    cwd: install,
    env,
    stdio: ["ignore", "pipe", "pipe"],
    maxBuffer: maxPolicy + 1024,
    timeout: 120_000,
    killSignal: "SIGKILL",
  });
  if (build.stderr?.length) process.stderr.write(build.stderr);
  if (build.error || build.status !== 0) {
    return fail(`could not build policy from ${root}/.rulecheck/rules${build.error ? `: ${build.error.message}` : ""}`);
  }

  const evaluate = [command, ...(command === "check" ? source : [])];
  if (options.has("--id-prefix")) evaluate.push("--id-prefix", options.get("--id-prefix"));
  const result = spawnSync(evaluator, evaluate, {
    cwd: install,
    env,
    input: build.stdout,
    stdio: ["pipe", "inherit", "inherit"],
  });
  return status(result) ?? fail(`cannot start ${evaluator}: ${result.error.message}`);
}

try {
  process.exitCode = main(process.argv.slice(2));
} catch (error) {
  process.exitCode = fail(error.message);
}
