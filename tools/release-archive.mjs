#!/usr/bin/env node
// Packages the two Rulecheck executables into a reproducible .tar.gz and writes
// the SHA-256 manifest that release downloads are pinned against.
//
//   node tools/release-archive.mjs pack --target TRIPLE --bin-dir DIR --out DIR
//       --commit SHA [--tag rust-vX.Y.Z] [--source-date-epoch N] [--rustc "rustc ..."]
//   node tools/release-archive.mjs manifest --out DIR [--complete true]
//
// The archive holds rulecheck-VERSION-TARGET/{bin/rulecheck, bin/rulecheck-evaluator,
// LICENSE, README.md, PROVENANCE.json}. Entries are sorted, owned by 0:0 and
// stamped with the commit time, so the same binaries always give the same bytes.
import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import { readFileSync, readdirSync, writeFileSync, mkdirSync, statSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { gzipSync } from "node:zlib";

const root = resolve(fileURLToPath(new URL("..", import.meta.url)));

// The release targets, one per supported CI runner. Keep docs/src/content/docs/install.md in step.
export const TARGETS = {
  "x86_64-unknown-linux-gnu": { runner: "ubuntu-latest", os: "linux", arch: "x64" },
  "aarch64-apple-darwin": { runner: "macos-latest", os: "macos", arch: "arm64" },
};
const BINARIES = ["rulecheck", "rulecheck-evaluator"];
const ARCHIVE = /^rulecheck-(\d+\.\d+\.\d+)-([a-z0-9_]+-[a-z0-9_]+-[a-z0-9_]+(?:-[a-z0-9_]+)?)\.tar\.gz$/;

function fail(message) {
  console.error(`release-archive: ${message}`);
  process.exit(2);
}

function sha256(data) {
  return createHash("sha256").update(data).digest("hex");
}

export function packageVersion(manifest = readFileSync(join(root, "Cargo.toml"), "utf8")) {
  const section = manifest.split(/^\[/m).find((part) => part.startsWith("package]"));
  const version = section?.match(/^version\s*=\s*"([^"]+)"/m)?.[1];
  if (!version) throw new Error("Cargo.toml has no [package] version");
  return version;
}

function header(name, size, mode, mtime) {
  const block = Buffer.alloc(512);
  const field = (offset, length, value) => block.write(value, offset, length, "ascii");
  const octal = (offset, length, value) => field(offset, length, `${value.toString(8).padStart(length - 1, "0")}\0`);
  if (Buffer.byteLength(name) > 100) throw new Error(`archive path too long: ${name}`);
  block.write(name, 0, 100, "utf8");
  octal(100, 8, mode);
  octal(108, 8, 0);
  octal(116, 8, 0);
  octal(124, 12, size);
  octal(136, 12, mtime);
  field(148, 8, "        ");
  field(156, 1, size === 0 && name.endsWith("/") ? "5" : "0");
  field(257, 6, "ustar\0");
  field(263, 2, "00");
  let sum = 0;
  for (const byte of block) sum += byte;
  field(148, 8, `${sum.toString(8).padStart(6, "0")}\0 `);
  return block;
}

// A minimal POSIX ustar writer, so Linux and macOS produce the same layout
// without depending on GNU tar or bsdtar flags.
export function tarGz(entries, mtime) {
  const blocks = [];
  for (const entry of [...entries].sort((a, b) => a.path.localeCompare(b.path, "en"))) {
    const data = entry.data ?? Buffer.alloc(0);
    blocks.push(header(entry.path, data.length, entry.mode, mtime), data);
    const pad = (512 - (data.length % 512)) % 512;
    if (pad) blocks.push(Buffer.alloc(pad));
  }
  blocks.push(Buffer.alloc(1024));
  const gz = gzipSync(Buffer.concat(blocks), { level: 9 });
  gz[9] = 3; // zlib writes a platform OS byte; pin it to Unix.
  return gz;
}

function parseArgs(argv) {
  const options = {};
  for (let i = 0; i < argv.length; i += 2) {
    const key = argv[i];
    if (!key.startsWith("--") || argv[i + 1] === undefined) fail(`expected --name value, got ${key}`);
    options[key.slice(2)] = argv[i + 1];
  }
  return options;
}

function pack(options) {
  const { target, commit } = options;
  if (!TARGETS[target]) fail(`unsupported --target ${target}; use one of ${Object.keys(TARGETS).join(", ")}`);
  if (!/^[0-9a-f]{40}$/.test(commit ?? "")) fail("--commit must be a full 40-character commit SHA");
  if (!options["bin-dir"] || !options.out) fail("--bin-dir and --out are required");
  const version = packageVersion();
  const tag = options.tag || null;
  if (tag !== null && tag !== `rust-v${version}`) fail(`--tag ${tag} does not match Cargo.toml version ${version}`);
  const epoch = Number(options["source-date-epoch"] ?? process.env.SOURCE_DATE_EPOCH);
  if (!Number.isSafeInteger(epoch) || epoch < 0) fail("set --source-date-epoch or SOURCE_DATE_EPOCH to the commit time");

  const binDir = resolve(options["bin-dir"]);
  const files = [];
  for (const name of BINARIES) {
    const path = join(binDir, name);
    if (!statSync(path, { throwIfNoEntry: false })?.isFile()) fail(`missing executable ${path}`);
    files.push({ path: `bin/${name}`, mode: 0o755, data: readFileSync(path) });
  }
  // Refuse to label binaries with a version they don't report.
  const reported = execFileSync(join(binDir, "rulecheck-evaluator"), ["--version"], { encoding: "utf8" }).trim();
  if (reported !== `rulecheck ${version}`) fail(`evaluator reports "${reported}", expected "rulecheck ${version}"`);
  for (const name of ["LICENSE", "README.md"]) {
    files.push({ path: name, mode: 0o644, data: readFileSync(join(root, name)) });
  }

  const provenance = {
    schema: "rulecheck-release-provenance-v1",
    name: "rulecheck",
    version,
    target,
    tag,
    commit,
    sourceDateEpoch: epoch,
    rustc: options.rustc ?? null,
    cargoLockSha256: sha256(readFileSync(join(root, "Cargo.lock"))),
    files: files.map((file) => ({ path: file.path, sha256: sha256(file.data) })),
  };
  files.push({
    path: "PROVENANCE.json",
    mode: 0o644,
    data: Buffer.from(`${JSON.stringify(provenance, null, 2)}\n`),
  });

  const base = `rulecheck-${version}-${target}`;
  const entries = [{ path: `${base}/`, mode: 0o755 }, { path: `${base}/bin/`, mode: 0o755 }];
  for (const file of files) entries.push({ ...file, path: `${base}/${file.path}` });
  const archive = tarGz(entries, epoch);
  const out = resolve(options.out);
  mkdirSync(out, { recursive: true });
  writeFileSync(join(out, `${base}.tar.gz`), archive);
  const line = `${sha256(archive)}  ${base}.tar.gz\n`;
  writeFileSync(join(out, `${base}.tar.gz.sha256`), line);
  process.stdout.write(line);
}

function manifest(options) {
  if (!options.out) fail("--out is required");
  const out = resolve(options.out);
  const archives = readdirSync(out).filter((name) => name.endsWith(".tar.gz")).sort();
  if (archives.length === 0) fail(`no .tar.gz archives in ${out}`);
  const versions = new Set();
  const lines = archives.map((name) => {
    const match = ARCHIVE.exec(name);
    if (!match || !TARGETS[match[2]]) fail(`unexpected archive name ${name}`);
    versions.add(match[1]);
    const line = `${sha256(readFileSync(join(out, name)))}  ${name}`;
    const sidecar = readFileSync(join(out, `${name}.sha256`), "utf8").trim();
    if (sidecar !== line) fail(`${name} no longer matches the checksum recorded when it was packed`);
    return line;
  });
  if (versions.size !== 1) fail(`archives mix versions: ${[...versions].join(", ")}`);
  const missing = Object.keys(TARGETS).filter((t) => !archives.some((name) => name.endsWith(`-${t}.tar.gz`)));
  if (options.complete === "true" && missing.length) fail(`missing archives for ${missing.join(", ")}`);
  writeFileSync(join(out, "SHA256SUMS"), `${lines.join("\n")}\n`);
  process.stdout.write(`${lines.join("\n")}\n`);
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const [command, ...rest] = process.argv.slice(2);
  const options = parseArgs(rest);
  if (command === "pack") pack(options);
  else if (command === "manifest") manifest(options);
  else fail("usage: release-archive.mjs pack|manifest --name value ...");
}
