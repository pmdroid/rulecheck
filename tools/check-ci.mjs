import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const builder = fileURLToPath(new URL("./build.mjs", import.meta.url));
const [root, baseSha, candidateSha, evaluator, ...extra] = process.argv.slice(2);
const toolSha = process.env.RULECHECK_TOOL_SHA ?? null;
const policyReviews = process.env.RULECHECK_POLICY_REVIEWS;
const policyOwner = "pmdroid";
const fullSha = /^(?:[0-9a-f]{40}|[0-9a-f]{64})$/;
const outcomes = ["pass", "violation", "error"];
// The committed baseline of existing findings. Base-check reads it from the
// base commit, so a pull request cannot raise its own allowance.
const baselinePath = ".rulecheck/baseline.json";
const maxBaseline = 4 * 1024 * 1024;
const evidence = {
  version: 1,
  baseSha: baseSha ?? null,
  candidateSha: candidateSha ?? null,
  tools: { sourceSha: toolSha, node: process.version, git: null, builder: null, evaluator: null },
  phases: {},
  effectivePolicyDiff: null,
  baselineChange: null,
};

function run(command, args, input) {
  const { status, error, stdout, stderr } = spawnSync(command, args, {
    input,
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024,
    timeout: 120_000,
    killSignal: "SIGKILL",
  });
  return { outcome: error ? "error" : (outcomes[status] ?? "error"), stdout, stderr, error: error?.message };
}

function record(phase, policySha, policyDigest, result) {
  const { outcome, stdout, stderr, error, ...details } = result;
  evidence.phases[phase] = { policySha, policyDigest, outcome, ...details };
  const entry = evidence.phases[phase];
  if (stdout) entry.stdout = stdout;
  if (stderr) entry.stderr = stderr;
  if (error) entry.error = error;
  return outcome === "pass";
}

function version(command, args) {
  const result = run(command, args);
  if (result.outcome !== "pass" || !result.stdout.trim()) {
    throw new Error(`Could not read tool version: ${result.error ?? result.stderr}`);
  }
  return result.stdout.trim();
}

function canonical(value) {
  if (Array.isArray(value)) return value.map(canonical);
  if (value === null || typeof value !== "object") return value;
  return Object.fromEntries(Object.keys(value).sort().map((key) => [key, canonical(value[key])]));
}

// Runs after the evaluator validated the policy, so every set exists and includes
// have no cycles. A set reached twice is expanded once, as in src/policy.rs.
function resolveExclude(rule, sets) {
  const globs = [...(rule.exclude ?? [])];
  const visited = new Set();
  const visit = (name) => {
    if (visited.has(name)) return;
    visited.add(name);
    const set = sets.get(name);
    globs.push(...set.globs);
    for (const include of set.include ?? []) visit(include);
  };
  for (const name of rule.excludeSets ?? []) visit(name);
  return [...new Set(globs)].sort();
}

function effectivePolicy(raw) {
  const policy = JSON.parse(raw);
  const sets = new Map((policy.sets ?? []).map((set) => [set.name, canonical(set)]));
  const rules = new Map(policy.rules.map((rule) => [
    rule.id,
    canonical({ ...rule, paths: [...new Set(rule.paths)].sort() }),
  ]));
  const ids = [...rules.keys()].sort();
  const names = [...sets.keys()].sort();
  const input = JSON.stringify({
    version: policy.version,
    ...(names.length ? { sets: names.map((name) => sets.get(name)) } : {}),
    rules: ids.map((id) => rules.get(id)),
  });
  const digest = `sha256:${createHash("sha256").update(input).digest("hex")}`;
  // The diff shows each rule with the exclusion globs its sets resolve to, so a
  // set edit lists every rule whose scope it changes.
  const effective = new Map(ids.map((id) => {
    const rule = rules.get(id);
    return [id, rule.excludeSets ? canonical({ ...rule, resolvedExclude: resolveExclude(rule, sets) }) : rule];
  }));
  return { rules: effective, input, digest };
}

function evaluate(phase, policySha, testFixtures, baseline) {
  const build = run(process.execPath, [builder, "--root", resolve(root), "--commit", policySha]);
  record(`${phase}-build`, policySha, null, { ...build, stdout: undefined });
  if (build.outcome !== "pass") return null;

  const validate = run(resolve(evaluator), ["validate"], build.stdout);
  const policy = validate.outcome === "pass" ? effectivePolicy(build.stdout) : null;
  if (!record(`${phase}-validate`, policySha, policy?.digest ?? null, validate)) return null;

  if (testFixtures) {
    record(`${phase}-test`, policySha, policy.digest, run(resolve(evaluator), ["test"], policy.input));
  }
  const check = ["check", "--root", resolve(root), "--commit", candidateSha];
  let details = {};
  if (baseline) {
    check.push("--baseline", baseline.file, ...(baseline.strict ? ["--strict-baseline"] : []));
    const { source, sha, sha256, strict } = baseline;
    details = { baseline: { source, sha, sha256, strict } };
  }
  record(`${phase}-check`, policySha, policy.digest, { ...run(resolve(evaluator), check, policy.input), ...details });
  return policy;
}

const git = (args, options = {}) => spawnSync("git", ["--no-replace-objects", "-C", resolve(root), ...args],
  { maxBuffer: maxBaseline + 1, timeout: 120_000, killSignal: "SIGKILL", ...options });

// Copies the baseline at an exact commit to a private file, or returns null if
// the commit has none. Anything but a regular blob within the size limit throws.
function commitBaseline(label, sha, scratch) {
  const entry = git(["ls-tree", "-z", "--full-tree", sha, "--", baselinePath], { encoding: "utf8" });
  if (entry.error || entry.status !== 0) throw new Error(`Could not list ${baselinePath} at ${sha}`);
  if (!entry.stdout) return null;
  const [mode, type, oid] = entry.stdout.split("\t")[0].split(" ");
  if (!["100644", "100755"].includes(mode) || type !== "blob") {
    throw new Error(`${baselinePath} at ${sha} must be a regular file`);
  }
  const blob = git(["cat-file", "blob", oid]);
  if (blob.error || blob.status !== 0 || blob.stdout.length > maxBaseline) {
    throw new Error(`Could not read ${baselinePath} at ${sha} within ${maxBaseline} bytes`);
  }
  const file = join(scratch, `${label}-baseline.json`);
  writeFileSync(file, blob.stdout);
  const sha256 = `sha256:${createHash("sha256").update(blob.stdout).digest("hex")}`;
  return { file, sha, sha256, bytes: blob.stdout };
}

// True when the only path the candidate changes is the baseline: no code or
// rule moved, so the reviewed baseline can only re-record the base's counts.
function baselineOnly() {
  const changes = git(["diff-tree", "--no-renames", "-r", "--name-only", "-z", baseSha, candidateSha],
    { encoding: "utf8" });
  if (changes.error || changes.status !== 0) throw new Error("Could not list the candidate's changed paths");
  const paths = changes.stdout.split("\0").filter(Boolean);
  return paths.length === 1 && paths[0] === baselinePath;
}

// Per rule and path, the allowances the candidate's baseline raises or lowers.
// raised and lowered are null when either baseline is not valid JSON.
function baselineChange(base, proposed, rebaseline) {
  const parse = (baseline) => {
    try {
      return baseline ? JSON.parse(baseline.bytes).rules ?? {} : {};
    } catch {
      return null;
    }
  };
  const before = parse(base);
  const after = parse(proposed);
  const change = { baseSha256: base?.sha256 ?? null, proposedSha256: proposed?.sha256 ?? null, rebaseline,
    raised: null, lowered: null };
  if (before === null || after === null) return change;
  change.raised = [];
  change.lowered = [];
  const ids = [...new Set([...Object.keys(before), ...Object.keys(after)])].sort();
  for (const ruleId of ids) {
    const old = before[ruleId]?.paths ?? {};
    const now = after[ruleId]?.paths ?? {};
    const matcherChanged = ruleId in before && ruleId in after && before[ruleId].matcher !== after[ruleId].matcher;
    for (const path of [...new Set([...Object.keys(old), ...Object.keys(now)])].sort()) {
      const entry = { ruleId, path, before: old[path] ?? 0, after: now[path] ?? 0, matcherChanged };
      if (entry.after > entry.before) change.raised.push(entry);
      if (entry.after < entry.before) change.lowered.push(entry);
    }
  }
  return change;
}

function diff(base, proposed) {
  const ids = [...new Set([...base.rules.keys(), ...proposed.rules.keys()])].sort();
  const changes = [];
  for (const id of ids) {
    const before = base.rules.get(id) ?? null;
    const after = proposed.rules.get(id) ?? null;
    if (JSON.stringify(before) !== JSON.stringify(after)) changes.push({ id, before, after });
  }
  return { baseDigest: base.digest, proposedDigest: proposed.digest, changes };
}

function checkOwnerReview() {
  const changes = run("git", ["--no-replace-objects", "-C", resolve(root),
    "diff-tree", "--quiet", "--no-renames", "-r", baseSha, candidateSha, "--", ".rulecheck"]);
  if (changes.outcome === "error") return changes;
  if (changes.outcome === "pass") return { outcome: "pass", owner: policyOwner, required: false };

  const details = { owner: policyOwner, required: true };
  try {
    const decisiveStates = new Set(["APPROVED", "CHANGES_REQUESTED", "DISMISSED"]);
    if (policyReviews === undefined) throw new Error("RULECHECK_POLICY_REVIEWS is not set; a rebaseline needs it");
    const review = readFileSync(policyReviews, "utf8").trim().split("\n").filter(Boolean)
      .map((line) => JSON.parse(line))
      .findLast(({ owner, state }) => owner === policyOwner && decisiveStates.has(state));
    const approved = review?.state === "APPROVED" && review.sha === candidateSha;
    return { ...details, outcome: approved ? "pass" : "violation",
      reviewSha: review?.sha ?? null, reviewState: review?.state ?? null };
  } catch (error) {
    return { ...details, outcome: "error", error: `Could not read GitHub reviews: ${error.message}` };
  }
}

try {
  if (!root || !evaluator || extra.length || !fullSha.test(baseSha) || !fullSha.test(candidateSha)
      || (toolSha !== null && !fullSha.test(toolSha))) {
    throw new Error("Usage: node tools/check-ci.mjs REPO BASE_SHA CANDIDATE_SHA EVALUATOR");
  }
  evidence.tools.git = version("git", ["--version"]);
  evidence.tools.builder = version(process.execPath, [builder, "--version"]);
  evidence.tools.evaluator = version(resolve(evaluator), ["--version"]);

  const scratch = mkdtempSync(join(tmpdir(), "rulecheck-baselines-"));
  try {
    let baseBaseline = null;
    let proposedBaseline = null;
    let rebaseline = false;
    let readable = true;
    try {
      baseBaseline = commitBaseline("base", baseSha, scratch);
      proposedBaseline = commitBaseline("proposed", candidateSha, scratch);
      rebaseline = proposedBaseline !== null && baselineOnly();
    } catch (error) {
      readable = false;
      record("baseline-read", null, null, { outcome: "error", error: error.message });
    }
    if (readable) {
      // The trusted allowance is the base commit's. A baseline-only pull request
      // is the reviewed rebaseline path: it changes no code or rule, so base-check
      // uses its baseline, owner review is required, and the strict proposed check
      // makes it equal the counts the base commit already has.
      const trusted = rebaseline ? proposedBaseline : baseBaseline;
      const base = evaluate("base", baseSha, false,
        trusted && { ...trusted, source: rebaseline ? "proposed" : "base", strict: false });
      const proposed = evaluate("proposed", candidateSha, true,
        proposedBaseline && { ...proposedBaseline, source: "proposed", strict: true });
      if (base && proposed) evidence.effectivePolicyDiff = diff(base, proposed);
      if (baseBaseline || proposedBaseline) {
        evidence.baselineChange = baselineChange(baseBaseline, proposedBaseline, rebaseline);
      }
      if (policyReviews !== undefined || rebaseline) {
        record("proposed-owner-review", candidateSha, proposed?.digest ?? null, checkOwnerReview());
      }
    }
  } finally {
    rmSync(scratch, { recursive: true, force: true });
  }
} catch (error) {
  record("setup", null, null, { outcome: "error", error: error.message });
}

process.exitCode = Math.max(...Object.values(evidence.phases).map(({ outcome }) => outcomes.indexOf(outcome)));
evidence.outcome = outcomes[process.exitCode];
process.stdout.write(`${JSON.stringify(evidence, null, 2)}\n`);
