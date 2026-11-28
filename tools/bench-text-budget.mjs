#!/usr/bin/env node
// Compares the per-prefix workaround with one budgeted run on the same synthetic snapshot.
//
//   node tools/bench-text-budget.mjs [--packages N] [--mib N] [--runs N]
//
// It creates a throwaway Git repository with N packages (default 4) of N MiB of
// scoped text each (default 20, so 80 MiB in total, over the 64 MiB default) and one
// text rule per package. It then checks the same commit two ways with the installed
// wrapper in this checkout (bin/rulecheck.mjs, which needs target/release/rulecheck-evaluator):
//
//   workaround  one `check --id-prefix pkgN/` run per package, each under the default budget
//   one run     a single `check --text-budget M`, with M the total rounded up to a MiB
//
// For each it records the scoped text the evaluator reads, how many times the builder
// starts (once per wrapper run), and wall time (median of --runs, default 3). It exits
// 0 when both report the same findings and the default-budget single run stops with a
// partial result, and 1 otherwise. Every file and rule is synthetic.
import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const install = fileURLToPath(new URL("..", import.meta.url));
const wrapper = join(install, "bin", "rulecheck.mjs");
const MiB = 1024 * 1024;
const FILE = 512 * 1024;

function option(name, fallback) {
  const index = process.argv.indexOf(name);
  if (index < 0) return fallback;
  const value = Number(process.argv[index + 1]);
  if (!Number.isInteger(value) || value < 1) throw new Error(`${name} requires a positive integer`);
  return value;
}

const packages = option("--packages", 4);
const mib = option("--mib", 20);
const runs = option("--runs", 3);

function run(cmd, args, cwd, codes = [0]) {
  const result = spawnSync(cmd, args, { cwd, encoding: "utf8", maxBuffer: 64 * MiB });
  if (result.error || !codes.includes(result.status)) {
    throw new Error(`${cmd} ${args.join(" ")} exited ${result.status}\n${result.stderr}`);
  }
  return result;
}

function write(root, path, data) {
  mkdirSync(dirname(join(root, path)), { recursive: true });
  writeFileSync(join(root, path), data);
}

function rule(name) {
  return `import { defineRule } from "@rulecheck/sdk";

export default defineRule({
  id: "${name}/no-debug-marker",
  kind: "text",
  paths: ["packages/${name}/**/*.txt"],
  pattern: { kind: "literal", value: "DEBUG_MARKER" },
  remediation: "Remove the debug marker.",
  fixtures: [
    { path: "packages/${name}/x.txt", content: "DEBUG_MARKER\\n", expect: { line: 1, column: 1 } },
    { path: "packages/${name}/y.txt", content: "clean\\n", expect: null },
  ],
});
`;
}

function median(values) {
  const sorted = [...values].sort((a, b) => a - b);
  return sorted[Math.floor(sorted.length / 2)];
}

// Runs the wrapper with --format json and returns the parsed result and wall time.
function check(root, args) {
  const started = process.hrtime.bigint();
  const result = run(process.execPath, [wrapper, "check", ...args, "--format", "json"], root, [0, 1, 2]);
  const ms = Number(process.hrtime.bigint() - started) / 1e6;
  return { result: JSON.parse(result.stdout), ms };
}

const tmp = mkdtempSync(join(tmpdir(), "rulecheck-bench-"));
try {
  const root = join(tmp, "repo");
  mkdirSync(root);
  run("git", ["init", "-q"], root);
  const names = Array.from({ length: packages }, (_, n) => `pkg${n + 1}`);
  const filler = Buffer.alloc(FILE, "lorem ipsum dolor sit amet\n");
  let total = 0;
  for (const name of names) {
    write(root, `.rulecheck/rules/${name}.rule.ts`, rule(name));
    for (let n = 0; n < (mib * MiB) / FILE; n += 1) {
      const data = Buffer.from(filler);
      // One marker per package, in its first file.
      if (n === 0) data.write("DEBUG_MARKER\n");
      write(root, `packages/${name}/src/${String(n).padStart(4, "0")}.txt`, data);
      total += data.length;
    }
  }
  run("git", ["add", "-A"], root);
  run("git", ["-c", "user.name=Rulecheck Bench", "-c", "user.email=bench@example.invalid",
    "-c", "commit.gpgsign=false", "commit", "-qm", "Synthetic benchmark snapshot"], root);
  const sha = run("git", ["rev-parse", "HEAD"], root).stdout.trim();
  const budget = Math.ceil(total / MiB);

  const defaultRun = check(root, ["--commit", sha]).result;
  const stopsAtDefault = defaultRun.exitCode === 2 && defaultRun.partial?.reason === "text-budget";

  const split = [];
  const single = [];
  let splitFindings = [];
  let singleFindings = [];
  for (let attempt = 0; attempt < runs; attempt += 1) {
    let ms = 0;
    splitFindings = [];
    for (const name of names) {
      const one = check(root, ["--commit", sha, "--id-prefix", `${name}/`]);
      if (one.result.exitCode === 2) throw new Error(`${name} alone cannot be judged: ${JSON.stringify(one.result.diagnostics)}`);
      ms += one.ms;
      splitFindings.push(...one.result.findings);
    }
    split.push(ms);
    const one = check(root, ["--commit", sha, "--text-budget", String(budget)]);
    if (one.result.partial) throw new Error("the single run stopped at the budget");
    single.push(one.ms);
    singleFindings = one.result.findings;
  }
  const key = (finding) => `${finding.path}:${finding.line}:${finding.column}:${finding.ruleId}`;
  const same = JSON.stringify(splitFindings.map(key).sort()) === JSON.stringify(singleFindings.map(key).sort());

  const report = {
    snapshot: { packages, scopedTextBytes: total, files: (total / FILE), commit: sha },
    defaultBudget: { exitCode: defaultRun.exitCode, partial: defaultRun.partial ?? null },
    workaround: {
      runs: packages,
      builderStarts: packages,
      scopedTextBytesRead: total,
      wallMsMedian: Math.round(median(split)),
    },
    oneRun: {
      runs: 1,
      textBudgetMiB: budget,
      builderStarts: 1,
      scopedTextBytesRead: total,
      wallMsMedian: Math.round(median(single)),
    },
    sameFindings: same,
    findings: singleFindings.length,
  };
  console.log(JSON.stringify(report, null, 2));
  process.exitCode = same && stopsAtDefault ? 0 : 1;
} finally {
  rmSync(tmp, { recursive: true, force: true });
}
