import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { test } from "node:test";
import { packageVersion, TARGETS } from "../tools/release-archive.mjs";

const script = fileURLToPath(new URL("../tools/release-archive.mjs", import.meta.url));
const version = packageVersion();
const commit = "a".repeat(40);
const target = "x86_64-unknown-linux-gnu";
const name = `rulecheck-${version}-${target}`;

function fixture(t, reported = version) {
  const dir = mkdtempSync(join(tmpdir(), "rulecheck-release-"));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  const bin = join(dir, "bin");
  mkdirSync(bin);
  // Stand-ins for the Cargo builds: the packager only runs --version on the evaluator.
  writeFileSync(join(bin, "rulecheck"), "#!/bin/sh\necho frontend\n", { mode: 0o755 });
  writeFileSync(join(bin, "rulecheck-evaluator"), `#!/bin/sh\necho "rulecheck ${reported}"\n`, { mode: 0o755 });
  return { dir, bin };
}

function run(args, expected) {
  const result = spawnSync(process.execPath, [script, ...args], { encoding: "utf8" });
  assert.equal(result.status, expected, `${args.join(" ")}\n${result.stderr}`);
  return result;
}

const pack = (bin, out, extra = []) => run(["pack", "--target", target, "--bin-dir", bin, "--out", out,
  "--commit", commit, "--source-date-epoch", "1700000000", ...extra], 0);

test("archives are reproducible and carry provenance", (t) => {
  const { dir, bin } = fixture(t);
  pack(bin, join(dir, "a"), ["--tag", `rust-v${version}`]);
  pack(bin, join(dir, "b"), ["--tag", `rust-v${version}`]);
  const archive = readFileSync(join(dir, "a", `${name}.tar.gz`));
  assert.deepEqual(archive, readFileSync(join(dir, "b", `${name}.tar.gz`)));

  const listing = spawnSync("tar", ["-tvzf", join(dir, "a", `${name}.tar.gz`)], { encoding: "utf8" });
  assert.equal(listing.status, 0, listing.stderr);
  const rows = listing.stdout.trim().split("\n").map((row) => row.split(/\s+/));
  assert.deepEqual(rows.map((row) => row.at(-1)), [
    `${name}/`, `${name}/bin/`, `${name}/bin/rulecheck`, `${name}/bin/rulecheck-evaluator`,
    `${name}/LICENSE`, `${name}/PROVENANCE.json`, `${name}/README.md`,
  ]);
  assert.deepEqual(rows.map((row) => row[0]).slice(2, 4), ["-rwxr-xr-x", "-rwxr-xr-x"]);

  const out = join(dir, "x");
  mkdirSync(out);
  assert.equal(spawnSync("tar", ["-xzf", join(dir, "a", `${name}.tar.gz`), "-C", out]).status, 0);
  const provenance = JSON.parse(readFileSync(join(out, name, "PROVENANCE.json"), "utf8"));
  assert.equal(provenance.commit, commit);
  assert.equal(provenance.tag, `rust-v${version}`);
  assert.equal(provenance.target, target);
  const digest = createHash("sha256").update(readFileSync(join(out, name, "bin/rulecheck-evaluator"))).digest("hex");
  assert.deepEqual(provenance.files.find((file) => file.path === "bin/rulecheck-evaluator"),
    { path: "bin/rulecheck-evaluator", sha256: digest });

  run(["manifest", "--out", join(dir, "a")], 0);
  const sums = readFileSync(join(dir, "a", "SHA256SUMS"), "utf8");
  assert.equal(sums, `${createHash("sha256").update(archive).digest("hex")}  ${name}.tar.gz\n`);
  // A release manifest must cover every supported target.
  assert.match(run(["manifest", "--out", join(dir, "a"), "--complete", "true"], 2).stderr,
    new RegExp(`missing archives for ${Object.keys(TARGETS).filter((t) => t !== target).join(", ")}`));
});

test("packaging refuses mislabelled or tampered archives", (t) => {
  const { dir, bin } = fixture(t);
  const args = ["pack", "--target", target, "--bin-dir", bin, "--out", join(dir, "o"),
    "--commit", commit, "--source-date-epoch", "1"];
  assert.match(run([...args, "--tag", "rust-v9.9.9"], 2).stderr, /does not match Cargo.toml version/);
  assert.match(run(["pack", "--target", "x86_64-pc-windows-msvc", "--bin-dir", bin, "--out", dir,
    "--commit", commit, "--source-date-epoch", "1"], 2).stderr, /unsupported --target/);
  assert.match(run(["pack", "--target", target, "--bin-dir", bin, "--out", dir,
    "--commit", "main", "--source-date-epoch", "1"], 2).stderr, /full 40-character commit SHA/);

  const stale = fixture(t, "0.0.0-stale");
  assert.match(run(["pack", "--target", target, "--bin-dir", stale.bin, "--out", dir,
    "--commit", commit, "--source-date-epoch", "1"], 2).stderr, /evaluator reports "rulecheck 0.0.0-stale"/);

  pack(bin, join(dir, "o"));
  writeFileSync(join(dir, "o", `${name}.tar.gz`), "tampered");
  assert.match(run(["manifest", "--out", join(dir, "o")], 2).stderr, /no longer matches the checksum/);
});
