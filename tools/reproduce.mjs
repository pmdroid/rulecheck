// Reproducibility check: build policy twice from one exact commit, check that
// commit with each policy, and require identical policy bytes, results, and
// finding order. Wall times are recorded as observations only; no run fails for
// being slow, apart from the per-step timeout shared with the CLI.
//
//   node tools/reproduce.mjs REPO COMMIT_SHA EVALUATOR > reproduction.json
//
// Exit 0: both runs agree. Exit 1: they differ. Exit 2: a step could not run.
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { isDeepStrictEqual } from "node:util";
import { performance } from "node:perf_hooks";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const builder = fileURLToPath(new URL("./build.mjs", import.meta.url));
const [root, commit, evaluator, ...extra] = process.argv.slice(2);
const fullSha = /^(?:[0-9a-f]{40}|[0-9a-f]{64})$/;
const timeout = 120_000;

function run(command, args, input) {
  const started = performance.now();
  const result = spawnSync(command, args, {
    input,
    maxBuffer: 64 * 1024 * 1024,
    timeout,
    killSignal: "SIGKILL",
  });
  const wallTimeMs = Math.round(performance.now() - started);
  if (result.error) throw new Error(`${command} ${args[0]}: ${result.error.message}`);
  return { status: result.status, stdout: result.stdout, stderr: result.stderr.toString(), wallTimeMs };
}

function version(command, args) {
  const { status, stdout, stderr } = run(command, args);
  if (status !== 0) throw new Error(`could not read the version of ${command}: ${stderr.trim()}`);
  return stdout.toString().trim();
}

function digest(bytes) {
  return `sha256:${createHash("sha256").update(bytes).digest("hex")}`;
}

function attempt(repo, tool) {
  const build = run(process.execPath, [builder, "--root", repo, "--commit", commit]);
  if (build.status !== 0) throw new Error(`builder exited ${build.status}: ${build.stderr.trim()}`);
  const test = run(tool, ["test"], build.stdout);
  const check = run(tool, ["check", "--root", repo, "--commit", commit, "--format", "json"], build.stdout);
  const result = JSON.parse(check.stdout.toString());
  if (result.resultVersion !== 1 || result.exitCode !== check.status) {
    throw new Error(`evaluator exited ${check.status} without a matching version 1 result`);
  }
  return {
    policy: build.stdout,
    result,
    test: { exitCode: test.status, stdout: test.stdout.toString() },
    summary: {
      policySha256: digest(build.stdout),
      policyBytes: build.stdout.length,
      testExitCode: test.status,
      checkOutcome: result.outcome,
      checkExitCode: result.exitCode,
      findings: result.findings.length,
      wallTimeMs: { build: build.wallTimeMs, test: test.wallTimeMs, check: check.wallTimeMs },
    },
  };
}

const report = { version: 1, commit: commit ?? null, tools: null, runs: [], identical: null, outcome: "error" };
try {
  if (!root || !evaluator || extra.length || !fullSha.test(commit ?? "")) {
    throw new Error("Usage: node tools/reproduce.mjs REPO COMMIT_SHA EVALUATOR");
  }
  const repo = resolve(root);
  const tool = resolve(evaluator);
  report.tools = {
    node: process.version,
    git: version("git", ["--version"]),
    builder: version(process.execPath, [builder, "--version"]),
    evaluator: version(tool, ["--version"]),
  };
  const first = attempt(repo, tool);
  report.runs.push(first.summary);
  const second = attempt(repo, tool);
  report.runs.push(second.summary);
  report.identical = {
    policyBytes: first.policy.equals(second.policy),
    tests: isDeepStrictEqual(first.test, second.test),
    findingsInOrder: isDeepStrictEqual(first.result.findings, second.result.findings),
    result: isDeepStrictEqual(first.result, second.result),
  };
  report.outcome = Object.values(report.identical).every(Boolean) ? "reproducible" : "different";
} catch (error) {
  report.error = error.message;
}

process.exitCode = { reproducible: 0, different: 1, error: 2 }[report.outcome];
process.stdout.write(`${JSON.stringify(report, null, 2)}\n`);
