// First-run compiler setup from a trusted mirror or a preseeded tarball.
// Run with RULECHECK_INSTALLED_BIN set to a Cargo-installed `rulecheck`; the
// tests fetch the pinned TypeScript tarball once from the public registry.
import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { before, after, test } from "node:test";

const configured = process.env.RULECHECK_INSTALLED_BIN;
const cli = configured && resolve(configured);
const skip = !cli;
const root = fileURLToPath(new URL("..", import.meta.url));
const lock = JSON.parse(readFileSync(join(root, "package-lock.json"), "utf8"));
const pinned = lock.packages["node_modules/typescript"];
// Any network access during setup goes to these dead proxies and fails.
const offline = { HTTPS_PROXY: "http://127.0.0.1:9", HTTP_PROXY: "http://127.0.0.1:9",
  https_proxy: "http://127.0.0.1:9", http_proxy: "http://127.0.0.1:9", NO_PROXY: "", no_proxy: "" };

let tmp;
let tarball;
before(async () => {
  if (skip) return;
  tmp = mkdtempSync(join(tmpdir(), "rulecheck-sources-"));
  const response = await fetch(pinned.resolved);
  assert.equal(response.status, 200);
  tarball = Buffer.from(await response.arrayBuffer());
  assert.equal(`sha512-${createHash("sha512").update(tarball).digest("base64")}`, pinned.integrity);
  writeFileSync(join(tmp, "typescript.tgz"), tarball);
  writeFileSync(join(tmp, "corrupt.tgz"), Buffer.concat([tarball.subarray(0, 4096), Buffer.alloc(16)]));
});
after(() => tmp && rmSync(tmp, { recursive: true, force: true }));

// A synthetic repository whose own npm config points somewhere that must never be used.
function repository(name) {
  const cwd = join(tmp, name);
  const write = (path, data) => {
    mkdirSync(dirname(join(cwd, path)), { recursive: true });
    writeFileSync(join(cwd, path), data);
  };
  write(".npmrc", "registry=http://127.0.0.1:1/\n//127.0.0.1:1/:_authToken=repo-token\n");
  write("config/app.toml", "legacy_mode = false\n");
  write(".rulecheck/rules/legacy.rule.ts", readFileSync(join(root, ".rulecheck/rules/no-legacy-mode.rule.ts")));
  spawnSync("git", ["init", "-q"], { cwd });
  spawnSync("git", ["add", "-A"], { cwd });
  return cwd;
}

// spawn, not spawnSync: the mirror runs in this process and must keep serving.
function rulecheck(cwd, cache, extraEnv) {
  return new Promise((resolvePromise, reject) => {
    const child = spawn(cli, ["test"], {
      cwd, env: { ...process.env, XDG_CACHE_HOME: cache, ...extraEnv }, stdio: ["ignore", "pipe", "pipe"],
    });
    let stdout = "";
    let stderr = "";
    child.stdout.on("data", (chunk) => { stdout += chunk; });
    child.stderr.on("data", (chunk) => { stderr += chunk; });
    const timer = setTimeout(() => child.kill(), 90_000);
    child.on("error", reject);
    child.on("close", (status) => { clearTimeout(timer); resolvePromise({ status, stdout, stderr }); });
  });
}

const installed = (cache) => existsSync(join(cache, "rulecheck")) && readdirSync(join(cache, "rulecheck"))
  .some((entry) => existsSync(join(cache, "rulecheck", entry, "node_modules/typescript/package.json")));

// A registry mirror under a path prefix that serves the tarball with a bearer token.
async function mirror(body, token) {
  const requests = [];
  const server = createServer((req, res) => {
    requests.push({ url: req.url, authorization: req.headers.authorization ?? "" });
    if (token && req.headers.authorization !== `Bearer ${token}`) {
      res.writeHead(401).end();
    } else if (req.url === "/npm/typescript/-/typescript-5.9.3.tgz") {
      res.writeHead(200, { "content-type": "application/octet-stream" }).end(body);
    } else {
      res.writeHead(404).end();
    }
  });
  await new Promise((done) => server.listen(0, "127.0.0.1", done));
  const url = `http://127.0.0.1:${server.address().port}/npm/`;
  return { url, requests, close: () => new Promise((done) => server.close(done)) };
}

test("a trusted mirror installs the pinned compiler with credentials from outside the repository", { skip }, async () => {
  const server = await mirror(tarball, "mirror-secret");
  try {
    const cwd = repository("mirror");
    const cache = join(tmp, "cache-mirror");
    const userconfig = join(tmp, "mirror.npmrc");
    writeFileSync(userconfig, `${server.url.replace("http:", "")}:_authToken=mirror-secret\n`);
    const result = await rulecheck(cwd, cache, {
      ...offline, NO_PROXY: "127.0.0.1", no_proxy: "127.0.0.1",
      RULECHECK_NPM_REGISTRY: server.url, RULECHECK_NPM_USERCONFIG: userconfig,
    });
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.stdout, /7 fixtures passed/);
    assert.ok(server.requests.some((r) => r.url === "/npm/typescript/-/typescript-5.9.3.tgz"));
    assert.ok(server.requests.every((r) => !r.authorization.includes("repo-token")));
    assert.ok(!result.stderr.includes("mirror-secret"));
    assert.ok(installed(cache));
  } finally {
    await server.close();
  }
});

test("a mirror that serves a different tarball fails the lockfile integrity check", { skip }, async () => {
  const server = await mirror(readFileSync(join(tmp, "corrupt.tgz")));
  try {
    const cache = join(tmp, "cache-mismatch");
    const result = await rulecheck(repository("mismatch"), cache, {
      ...offline, NO_PROXY: "127.0.0.1", no_proxy: "127.0.0.1", RULECHECK_NPM_REGISTRY: server.url,
    });
    assert.equal(result.status, 2);
    assert.match(result.stderr, /EINTEGRITY|integrity/i);
    assert.match(result.stderr, /does not fall back to the public npm registry/);
    assert.ok(!installed(cache));
  } finally {
    await server.close();
  }
});

test("an unreachable mirror fails instead of falling back to public npm", { skip }, async () => {
  const server = await mirror(tarball);
  const url = server.url;
  await server.close();
  const cache = join(tmp, "cache-down");
  // No proxies: public npm would be reachable if Rulecheck fell back to it.
  const result = await rulecheck(repository("down"), cache, { RULECHECK_NPM_REGISTRY: url });
  assert.equal(result.status, 2);
  assert.match(result.stderr, /RULECHECK_NPM_REGISTRY http:\/\/127\.0\.0\.1:\d+\/npm\/ failed; Rulecheck does not fall back/);
  assert.ok(!installed(cache));
});

test("invalid source settings fail before npm runs", { skip }, async () => {
  const cwd = repository("invalid");
  const cache = join(tmp, "cache-invalid");
  const inRepo = join(cwd, "mirror.npmrc");
  writeFileSync(inRepo, "");
  const cases = [
    [{ RULECHECK_NPM_REGISTRY: "https://user:pass@mirror.example/" }, /put credentials in the file named by RULECHECK_NPM_USERCONFIG/],
    [{ RULECHECK_NPM_REGISTRY: "ftp://mirror.example/" }, /must be an http:\/\/ or https:\/\/ URL/],
    [{ RULECHECK_NPM_REGISTRY: "https://mirror.example/", RULECHECK_NPM_USERCONFIG: inRepo }, /must be outside the repository being checked/],
    [{ RULECHECK_NPM_USERCONFIG: join(tmp, "mirror.npmrc") }, /RULECHECK_NPM_USERCONFIG needs RULECHECK_NPM_REGISTRY/],
    [{ RULECHECK_COMPILER_TARBALL: "typescript.tgz" }, /must be the absolute path of an existing file/],
    [{ RULECHECK_COMPILER_TARBALL: join(tmp, "typescript.tgz"), RULECHECK_NPM_REGISTRY: "https://mirror.example/" }, /not both/],
  ];
  writeFileSync(join(tmp, "mirror.npmrc"), "");
  for (const [settings, message] of cases) {
    const result = await rulecheck(cwd, cache, { ...offline, ...settings });
    assert.equal(result.status, 2, JSON.stringify(settings));
    assert.match(result.stderr, message);
    assert.ok(!result.stderr.includes("pass@"));
  }
  assert.ok(!installed(cache));
});

test("a preseeded tarball installs without network access", { skip }, async () => {
  const cache = join(tmp, "cache-offline");
  const result = await rulecheck(repository("offline"), cache, {
    ...offline, RULECHECK_COMPILER_TARBALL: join(tmp, "typescript.tgz"),
  });
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /7 fixtures passed/);
  assert.ok(installed(cache));
  // The cache is then reused with no source configured and no network.
  const again = await rulecheck(repository("offline-again"), cache, offline);
  assert.equal(again.status, 0, again.stderr);
  assert.doesNotMatch(again.stderr, /setting up/);
});

test("a preseeded tarball with the wrong checksum is rejected", { skip }, async () => {
  const cache = join(tmp, "cache-corrupt");
  const result = await rulecheck(repository("corrupt"), cache, {
    ...offline, RULECHECK_COMPILER_TARBALL: join(tmp, "corrupt.tgz"),
  });
  assert.equal(result.status, 2);
  assert.match(result.stderr, new RegExp(`does not match the pinned TypeScript 5\\.9\\.3: expected ${pinned.integrity.replace(/[+/]/g, "\\$&")}, got sha512-`));
  assert.ok(!installed(cache));
});

test("with no network and no configured source, setup fails with guidance", { skip }, async () => {
  const cache = join(tmp, "cache-nonet");
  const result = await rulecheck(repository("nonet"), cache, offline);
  assert.equal(result.status, 2);
  assert.match(result.stderr, /from https:\/\/registry\.npmjs\.org\/ failed; .*set RULECHECK_NPM_REGISTRY or RULECHECK_COMPILER_TARBALL/);
  assert.ok(!installed(cache));
});
