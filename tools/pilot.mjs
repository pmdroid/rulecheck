#!/usr/bin/env node
// Adoption pilot on a synthetic large repository: one run against per-prefix runs,
// and exact finding counts by rule and path against an oracle.
//
//   node tools/pilot.mjs [--prefixes N] [--files N] [--file-kib N] [--runs N]
//                        [--time-budget-ms N] [--install DIR]
//
// It creates a throwaway Git repository with N rule prefixes (default 8, svc01/ to
// svcNN/), each owning --files text files (default 12) of --file-kib KiB (default 512)
// under services/svcNN/, plus one shared/ rule over every service. Each prefix has a
// literal rule, a regex rule and a forbidden-path rule. The generator seeds existing
// findings at known lines, including files with two and three findings of one rule
// and files with findings of several rules, and keeps the expected counts by rule and
// path as the oracle. Rulecheck's own output never feeds the oracle.
//
// Throughput: the same rules are checked against the committed baseline with the
// installed wrapper in --install (default: this checkout, which needs
// target/release/rulecheck-evaluator), once as a single run and once as one
// `--id-prefix` run per prefix. Each approach gets a cold run in a fresh clone, then
// --runs warm runs (default 3, median reported). The report gives wall time, builder
// starts (counted from the Node.js processes that actually start tools/build.mjs),
// the scoped text bytes each approach reads, and whether every run completed.
// --time-budget-ms (default 10000) is the budget a single run should meet; a policy
// cache is only worth considering when it does not.
//
// Parity: single-run, per-prefix and --commit findings, and the recorded baseline,
// must equal the oracle exactly by rule and path. Then, on a file with two findings
// of one rule: one more finding fails the baseline check, a scan stopped at the text
// budget cannot pass, record or change the baseline, and a clean-up passes, fails
// --strict-baseline until regenerated, and lowers that path's count by one.
//
// It prints one JSON report and exits 0 when every parity and baseline check passes
// and every run completed, and 1 otherwise. Missing the time budget is reported, not
// failed. Every file, rule and number is synthetic.
import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const MiB = 1024 * 1024;
const BASELINE = ".rulecheck/baseline.json";
const FILLER = "lorem ipsum dolor sit amet\n";

function argument(name) {
  const index = process.argv.indexOf(name);
  return index < 0 ? undefined : process.argv[index + 1];
}

function option(name, fallback) {
  const raw = argument(name);
  if (raw === undefined) return fallback;
  const value = Number(raw);
  if (!Number.isInteger(value) || value < 1) throw new Error(`${name} requires a positive integer`);
  return value;
}

const prefixes = option("--prefixes", 8);
const files = option("--files", 12);
const fileBytes = option("--file-kib", 512) * 1024;
const runs = option("--runs", 3);
const timeBudgetMs = option("--time-budget-ms", 10_000);
const install = realpathSync(resolve(argument("--install") ?? fileURLToPath(new URL("..", import.meta.url))));
const wrapper = join(install, "bin", "rulecheck.mjs");
const builder = join(install, "tools", "build.mjs");
if (prefixes > 99) throw new Error("--prefixes is at most 99");
if (files < 5) throw new Error("--files must be at least 5, so every seeded count occurs");

// ---------------------------------------------------------------- the snapshot

const services = Array.from({ length: prefixes }, (_, n) => `svc${String(n + 1).padStart(2, "0")}`);
const SHARED = "shared/no-inline-token";

// Seeded findings per file, by rule. Index f is the file number in its service.
const DEBUG_COUNTS = [0, 1, 2, 0, 3];
const seeds = {
  debug: (s, f) => DEBUG_COUNTS[f % DEBUG_COUNTS.length],
  fixme: (s, f) => (f % 3 === 0 ? 1 : 0),
  token: (s, f) => (s % 2 === 0 && f === files - 1 ? 1 : 0),
};
const MARKERS = {
  debug: "keep DEBUG_MARKER out\n",
  fixme: "see FIXME(owner) later\n",
  token: "export SECRET_TOKEN=synthetic\n",
};

// Three rule files per service, one rule each.
function rules(service) {
  const scope = `services/${service}/**/*.txt`;
  const head = `import { defineRule } from "@rulecheck/sdk";\n\nexport default defineRule({`;
  return {
    "no-debug-marker": `${head}
  id: "${service}/no-debug-marker",
  kind: "text",
  paths: ["${scope}"],
  pattern: { kind: "literal", value: "DEBUG_MARKER" },
  remediation: "Remove the debug marker.",
  fixtures: [
    { path: "services/${service}/x.txt", content: "DEBUG_MARKER DEBUG_MARKER\\n", expectAll: [{ line: 1, column: 1 }, { line: 1, column: 14 }] },
    { path: "services/${service}/y.txt", content: "clean\\n", expect: null },
  ],
});
`,
    "no-owned-fixme": `${head}
  id: "${service}/no-owned-fixme",
  kind: "text",
  paths: ["${scope}"],
  pattern: { kind: "regex", value: "FIXME\\\\([a-z]+\\\\)" },
  remediation: "Resolve the FIXME or file it.",
  fixtures: [
    { path: "services/${service}/x.txt", content: "a FIXME(owner)\\n", expect: { line: 1, column: 3 } },
    { path: "services/${service}/y.txt", content: "FIXME()\\n", expect: null },
  ],
});
`,
    "no-scratch-files": `${head}
  id: "${service}/no-scratch-files",
  kind: "forbidden-path",
  paths: ["services/${service}/**/*.tmp"],
  remediation: "Delete the scratch file.",
  fixtures: [
    { path: "services/${service}/a.tmp", expect: { line: 1, column: 1 } },
    { path: "services/${service}/a.txt", expect: null },
  ],
});
`,
  };
}

const SHARED_RULE = `import { defineRule } from "@rulecheck/sdk";

export default defineRule({
  id: "${SHARED}",
  kind: "text",
  paths: ["services/**/*.txt"],
  pattern: { kind: "literal", value: "SECRET_TOKEN=" },
  remediation: "Read the token from the environment.",
  fixtures: [
    { path: "services/a/x.txt", content: "SECRET_TOKEN=x\\n", expect: { line: 1, column: 1 } },
    { path: "services/a/y.txt", content: "token\\n", expect: null },
  ],
});
`;

function ruleIds(service) {
  return { debug: `${service}/no-debug-marker`, fixme: `${service}/no-owned-fixme`, token: SHARED };
}

// The text of one service file with the given seeded counts, padded to fileBytes,
// and the expected position of every finding. Markers come first, one per line.
function text(service, counts) {
  const ids = ruleIds(service);
  let head = "";
  const findings = [];
  let line = 1;
  for (const kind of ["debug", "fixme", "token"]) {
    for (let n = 0; n < counts[kind]; n += 1) {
      const column = MARKERS[kind].indexOf(kind === "debug" ? "DEBUG" : kind === "fixme" ? "FIXME" : "SECRET") + 1;
      findings.push({ ruleId: ids[kind], line, column });
      head += MARKERS[kind];
      line += 1;
    }
  }
  const data = Buffer.alloc(fileBytes, FILLER);
  data.write(head);
  return { data, findings };
}

function write(root, path, data) {
  mkdirSync(dirname(join(root, path)), { recursive: true });
  writeFileSync(join(root, path), data);
}

function run(cmd, args, cwd, { codes = [0], env } = {}) {
  const result = spawnSync(cmd, args, { cwd, env, encoding: "utf8", maxBuffer: 256 * MiB });
  if (result.error || !codes.includes(result.status)) {
    throw new Error(`${cmd} ${args.join(" ")} exited ${result.status}\n${result.stderr}`);
  }
  return result;
}

function git(root, args) {
  return run("git", args, root).stdout.trim();
}

function commit(root, message) {
  git(root, ["add", "-A"]);
  git(root, ["-c", "user.name=Rulecheck Pilot", "-c", "user.email=pilot@example.invalid",
    "-c", "commit.gpgsign=false", "commit", "-qm", message]);
  return git(root, ["rev-parse", "HEAD"]);
}

// Writes the snapshot and returns the oracle: expected findings, and the scoped text
// bytes of each rule prefix.
function snapshot(root) {
  git(root, ["init", "-q"]);
  write(root, ".rulecheck/rules/shared.rule.ts", SHARED_RULE);
  const findings = [];
  const files_ = new Map();
  let scopedBytes = 0;
  services.forEach((service, s) => {
    for (const [name, source] of Object.entries(rules(service))) {
      write(root, `.rulecheck/rules/${service}/${name}.rule.ts`, source);
    }
    for (let f = 0; f < files; f += 1) {
      const path = `services/${service}/src/${String(f).padStart(4, "0")}.txt`;
      const counts = { debug: seeds.debug(s, f), fixme: seeds.fixme(s, f), token: seeds.token(s, f) };
      const file = text(service, counts);
      write(root, path, file.data);
      files_.set(path, { service, counts });
      scopedBytes += file.data.length;
      findings.push(...file.findings.map((finding) => ({ ...finding, path })));
    }
    // One forbidden scratch file in every third service.
    if (s % 3 === 0) {
      const path = `services/${service}/build/cache.tmp`;
      write(root, path, "scratch\n");
      findings.push({ ruleId: `${service}/no-scratch-files`, path, line: 1, column: 1 });
    }
  });
  // Every service rule reads its own service; the shared rule reads all of them.
  const perServiceBytes = files * fileBytes;
  const prefixBytes = Object.fromEntries([
    ...services.map((service) => [`${service}/`, perServiceBytes]),
    ["shared/", scopedBytes],
  ]);
  return { findings, files: files_, scopedBytes, prefixBytes };
}

// ---------------------------------------------------------------- measurement

function rulecheck(root, args, { codes = [0, 1, 2], env } = {}) {
  const started = process.hrtime.bigint();
  const result = run(process.execPath, [wrapper, ...args], root, { codes, env });
  return { result, ms: Number(process.hrtime.bigint() - started) / 1e6 };
}

function json(root, args, options) {
  const { result, ms } = rulecheck(root, [...args, "--format", "json"], options);
  const document = JSON.parse(result.stdout);
  if (document.exitCode !== result.status) throw new Error(`exitCode ${document.exitCode} != ${result.status}`);
  return { document, ms, stderr: result.stderr };
}

function complete(document) {
  return document.exitCode !== 2 && document.partial === undefined;
}

function median(values) {
  const sorted = [...values].sort((a, b) => a - b);
  return sorted[Math.floor(sorted.length / 2)];
}

const runPrefixes = [...services.map((service) => `${service}/`), "shared/"];

// One approach: a list of wrapper invocations that together check every rule.
function invocations(approach, extra) {
  if (approach === "oneRun") return [extra];
  return runPrefixes.map((prefix) => [...extra, "--id-prefix", prefix]);
}

function measure(approach, origin, oracle, budget) {
  const clone = mkdtempSync(join(tmpdir(), `rulecheck-pilot-${approach}-`));
  try {
    git(tmpdir(), ["clone", "-q", "--no-hardlinks", origin, clone]);
    const args = ["check", "--baseline", BASELINE];
    if (budget) args.push("--text-budget", String(budget));
    const once = () => {
      let ms = 0;
      let completed = true;
      for (const one of invocations(approach, args)) {
        const result = json(clone, one);
        ms += result.ms;
        completed &&= complete(result.document) && result.document.exitCode === 0;
      }
      return { ms, completed };
    };
    const cold = once();
    const warm = Array.from({ length: runs }, once);
    // Count builder starts in a separate, untimed pass: every Node.js process the
    // wrapper starts loads this module first and logs its entry script.
    const log = join(clone, ".git", "rulecheck-pilot-node.log");
    const hook = join(clone, ".git", "rulecheck-pilot-hook.mjs");
    writeFileSync(hook, `import { appendFileSync } from "node:fs";\nappendFileSync(${JSON.stringify(log)}, (process.argv[1] ?? "") + "\\n");\n`);
    writeFileSync(log, "");
    const env = { ...process.env, NODE_OPTIONS: `${process.env.NODE_OPTIONS ?? ""} --import=${pathToFileURL(hook)}`.trim() };
    for (const one of invocations(approach, args)) json(clone, one, { env });
    const builderStarts = readFileSync(log, "utf8").split("\n").filter((line) => line && realpathSync(line) === builder).length;
    const selected = approach === "oneRun" ? [oracle.scopedBytes] : runPrefixes.map((prefix) => oracle.prefixBytes[prefix]);
    return {
      invocations: invocations(approach, args).length,
      builderStarts,
      scopedTextBytesRead: selected.reduce((a, b) => a + b, 0),
      completed: cold.completed && warm.every((one) => one.completed),
      coldMs: Math.round(cold.ms),
      warmMsMedian: Math.round(median(warm.map((one) => one.ms))),
    };
  } finally {
    rmSync(clone, { recursive: true, force: true });
  }
}

// ---------------------------------------------------------------- parity

function tally(findings) {
  const counts = {};
  for (const { ruleId, path } of findings) {
    counts[ruleId] ??= {};
    counts[ruleId][path] = (counts[ruleId][path] ?? 0) + 1;
  }
  return counts;
}

function sortedCounts(counts) {
  return Object.fromEntries(Object.keys(counts).sort().map((id) =>
    [id, Object.fromEntries(Object.keys(counts[id]).sort().map((path) => [path, counts[id][path]]))]));
}

function sameJson(a, b) {
  return JSON.stringify(a) === JSON.stringify(b);
}

function locations(findings) {
  return findings.map(({ ruleId, path, line, column }) => `${path}:${line}:${column}:${ruleId}`).sort();
}

// Differences between two count maps, as { ruleId, path, expected, actual }.
function mismatches(expected, actual) {
  const out = [];
  const ids = new Set([...Object.keys(expected), ...Object.keys(actual)]);
  for (const id of [...ids].sort()) {
    const paths = new Set([...Object.keys(expected[id] ?? {}), ...Object.keys(actual[id] ?? {})]);
    for (const path of [...paths].sort()) {
      const e = expected[id]?.[path] ?? 0;
      const a = actual[id]?.[path] ?? 0;
      if (e !== a) out.push({ ruleId: id, path, expected: e, actual: a });
    }
  }
  return out;
}

const checks = [];
function verify(name, pass, detail) {
  checks.push(detail === undefined ? { name, pass: Boolean(pass) } : { name, pass: Boolean(pass), detail });
}

function baselineCounts(text) {
  const document = JSON.parse(text);
  return Object.fromEntries(Object.entries(document.rules).map(([id, entry]) => [id, entry.paths]));
}

function parity(root, oracle, budget) {
  const expected = sortedCounts(tally(oracle.findings));
  const budgetArgs = budget ? ["--text-budget", String(budget)] : [];
  const sha = git(root, ["rev-parse", "HEAD"]);

  const one = json(root, ["check", "--all-matches", ...budgetArgs]).document;
  verify("one run completes", complete(one), { exitCode: one.exitCode, partial: one.partial ?? null });
  verify("one run counts equal the oracle by rule and path", sameJson(sortedCounts(tally(one.findings)), expected),
    mismatches(expected, tally(one.findings)));
  verify("one run finding positions equal the oracle", sameJson(locations(one.findings), locations(oracle.findings)));

  const split = [];
  let splitComplete = true;
  for (const prefix of runPrefixes) {
    const document = json(root, ["check", "--all-matches", "--id-prefix", prefix]).document;
    splitComplete &&= complete(document);
    split.push(...document.findings);
  }
  verify("per-prefix runs complete", splitComplete);
  verify("per-prefix counts equal the oracle by rule and path", sameJson(sortedCounts(tally(split)), expected),
    mismatches(expected, tally(split)));

  const exact = json(root, ["check", "--all-matches", "--commit", sha, ...budgetArgs]).document;
  verify("--commit counts equal the oracle by rule and path", complete(exact) && sameJson(sortedCounts(tally(exact.findings)), expected),
    mismatches(expected, tally(exact.findings)));

  rulecheck(root, ["baseline", "--baseline", BASELINE, "--write", ...budgetArgs], { codes: [0] });
  const recorded = readFileSync(join(root, BASELINE), "utf8");
  verify("recorded baseline equals the oracle by rule and path", sameJson(baselineCounts(recorded), expected),
    mismatches(expected, baselineCounts(recorded)));
  return { expected, sha: commit(root, "Record the reviewed baseline") };
}

function scenarios(root, oracle, expected, budget) {
  const budgetArgs = budget ? ["--text-budget", String(budget)] : [];
  // The first file with exactly two findings of one rule.
  const [path, target] = [...oracle.files].find(([, file]) => file.counts.debug === 2);
  const ruleId = ruleIds(target.service).debug;
  const set = (debug) => write(root, path, text(target.service, { ...target.counts, debug }).data);
  const baselineText = () => readFileSync(join(root, BASELINE), "utf8");
  const check = (extra = []) => json(root, ["check", "--baseline", BASELINE, ...budgetArgs, ...extra]).document;
  const total = oracle.findings.length;
  const original = baselineText();

  verify("same file has two findings of one rule", expected[ruleId]?.[path] === 2, { ruleId, path });

  const clean = check();
  verify("baseline check passes on the snapshot", clean.exitCode === 0 && clean.baseline?.allowedFindings === total
    && clean.baseline.exceeded.length === 0 && clean.baseline.loose.length === 0,
  { exitCode: clean.exitCode, allowedFindings: clean.baseline?.allowedFindings, expected: total });
  const sha = git(root, ["rev-parse", "HEAD"]);
  const exact = check(["--commit", sha, "--strict-baseline"]);
  verify("strict --commit baseline check passes on the snapshot", exact.exitCode === 0, { exitCode: exact.exitCode });

  set(3);
  const added = check();
  verify("one additional finding in the same file fails", added.exitCode === 1
    && sameJson(added.baseline?.exceeded, [{ allowed: 2, count: 3, path, ruleId }])
    && added.findings.length === 3 && added.findings.every((finding) => finding.path === path && finding.ruleId === ruleId),
  { exitCode: added.exitCode, exceeded: added.baseline?.exceeded });

  // A scan stopped at the text budget, with the extra finding still present.
  const stopped = json(root, ["check", "--baseline", BASELINE, "--text-budget", "1"]).document;
  verify("an incomplete scan cannot pass", stopped.exitCode === 2 && stopped.partial?.reason === "text-budget"
    && stopped.baseline === undefined, { exitCode: stopped.exitCode, partial: stopped.partial?.reason ?? null });
  const record = rulecheck(root, ["baseline", "--baseline", BASELINE, "--write", "--text-budget", "1"]).result;
  const scoped = rulecheck(root, ["baseline", "--baseline", BASELINE, "--write", "--text-budget", "1",
    "--id-prefix", `${target.service}/`]).result;
  verify("an incomplete scan cannot record or change the allowance", record.status === 2 && scoped.status === 2
    && baselineText() === original, { exitCodes: [record.status, scoped.status] });
  verify("the allowance still rejects the addition after the stopped scan", check().exitCode === 1);

  set(1);
  const cleaned = check();
  verify("a clean-up passes and reports the loose count", cleaned.exitCode === 0
    && sameJson(cleaned.baseline?.loose, [{ allowed: 2, count: 1, path, ruleId }]),
  { exitCode: cleaned.exitCode, loose: cleaned.baseline?.loose });
  verify("strict mode fails the clean-up until the baseline is tightened", check(["--strict-baseline"]).exitCode === 1);
  rulecheck(root, ["baseline", "--baseline", BASELINE, "--write", ...budgetArgs], { codes: [0] });
  const tightened = baselineCounts(baselineText());
  const lowered = structuredClone(expected);
  lowered[ruleId][path] = 1;
  verify("the clean-up lowers only that path's count by one", sameJson(tightened, lowered), mismatches(lowered, tightened));
  verify("strict mode passes with the tightened baseline", check(["--strict-baseline"]).exitCode === 0);
  set(2);
  verify("restoring the cleaned finding fails the tightened baseline", check().exitCode === 1);
  return { ruleId, path };
}

// ---------------------------------------------------------------- main

const tmp = mkdtempSync(join(tmpdir(), "rulecheck-pilot-"));
try {
  const root = join(tmp, "repo");
  mkdirSync(root);
  const oracle = snapshot(root);
  commit(root, "Synthetic pilot snapshot");
  // The single run fits the default 64 MiB budget unless the snapshot is larger.
  const budget = oracle.scopedBytes > 64 * MiB ? Math.ceil(oracle.scopedBytes / MiB) : null;
  if (budget > 1024) throw new Error("the snapshot is over the 1024 MiB text budget limit");

  const { expected, sha } = parity(root, oracle, budget);
  const oneRun = measure("oneRun", root, oracle, budget);
  const perPrefix = measure("perPrefix", root, oracle, budget);
  git(root, ["reset", "-q", "--hard", sha]);
  const scenario = scenarios(root, oracle, expected, budget);

  const meets = oneRun.coldMs <= timeBudgetMs && oneRun.warmMsMedian <= timeBudgetMs;
  verify("one run completes in every timed run", oneRun.completed);
  verify("per-prefix runs complete in every timed run", perPrefix.completed);
  verify("one run starts the builder once", oneRun.builderStarts === 1, { builderStarts: oneRun.builderStarts });
  verify("per-prefix runs start the builder once per prefix", perPrefix.builderStarts === runPrefixes.length,
    { builderStarts: perPrefix.builderStarts });

  const report = {
    snapshot: {
      prefixes: runPrefixes.length,
      rules: services.length * 3 + 1,
      files: services.length * files,
      scopedTextBytes: oracle.scopedBytes,
      findings: oracle.findings.length,
      rulePathCounts: Object.values(expected).reduce((n, paths) => n + Object.keys(paths).length, 0),
      textBudgetMiB: budget ?? 64,
    },
    timeBudgetMs,
    oneRun: { ...oneRun, meetsTimeBudget: meets },
    perPrefix,
    policyCacheJustified: !meets,
    sameFileTarget: scenario,
    checks,
    passed: checks.every((one) => one.pass),
  };
  console.log(JSON.stringify(report, null, 2));
  process.exitCode = report.passed ? 0 : 1;
} finally {
  rmSync(tmp, { recursive: true, force: true });
}
