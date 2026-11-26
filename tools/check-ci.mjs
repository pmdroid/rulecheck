import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const builder = fileURLToPath(new URL("./build.mjs", import.meta.url));
const [root, baseSha, candidateSha, evaluator, ...extra] = process.argv.slice(2);
const toolSha = process.env.RULECHECK_TOOL_SHA ?? null;
const policyReviews = process.env.RULECHECK_POLICY_REVIEWS;
const policyOwner = "pmdroid";
const fullSha = /^(?:[0-9a-f]{40}|[0-9a-f]{64})$/;
const outcomes = ["pass", "violation", "error"];
const evidence = {
  version: 1,
  baseSha: baseSha ?? null,
  candidateSha: candidateSha ?? null,
  tools: { sourceSha: toolSha, node: process.version, git: null, builder: null, evaluator: null },
  phases: {},
  effectivePolicyDiff: null,
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

function effectivePolicy(raw) {
  const policy = JSON.parse(raw);
  const rules = new Map(policy.rules.map((rule) => [
    rule.id,
    canonical({ ...rule, paths: [...new Set(rule.paths)].sort() }),
  ]));
  const input = JSON.stringify({ version: policy.version, rules: [...rules.keys()].sort().map((id) => rules.get(id)) });
  const digest = `sha256:${createHash("sha256").update(input).digest("hex")}`;
  return { rules, input, digest };
}

function evaluate(phase, policySha, testFixtures) {
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
  record(`${phase}-check`, policySha, policy.digest, run(resolve(evaluator), check, policy.input));
  return policy;
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

  const base = evaluate("base", baseSha, false);
  const proposed = evaluate("proposed", candidateSha, true);
  if (base && proposed) evidence.effectivePolicyDiff = diff(base, proposed);
  if (policyReviews !== undefined) {
    record("proposed-owner-review", candidateSha, proposed?.digest ?? null, checkOwnerReview());
  }
} catch (error) {
  record("setup", null, null, { outcome: "error", error: error.message });
}

process.exitCode = Math.max(...Object.values(evidence.phases).map(({ outcome }) => outcomes.indexOf(outcome)));
evidence.outcome = outcomes[process.exitCode];
process.stdout.write(`${JSON.stringify(evidence, null, 2)}\n`);
