#!/usr/bin/env node
// Times a large synthetic policy through the installed wrapper in this checkout
// (bin/rulecheck.mjs, which needs target/release/rulecheck-evaluator).
//
//   node tools/bench-rules.mjs [--rules N] [--files N] [--runs N]
//
// It creates a throwaway Git repository with N rule files (default 360) in nested
// package directories, 30 per package and cycling through literal, regex,
// forbidden-path and TypeScript syntax rules, plus N source files per package
// (default 20, 8 KiB each) with one violation per rule in every other package.
// It reports the median wall time (of --runs, default 3) of the builder alone and of
// `validate`, `test`, a working-tree `check` and a `check --commit`. It exits 0 when
// the two checks report the same findings and 1 otherwise. Every file and rule is
// synthetic.
import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const install = fileURLToPath(new URL("..", import.meta.url));
const wrapper = join(install, "bin", "rulecheck.mjs");
const builder = join(install, "tools", "build.mjs");
const PER_PACKAGE = 30;
const KINDS = ["lit", "re", "path", "syn"];

function option(name, fallback) {
  const index = process.argv.indexOf(name);
  if (index < 0) return fallback;
  const value = Number(process.argv[index + 1]);
  if (!Number.isInteger(value) || value < 1) throw new Error(`${name} requires a positive integer`);
  return value;
}

const rules = option("--rules", 360);
const files = option("--files", 20);
const runs = option("--runs", 3);
const packages = Math.ceil(rules / PER_PACKAGE);
const pkg = (n) => `pkg-${String(Math.floor(n / PER_PACKAGE)).padStart(2, "0")}`;

function run(cmd, args, cwd, codes = [0]) {
  const result = spawnSync(cmd, args, { cwd, encoding: "utf8", maxBuffer: 64 * 1024 * 1024 });
  if (result.error || !codes.includes(result.status)) {
    throw new Error(`${cmd} ${args.join(" ")} exited ${result.status}\n${result.stderr}`);
  }
  return result;
}

function write(root, path, data) {
  mkdirSync(dirname(join(root, path)), { recursive: true });
  writeFileSync(join(root, path), data);
}

function rule(n) {
  const id = `scale/${KINDS[n % 4]}-${String(9999 - n).padStart(4, "0")}`;
  const scope = `"${pkg(n)}/src/**/*.ts"`;
  const body = [
    `kind: "text", paths: [${scope}], pattern: { kind: "literal", value: "LIT-${n}-" },
  fixtures: [{ path: "${pkg(n)}/src/x.ts", content: "LIT-${n}-\\n", expect: { line: 1, column: 1 } }],`,
    `kind: "text", paths: [${scope}], pattern: { kind: "regex", value: "RE-${n}-[0-9]+" },
  fixtures: [{ path: "${pkg(n)}/src/x.ts", content: "RE-${n}-7\\n", expect: { line: 1, column: 1 } }],`,
    `kind: "forbidden-path", paths: ["${pkg(n)}/forbidden/f${n}.txt"],
  fixtures: [{ path: "${pkg(n)}/forbidden/f${n}.txt", expect: { line: 1, column: 1 } }],`,
    `kind: "syntax", paths: [${scope}],
  selector: { language: "typescript", node: "comment", ignore: [{ kind: "literal", value: "allowed" }] },
  fixtures: [{ path: "${pkg(n)}/src/x.ts", content: "// c\\n", expect: { line: 1, column: 1 } }],`,
  ][n % 4];
  return `import { defineRule } from "@rulecheck/sdk";

export default defineRule({
  id: "${id}",
  ${body}
  remediation: "Remove the synthetic marker.",
});
`;
}

function median(values) {
  const sorted = [...values].sort((a, b) => a - b);
  return Math.round(sorted[Math.floor(sorted.length / 2)]);
}

function time(cmd, args, cwd, codes) {
  const samples = [];
  let result;
  for (let attempt = 0; attempt < runs; attempt += 1) {
    const started = process.hrtime.bigint();
    result = run(cmd, args, cwd, codes);
    samples.push(Number(process.hrtime.bigint() - started) / 1e6);
  }
  return { result, wallMsMedian: median(samples) };
}

const tmp = mkdtempSync(join(tmpdir(), "rulecheck-bench-rules-"));
try {
  const root = join(tmp, "repo");
  mkdirSync(root);
  run("git", ["init", "-q"], root);
  for (let n = 0; n < rules; n += 1) {
    write(root, `.rulecheck/rules/${pkg(n)}/r${String(n).padStart(4, "0")}.rule.ts`, rule(n));
  }
  const filler = "export const value = 1; /* allowed */\n".repeat(220);
  let violations = 0;
  for (let p = 0; p < packages; p += 1) {
    const dir = `pkg-${String(p).padStart(2, "0")}`;
    for (let f = 0; f < files; f += 1) write(root, `${dir}/src/m${String(f).padStart(3, "0")}.ts`, filler);
    if (p % 2) continue;
    let bad = "// note\n";
    for (let n = p * PER_PACKAGE; n < Math.min(rules, (p + 1) * PER_PACKAGE); n += 1) {
      if (n % 4 === 0) bad += `LIT-${n}-\n`;
      if (n % 4 === 1) bad += `RE-${n}-42\n`;
      if (n % 4 === 2) write(root, `${dir}/forbidden/f${n}.txt`, "x\n");
      violations += 1;
    }
    write(root, `${dir}/src/bad.ts`, bad);
  }
  run("git", ["add", "-A"], root);
  run("git", ["-c", "user.name=Rulecheck Bench", "-c", "user.email=bench@example.invalid",
    "-c", "commit.gpgsign=false", "commit", "-qm", "Synthetic many-rule snapshot"], root);
  const sha = run("git", ["rev-parse", "HEAD"], root).stdout.trim();

  const build = time(process.execPath, [builder, "--root", root], root, [0]);
  const validate = time(process.execPath, [wrapper, "validate"], root, [0]);
  const test = time(process.execPath, [wrapper, "test"], root, [0]);
  const tree = time(process.execPath, [wrapper, "check", "--format", "json"], root, [1]);
  const exact = time(process.execPath, [wrapper, "check", "--commit", sha, "--format", "json"], root, [1]);
  const treeFindings = JSON.parse(tree.result.stdout).findings;
  const same = JSON.stringify(treeFindings) === JSON.stringify(JSON.parse(exact.result.stdout).findings);

  console.log(JSON.stringify({
    snapshot: {
      rules,
      packages,
      sourceFiles: packages * files,
      policyBytes: Buffer.byteLength(build.result.stdout),
      commit: sha,
    },
    wallMsMedian: {
      build: build.wallMsMedian,
      validate: validate.wallMsMedian,
      test: test.wallMsMedian,
      checkWorkingTree: tree.wallMsMedian,
      checkCommit: exact.wallMsMedian,
    },
    findings: treeFindings.length,
    expectedFindings: violations,
    sameFindings: same,
  }, null, 2));
  process.exitCode = same && treeFindings.length === violations ? 0 : 1;
} finally {
  rmSync(tmp, { recursive: true, force: true });
}
