import { spawnSync } from "node:child_process";
import { constants, closeSync, fstatSync, lstatSync, opendirSync, openSync, readFileSync, readSync } from "node:fs";
import { join, resolve } from "node:path";
import ts from "typescript";

const maxSource = 128 * 1024;
// Kept equal to MAX_POLICY and MAX_RULES in src/policy.rs, maxPolicy in bin/rulecheck.mjs
// and maxItems in schema/policy-v1.json.
const maxPolicy = 4 * 1024 * 1024;
const maxRules = 1024;
const maxSets = 64;
const maxEntries = 20_000;
const marker = "@pmdroid/rulecheck";
const sdkMarkers = new Set([marker, "@rulecheck/sdk"]);
const rulesDir = ".rulecheck/rules";
// Named exclusion sets: .rulecheck/sets/<name>.set.ts, read as static data like rules.
const setsDir = ".rulecheck/sets";
const SET_NAME = /^[a-z0-9-]+(?:\/[a-z0-9-]+)*$/;
const utf8 = new TextDecoder("utf-8", { fatal: true });
const FULL_SHA = /^(?:[a-f0-9]{40}|[a-f0-9]{64})$/i;
const COMMIT_HINT = '--commit requires a full 40- or 64-character Git commit SHA; resolve it first, e.g. --commit "$(git rev-parse HEAD)"';
const ROOT_HINT = "run from the checkout root or pass --root";
const TOO_MANY_RULES = `${rulesDir}: more than ${maxRules} .rule.ts files; a policy holds at most ${maxRules} rules`;

function readRule(path) {
  const fd = openSync(path, constants.O_RDONLY | constants.O_NOFOLLOW | constants.O_NONBLOCK);
  try {
    if (!fstatSync(fd).isFile()) throw new Error("use a regular rule file");
    const bytes = Buffer.alloc(maxSource + 1);
    let length = 0;
    while (length < bytes.length) {
      const count = readSync(fd, bytes, length, bytes.length - length);
      if (count === 0) return utf8.decode(bytes.subarray(0, length));
      length += count;
    }
    throw new Error("keep the rule file under 128 KiB");
  } finally {
    closeSync(fd);
  }
}

function statIfPresent(path) {
  return lstatSync(path, { throwIfNoEntry: false });
}

function git(root, args, maxBuffer = 8 * 1024 * 1024, input = undefined) {
  const result = spawnSync("git", ["--no-replace-objects", "-C", root, ...args], {
    input,
    maxBuffer,
    timeout: 15_000,
  });
  if (result.error || result.status !== 0) {
    throw new Error(`could not inspect Git checkout: ${result.error?.message ?? result.stderr.toString().trim()}`);
  }
  return result.stdout;
}

// The working-tree root must be the top of a Git checkout, not a subdirectory.
function checkRoot(root) {
  if (!statIfPresent(root)?.isDirectory()) throw new Error(`${root} is not a directory; ${ROOT_HINT}`);
  const prefix = spawnSync("git", ["--no-replace-objects", "-C", root, "rev-parse", "--show-prefix"], { timeout: 15_000 });
  if (prefix.error || prefix.status !== 0) throw new Error(`${root} is not a Git checkout; ${ROOT_HINT}`);
  if (prefix.stdout.toString().trim()) {
    throw new Error(`${root} is a subdirectory of a Git checkout, not its root; ${ROOT_HINT}`);
  }
}

function describe(stat) {
  if (!stat) return "missing entry";
  if (stat.isSymbolicLink()) return "symlink";
  if (stat.isFIFO()) return "FIFO";
  if (stat.isSocket()) return "socket";
  if (stat.isBlockDevice() || stat.isCharacterDevice()) return "device";
  return "non-regular entry";
}

function discover(root) {
  checkRoot(root);

  const gitlinks = new Set();
  const index = git(root, ["ls-files", "--stage", "-z", "--", ".rulecheck"]);
  for (const entry of index.toString().split("\0")) {
    if (entry.startsWith("160000 ")) gitlinks.add(entry.slice(entry.indexOf("\t") + 1));
  }

  const regularDirectory = (path) => !gitlinks.has(path) && statIfPresent(join(root, path))?.isDirectory()
    && !statIfPresent(join(root, path, ".git"));
  for (const path of [".rulecheck", rulesDir]) {
    if (!regularDirectory(path)) throw new Error(`${path}: use a regular directory inside the checkout`);
  }
  // The sets directory is optional, but when present it must be a real directory,
  // so a symlink or nested repository can't swap in sets from elsewhere.
  const hasSets = gitlinks.has(setsDir) || statIfPresent(join(root, setsDir)) !== undefined;
  if (hasSets && !regularDirectory(setsDir)) {
    throw new Error(`${setsDir}: use a regular directory inside the checkout`);
  }

  let entries = 0;
  let found = [];
  let suffix = ".rule.ts";
  let limit = maxRules;
  function walk(directory, depth) {
    if (depth > 32) throw new Error(`${directory}: rules directory is nested too deeply`);
    const handle = opendirSync(join(root, directory));
    try {
      for (let entry; (entry = handle.readSync()) !== null;) {
        if (++entries > maxEntries) throw new Error(`${rulesDir}: too many directory entries`);
        const path = `${directory}/${entry.name}`;
        if (entry.name === ".git" || gitlinks.has(path)) continue;
        // Some file systems report no entry type, so fall back to lstat. Anything that is
        // not a regular file or a real directory fails, as --commit does for Git entries,
        // so a rule can't silently drop out of the policy.
        const kind = entry.isFile() || entry.isDirectory() ? entry : statIfPresent(join(root, path));
        if (!kind?.isFile() && !kind?.isDirectory()) {
          throw new Error(`${path}: expected a regular file or directory, not a ${describe(kind)}`);
        }
        if (kind.isDirectory() && !statIfPresent(join(root, path, ".git"))) walk(path, depth + 1);
        if (kind.isFile() && entry.name.endsWith(suffix)) {
          if (found.length >= limit) {
            throw new Error(suffix === ".rule.ts" ? TOO_MANY_RULES : `${directory}: at most ${limit} ${suffix} files are supported`);
          }
          found.push(path);
        }
      }
    } finally {
      handle.closeSync();
    }
  }
  walk(rulesDir, 0);
  const rules = found.sort();
  if (rules.length === 0) throw new Error(`${rulesDir}: add a .rule.ts file`);
  [found, suffix, limit] = [[], ".set.ts", maxSets];
  if (hasSets) walk(setsDir, 0);
  return { rules, sets: found.sort() };
}

function committedRules(root, sha) {
  if (!FULL_SHA.test(sha)) throw new Error(COMMIT_HINT);
  const type = spawnSync("git", ["--no-replace-objects", "-C", root, "cat-file", "-t", sha], { timeout: 15_000 });
  if (type.error || type.status !== 0 || type.stdout.toString() !== "commit\n") {
    throw new Error(`--commit ${sha} is not a commit in ${root}; fetch it or pass the right --root`);
  }
  const tree = utf8.decode(git(root, ["ls-tree", "--full-tree", "-r", "-t", "-z", sha, "--", ".rulecheck"]));
  const rules = new Map();
  const sets = new Map();
  const paths = new Set();
  for (const entry of tree.split("\0").slice(0, -1)) {
    const tab = entry.indexOf("\t");
    const path = entry.slice(tab + 1);
    const inSets = path === setsDir || path.startsWith(`${setsDir}/`);
    if (path !== ".rulecheck" && path !== rulesDir && !path.startsWith(`${rulesDir}/`) && !inSets) continue;
    const [mode, type, oid] = entry.slice(0, tab).split(" ");
    const segments = path.split("/");
    if (paths.has(path) || paths.size >= maxEntries || segments.length > 35
        || Buffer.byteLength(path) > 4096 || /[\p{Cc}\\]/u.test(path)
        || segments.some((part) => ["", ".", "..", ".git"].includes(part))) {
      throw new Error(`${JSON.stringify(path)}: unsupported or excessive Git policy entries`);
    }
    paths.add(path);
    const directory = mode === "040000" && type === "tree";
    if (path === ".rulecheck" || path === rulesDir || path === setsDir) {
      if (!directory) throw new Error(`${path}: expected a Git tree`);
    } else if (!directory) {
      if (!["100644", "100755"].includes(mode) || type !== "blob") {
        throw new Error(`${path}: expected a regular Git blob`);
      }
      if (!inSets && path.endsWith(".rule.ts")) rules.set(path, oid);
      if (inSets && path.endsWith(".set.ts")) sets.set(path, oid);
    }
  }
  if (rules.size === 0) throw new Error(`${rulesDir}: add a .rule.ts file at commit ${sha}`);
  if (rules.size > maxRules) throw new Error(TOO_MANY_RULES);
  if (sets.size > maxSets) throw new Error(`${setsDir}: at most ${maxSets} .set.ts files are supported`);
  return new Map([...rules, ...sets]);
}

// Reads every committed rule and set blob with two Git processes, however many files there
// are: one batch-check for object types and sizes, then one batch for the blobs that
// fit. Spawning Git twice per rule made exact-commit discovery of 1024 rules take
// seconds. Returns read(path), which fails for one rule as reading it alone would.
function committedSources(root, rules) {
  const oids = [...new Set(rules.values())];
  const info = new Map();
  const listing = git(root, ["cat-file", "--batch-check"], 8 * 1024 * 1024, oids.map((oid) => `${oid}\n`).join(""));
  for (const line of listing.toString().split("\n").slice(0, -1)) {
    const [oid, type, size] = line.split(" ");
    info.set(oid, { type, size: Number(size) });
  }
  const wanted = oids.filter((oid) => info.get(oid)?.type === "blob" && info.get(oid).size <= maxSource);
  const contents = new Map();
  if (wanted.length > 0) {
    const total = wanted.reduce((sum, oid) => sum + info.get(oid).size + 128, 0);
    const batch = git(root, ["cat-file", "--batch"], total, wanted.map((oid) => `${oid}\n`).join(""));
    let offset = 0;
    for (const oid of wanted) {
      const end = batch.indexOf(10, offset);
      const header = batch.subarray(offset, end).toString();
      const { size } = info.get(oid);
      if (header !== `${oid} blob ${size}`) throw new Error("could not inspect Git checkout: unexpected cat-file output");
      contents.set(oid, batch.subarray(end + 1, end + 1 + size));
      offset = end + 2 + size;
    }
  }
  return (path) => {
    const oid = rules.get(path);
    if (info.get(oid)?.type !== "blob") throw new Error("expected a Git blob object");
    if (!contents.has(oid)) throw new Error("keep the rule file under 128 KiB");
    return utf8.decode(contents.get(oid));
  };
}

// .rulecheck/sets/web/vendor.set.ts names the set "web/vendor".
const setName = (path) => path.slice(setsDir.length + 1, -".set.ts".length);

const isObject = (value) => value !== null && typeof value === "object" && !Array.isArray(value);
const isStrings = (value) => Array.isArray(value) && value.every((item) => typeof item === "string");
const isCount = (value) => Number.isInteger(value) && value >= 1;
// Mirrors the provenance pattern and maxLength in schema/policy-v1.json and src/policy.rs.
const isFindingUrl = (value) => typeof value === "string" && [...value].length <= 2048
  && /^https:\/\/[^\p{White_Space}\p{Cc}]+$/u.test(value);
// Mirrors the tags and tag limits in schema/policy-v1.json and MAX_TAGS/MAX_TAG in src/policy.rs.
const maxTags = 16;
const isTag = (value) => typeof value === "string" && value.length <= 64 && /^[a-z0-9-]+(\/[a-z0-9-]+)*$/.test(value);
const list = (names) => names.length === 1 ? names[0] : `${names.slice(0, -1).join(", ")} and ${names.at(-1)}`;

// Checks the property names and value types of schema/policy-v1.json, so a typo is
// reported in the rule file. The evaluator still validates limits, IDs, globs, and
// regex syntax. fail(container, key, reason, rewrite) throws at the source position of
// container[key], or of the whole rule when container is undefined.
function checkShape(rule, fail, setNames) {
  function fields(object, label, known, required) {
    for (const key of Object.keys(object)) {
      if (!known.includes(key)) fail(object, key, `Unknown property ${key} in ${label}`, `Use ${list(known)}.`);
    }
    for (const key of required) {
      if (!Object.hasOwn(object, key)) fail(object, undefined, `Missing property ${key} in ${label}`, `Add ${key}.`);
    }
  }
  function expect(object, key, label, valid, want) {
    if (Object.hasOwn(object, key) && !valid(object[key])) {
      fail(object, key, `${label} ${key} must be ${want}`, `Change ${key} to ${want}.`);
    }
  }

  if (!isObject(rule)) fail(undefined, undefined, "The rule must be an object", "Pass an object literal to defineRule.");
  fields(rule, "the rule", ["id", "kind", "paths", "exclude", "excludeSets", "skipIfContent", "anchors", "remediation", "message", "pattern",
    "requires", "ignoreInside", "selector", "structure", "consistency", "provenance", "tags", "fixtures"], ["id", "kind", "paths", "remediation", "fixtures"]);
  expect(rule, "id", "rule", (value) => typeof value === "string", "a string");
  expect(rule, "kind", "rule", (value) => ["text", "forbidden-path", "syntax", "structured", "consistency"].includes(value),
    '"text", "forbidden-path", "syntax", "structured" or "consistency"');
  expect(rule, "paths", "rule", isStrings, "an array of glob strings");
  expect(rule, "exclude", "rule", isStrings, "an array of glob strings");
  expect(rule, "anchors", "rule", isStrings, "an array of glob strings");
  if (rule.kind !== "forbidden-path" && Object.hasOwn(rule, "anchors")) {
    fail(rule, "anchors", "Only forbidden-path rules have anchors",
      'Remove anchors: text and syntax rules already exit 2 when no file is in scope.');
  }
  expect(rule, "excludeSets", "rule", isStrings, "an array of set names");
  for (const [index, name] of (rule.excludeSets ?? []).entries()) {
    if (!setNames.has(name)) {
      fail(rule.excludeSets, index, `Unknown set ${JSON.stringify(name)} in excludeSets`,
        `Add ${setsDir}/${name}.set.ts or use one of: ${[...setNames].join(", ") || "no sets defined"}.`);
    }
  }
  if (Object.hasOwn(rule, "skipIfContent")) {
    if (rule.kind === "forbidden-path") {
      fail(rule, "skipIfContent", "Forbidden-path rules have no skipIfContent, because they never read content",
        "Remove skipIfContent.");
    }
    if (rule.kind === "consistency") {
      fail(rule, "skipIfContent", "Consistency rules have no skipIfContent", "Narrow the scopes with exclude.");
    }
    expect(rule, "skipIfContent", "rule", isObject, "an object { kind, value, firstBytes }");
    fields(rule.skipIfContent, "the skipIfContent", ["kind", "value", "firstBytes"], ["kind", "value", "firstBytes"]);
    expect(rule.skipIfContent, "kind", "skipIfContent", (value) => ["literal", "regex"].includes(value), '"literal" or "regex"');
    expect(rule.skipIfContent, "value", "skipIfContent", (value) => typeof value === "string", "a string");
    expect(rule.skipIfContent, "firstBytes", "skipIfContent",
      (value) => Number.isInteger(value) && value >= 1 && value <= 65536, "an integer from 1 to 65536");
  }
  expect(rule, "remediation", "rule", (value) => typeof value === "string", "a string");
  expect(rule, "message", "rule", (value) => typeof value === "string", "a string");
  expect(rule, "fixtures", "rule", (value) => Array.isArray(value), "an array of fixtures");
  if (Object.hasOwn(rule, "provenance")) {
    expect(rule, "provenance", "rule", isObject, "an object { finding }");
    fields(rule.provenance, "the provenance", ["finding"], ["finding"]);
    expect(rule.provenance, "finding", "provenance", isFindingUrl, "an https:// URL of at most 2048 characters");
  }
  if (Object.hasOwn(rule, "tags")) {
    expect(rule, "tags", "rule", (value) => Array.isArray(value) && value.length >= 1 && value.length <= maxTags,
      `an array of 1 to ${maxTags} tags`);
    const seen = new Set();
    for (const [index, tag] of rule.tags.entries()) {
      if (!isTag(tag)) {
        fail(rule.tags, index, `tag ${index + 1} must be at most 64 lowercase letters, digits, hyphens and slashes`,
          'Write a tag shaped like a rule ID, such as "adr/0007" or "domain/billing".');
      }
      if (seen.has(tag)) fail(rule.tags, index, `Duplicate tag ${JSON.stringify(tag)} in tags`, "Remove the repeated tag.");
      seen.add(tag);
    }
  }
  const syntax = rule.kind === "syntax";
  const structured = rule.kind === "structured";
  const text = rule.kind === "text" || syntax || structured;
  const consistency = rule.kind === "consistency";
  if (consistency !== Object.hasOwn(rule, "consistency")) {
    if (consistency) {
      fail(rule, undefined, "Consistency rules require a consistency block",
        'Add consistency: { form: "same-values", left: {...}, right: {...} }.');
    }
    fail(rule, "consistency", "Only consistency rules have a consistency block", 'Remove it or use kind: "consistency".');
  }
  if (consistency) {
    expect(rule, "consistency", "rule", isObject, "an object { form, ... }");
    const block = rule.consistency;
    expect(block, "form", "consistency", (value) => ["same-values", "path-exists"].includes(value),
      '"same-values" or "path-exists"');
    const pathExists = block.form === "path-exists";
    const scopes = pathExists ? ["source"] : ["left", "right"];
    fields(block, "the consistency block", ["form", ...scopes, ...(pathExists ? ["base"] : [])],
      ["form", ...scopes, ...(pathExists ? ["base"] : [])]);
    if (pathExists) expect(block, "base", "consistency", (value) => ["root", "file"].includes(value), '"root" or "file"');
    for (const key of scopes) {
      expect(block, key, "consistency", isObject, "an object { name, paths, extract }");
      const scope = block[key];
      const label = `consistency ${key}`;
      fields(scope, `the ${label} scope`, ["name", "paths", "exclude", "extract"], ["name", "paths", "extract"]);
      expect(scope, "name", label, (value) => typeof value === "string", "a string");
      expect(scope, "paths", label, isStrings, "an array of glob strings");
      expect(scope, "exclude", label, isStrings, "an array of glob strings");
      expect(scope, "extract", label, (value) => typeof value === "string", "a regex string");
    }
  }
  if (rule.kind === "text" && !Object.hasOwn(rule, "pattern")) {
    fail(rule, undefined, "Text rules require a pattern", 'Add pattern: { kind: "literal", value: "..." }.');
  }
  if (rule.kind !== "text" && Object.hasOwn(rule, "pattern")) {
    fail(rule, "pattern", `${syntax ? "Syntax" : structured ? "Structured" : consistency ? "Consistency" : "Forbidden-path"} rules have no pattern`,
      syntax ? "Use selector instead of pattern." : structured ? "Use structure instead of pattern." : "Remove pattern.");
  }
  if (structured && !Object.hasOwn(rule, "structure")) {
    fail(rule, undefined, "Structured rules require a structure",
      'Add structure: { format: "yaml", path: [{ key: "..." }], assert: { kind: "present" } }.');
  }
  if (!structured && Object.hasOwn(rule, "structure")) {
    fail(rule, "structure", "Only structured rules have a structure", 'Remove structure or use kind: "structured".');
  }
  if (structured) {
    expect(rule, "structure", "rule", isObject, "a structure object");
    const structure = rule.structure;
    fields(structure, "the structure", ["format", "path", "assert"], ["format", "path", "assert"]);
    expect(structure, "format", "structure", (value) => value === "yaml", '"yaml"');
    expect(structure, "path", "structure", (value) => Array.isArray(value), "an array of path segments");
    for (const [index, segment] of structure.path.entries()) {
      const label = `structure path ${index + 1}`;
      if (!isObject(segment) || Object.keys(segment).length !== 1) {
        fail(structure.path, index, `${label} must be an object with exactly one of key, index or each`,
          'Write { key: "jobs" }, { index: 0 } or { each: "mapping" }.');
      }
      fields(segment, label, ["key", "index", "each"], []);
      expect(segment, "key", label, (value) => typeof value === "string", "a string");
      expect(segment, "index", label, (value) => Number.isInteger(value) && value >= 0, "a zero-based integer");
      expect(segment, "each", label, (value) => ["mapping", "sequence"].includes(value), '"mapping" or "sequence"');
    }
    expect(structure, "assert", "structure", isObject, "an assertion object");
    const assert = structure.assert;
    const kinds = { present: [], absent: [], equals: ["value"], "not-equals": ["value"], "has-key": ["key"] };
    expect(assert, "kind", "structure assert", (value) => Object.hasOwn(kinds, value),
      '"present", "absent", "equals", "not-equals" or "has-key"');
    fields(assert, "the structure assert", ["kind", ...(kinds[assert.kind] ?? [])], ["kind", ...(kinds[assert.kind] ?? [])]);
    expect(assert, "value", "structure assert", (value) => typeof value === "string", "a string");
    expect(assert, "key", "structure assert", (value) => typeof value === "string", "a string");
  }
  if (rule.kind !== "text" && Object.hasOwn(rule, "requires")) {
    fail(rule, "requires", `${syntax ? "Syntax" : structured ? "Structured" : consistency ? "Consistency" : "Forbidden-path"} rules have no requires`,
      'Remove requires or use kind: "text".');
  }
  if (rule.kind !== "text" && Object.hasOwn(rule, "ignoreInside")) {
    fail(rule, "ignoreInside", `${syntax ? "Syntax" : structured ? "Structured" : consistency ? "Consistency" : "Forbidden-path"} rules have no ignoreInside`,
      syntax ? "Remove ignoreInside: a selector picks node kinds itself." : 'Remove ignoreInside or use kind: "text".');
  }
  if (syntax && !Object.hasOwn(rule, "selector")) {
    fail(rule, undefined, "Syntax rules require a selector", 'Add selector: { language: "typescript", node: "comment" }.');
  }
  if (!syntax && Object.hasOwn(rule, "selector")) {
    fail(rule, "selector", "Only syntax rules have a selector", 'Remove selector or use kind: "syntax".');
  }
  if (syntax) {
    expect(rule, "selector", "rule", isObject, "a selector object");
    const selector = rule.selector;
    fields(selector, "the selector", ["language", "node", "inside", "ignore"], ["language", "node"]);
    expect(selector, "language", "selector", (value) => ["typescript", "rust", "go", "python"].includes(value), '"typescript", "rust", "go" or "python"');
    expect(selector, "node", "selector", (value) => typeof value === "string", "a node kind string");
    if (Object.hasOwn(selector, "inside")) {
      expect(selector, "inside", "selector", isObject, "an object { kinds, field }");
      fields(selector.inside, "the selector inside", ["kinds", "field"], ["kinds"]);
      expect(selector.inside, "kinds", "selector inside", isStrings, "an array of node kind strings");
      expect(selector.inside, "field", "selector inside", (value) => typeof value === "string", "a field name string");
    }
    if (Object.hasOwn(selector, "ignore")) {
      expect(selector, "ignore", "selector", (value) => Array.isArray(value), "an array of text filters");
      for (const [index, filter] of selector.ignore.entries()) {
        const label = `selector ignore ${index + 1}`;
        if (!isObject(filter)) fail(selector.ignore, index, `${label} must be an object`, 'Write { kind: "regex", value: "..." }.');
        fields(filter, label, ["kind", "value"], ["kind", "value"]);
        expect(filter, "kind", label, (value) => ["literal", "regex"].includes(value), '"literal" or "regex"');
        expect(filter, "value", label, (value) => typeof value === "string", "a string");
      }
    }
  }
  // Mirrors Rule::compile in src/policy.rs, which also checks each {name} reference.
  if (Object.hasOwn(rule, "message") && (rule.kind !== "text" || rule.pattern?.kind !== "regex")) {
    fail(rule, "message", "Only regex text rules have a message",
      "Use a regex pattern with named groups, or remove message.");
  }
  if (rule.kind === "text") {
    expect(rule, "pattern", "rule", isObject, "a pattern object");
    fields(rule.pattern, "the pattern", ["kind", "value"], ["kind", "value"]);
    expect(rule.pattern, "kind", "pattern", (value) => ["literal", "regex"].includes(value), '"literal" or "regex"');
    expect(rule.pattern, "value", "pattern", (value) => typeof value === "string", "a string");
    if (Object.hasOwn(rule, "requires")) {
      expect(rule, "requires", "rule", isObject, "a pattern object");
      fields(rule.requires, "the requires pattern", ["kind", "value"], ["kind", "value"]);
      expect(rule.requires, "kind", "requires", (value) => ["literal", "regex"].includes(value), '"literal" or "regex"');
      expect(rule.requires, "value", "requires", (value) => typeof value === "string", "a string");
    }
    if (Object.hasOwn(rule, "ignoreInside")) {
      expect(rule, "ignoreInside", "rule", isObject, "an object { language, contexts }");
      fields(rule.ignoreInside, "the ignoreInside", ["language", "contexts"], ["language", "contexts"]);
      expect(rule.ignoreInside, "language", "ignoreInside", (value) => ["typescript", "bash"].includes(value), '"typescript" or "bash"');
      expect(rule.ignoreInside, "contexts", "ignoreInside", (value) => Array.isArray(value) && value.length >= 1
        && value.length <= 2 && new Set(value).size === value.length && value.every((context) => ["comment", "string"].includes(context)),
        'a list of distinct contexts: "comment", "string"');
    }
  }

  for (const [index, fixture] of rule.fixtures.entries()) {
    const label = `fixture ${index + 1}`;
    if (!isObject(fixture)) fail(rule.fixtures, index, `${label} must be an object`, "Write { path, content, expect }.");
    if (consistency) {
      fields(fixture, label, ["files", "expectFindings"], ["files", "expectFindings"]);
      expect(fixture, "files", label, (value) => Array.isArray(value), "an array of { path, content }");
      for (const [position, file] of fixture.files.entries()) {
        const where = `${label} file ${position + 1}`;
        if (!isObject(file)) fail(fixture.files, position, `${where} must be an object`, "Write { path, content }.");
        fields(file, where, ["path", "content"], ["path"]);
        expect(file, "path", where, (value) => typeof value === "string", "a string");
        expect(file, "content", where, (value) => typeof value === "string", "a string");
      }
      expect(fixture, "expectFindings", label, (value) => Array.isArray(value), "an array of { path, line, column }");
      for (const [position, value] of fixture.expectFindings.entries()) {
        const where = `${label} expectFindings ${position + 1}`;
        if (!isObject(value)) fail(fixture.expectFindings, position, `${where} must be an object`, "Write { path, line, column }.");
        fields(value, where, ["path", "line", "column"], ["path", "line", "column"]);
        expect(value, "path", where, (item) => typeof item === "string", "a string");
        expect(value, "line", where, isCount, "a one-based integer");
        expect(value, "column", where, isCount, "a one-based integer");
      }
      continue;
    }
    const all = Object.hasOwn(fixture, "expectAll");
    fields(fixture, label, ["path", "content", "expect", "expectAll"],
      [...(text ? ["path", "content"] : ["path"]), ...(all ? [] : ["expect"])]);
    expect(fixture, "path", label, (value) => typeof value === "string", "a string");
    expect(fixture, "content", label, (value) => typeof value === "string", "a string");
    if (!text && Object.hasOwn(fixture, "content")) {
      fail(fixture, "content", `${label} has content, but forbidden-path fixtures match the path only`, "Remove content.");
    }
    const location = (value, where) => {
      fields(value, where, ["line", "column"], ["line", "column"]);
      expect(value, "line", where, isCount, "a one-based integer");
      expect(value, "column", where, isCount, "a one-based integer");
    };
    if (all) {
      // Mirrors Matcher::all_matches_unsupported in src/policy.rs.
      if (rule.kind !== "text" && !structured) {
        fail(fixture, "expectAll", syntax
          ? `${label} uses expectAll, but syntax rules do not count matching nodes yet`
          : `${label} uses expectAll, but forbidden-path rules report at most one finding per path`, "Use expect.");
      }
      if (Object.hasOwn(fixture, "expect")) {
        fail(fixture, "expectAll", `${label} has both expect and expectAll`, "Keep one of them.");
      }
      expect(fixture, "expectAll", label, (value) => Array.isArray(value), "an array of { line, column }");
      for (const [position, value] of fixture.expectAll.entries()) {
        const where = `${label} expectAll ${position + 1}`;
        if (!isObject(value)) fail(fixture.expectAll, position, `${where} must be an object`, "Write { line, column }.");
        location(value, where);
      }
    } else {
      expect(fixture, "expect", label, (value) => value === null || isObject(value),
        "null or { line, column }");
      if (fixture.expect !== null) location(fixture.expect, `${label} expect`);
    }
  }
}

// Checks a set file's defineGlobSet({...}) argument, as checkShape does for rules.
function checkSetShape(set, fail) {
  if (!isObject(set)) fail(undefined, undefined, "The set must be an object", "Pass an object literal to defineGlobSet.");
  for (const key of Object.keys(set)) {
    if (!["version", "globs", "include"].includes(key)) {
      fail(set, key, `Unknown property ${key} in the set`, "Use version, globs and include.");
    }
  }
  for (const key of ["version", "globs"]) {
    if (!Object.hasOwn(set, key)) fail(set, undefined, `Missing property ${key} in the set`, `Add ${key}.`);
  }
  if (set.version !== 1) fail(set, "version", "set version must be 1", "Change version to 1.");
  if (!isStrings(set.globs)) fail(set, "globs", "set globs must be an array of glob strings", "Change globs to an array of glob strings.");
  if (Object.hasOwn(set, "include") && !isStrings(set.include)) {
    fail(set, "include", "set include must be an array of set names", "Change include to an array of set names.");
  }
}

function parse(path, source, helper = "defineRule", setNames = new Set()) {
  const file = ts.createSourceFile(path, source, ts.ScriptTarget.ESNext, false, ts.ScriptKind.TS);
  function reject(position, reason, rewrite) {
    const { line, character } = file.getLineAndCharacterOfPosition(position);
    throw new Error(`${path}:${line + 1}:${character + 1}: ${reason}. ${rewrite}`);
  }

  const [diagnostic] = file.parseDiagnostics;
  if (diagnostic) {
    reject(diagnostic.start ?? 0, ts.flattenDiagnosticMessageText(diagnostic.messageText, " "),
      "Use plain literals and export default defineRule({...}).");
  }

  // Source positions of each object and array literal and of their properties or
  // elements, so shape errors point at the rule file instead of the policy JSON.
  const positions = new WeakMap();
  function at(container, key) {
    const entry = positions.get(container);
    const node = (key !== undefined && entry?.keys.get(String(key))) || entry?.node;
    return node ? node.getStart(file) : 0;
  }

  const bindings = new Map();
  function literal(node, depth = 0) {
    const position = node.getStart(file);
    if (depth > 32) reject(position, "Literal nesting exceeds 32 levels", "Flatten the object or array.");
    const child = (value) => literal(value, depth + 1);

    if (ts.isStringLiteral(node) || ts.isNoSubstitutionTemplateLiteral(node)) return node.text;
    if (node.kind === ts.SyntaxKind.TrueKeyword) return true;
    if (node.kind === ts.SyntaxKind.FalseKeyword) return false;
    if (node.kind === ts.SyntaxKind.NullKeyword) return null;
    if (ts.isNumericLiteral(node) && Number.isFinite(Number(node.text))) return Number(node.text);
    if (ts.isIdentifier(node) && bindings.has(node.text)) return bindings.get(node.text);
    if (ts.isArrayLiteralExpression(node)) {
      const array = node.elements.map(child);
      positions.set(array, { node, keys: new Map(node.elements.map((element, index) => [String(index), element])) });
      return array;
    }
    if (ts.isObjectLiteralExpression(node)) {
      const object = Object.create(null);
      const keys = new Map();
      positions.set(object, { node, keys });
      for (const property of node.properties) {
        const key = property.name;
        if (!ts.isPropertyAssignment(property) ||
            !(ts.isIdentifier(key) || ts.isStringLiteral(key) || ts.isNumericLiteral(key))) {
          reject(property.getStart(file), "Unsupported object property", "Write a plain key: value property.");
        }
        if (Object.hasOwn(object, key.text)) {
          reject(key.getStart(file), `Duplicate property ${key.text}`, "Keep one value for the property.");
        }
        keys.set(key.text, key);
        object[key.text] = child(property.initializer);
      }
      return object;
    }
    reject(position, "Unsupported value", "Replace it with a plain literal or reference an earlier literal const.");
  }

  const [first, ...statements] = file.statements;
  const clause = first && ts.isImportDeclaration(first) && first.importClause;
  const named = clause && clause.namedBindings;
  const imported = named && ts.isNamedImports(named) && named.elements;
  if (!imported || imported.length !== 1 || imported[0].name.text !== helper
      || imported[0].propertyName || imported[0].isTypeOnly || clause.isTypeOnly || clause.name
      || first.attributes || !ts.isStringLiteral(first.moduleSpecifier) || !sdkMarkers.has(first.moduleSpecifier.text)) {
    reject(first?.getStart(file) ?? 0, "Expected the SDK marker import", `Start with import { ${helper} } from "${marker}".`);
  }

  for (const [index, statement] of statements.entries()) {
    if (ts.isVariableStatement(statement) && !statement.modifiers?.length
        && statement.declarationList.flags & ts.NodeFlags.Const) {
      for (const decl of statement.declarationList.declarations) {
        if (!ts.isIdentifier(decl.name) || !decl.initializer || decl.type
            || decl.name.text === helper || bindings.has(decl.name.text)) {
          reject(decl.getStart(file), "Unsupported const declaration", "Use a unique name and const name = literal.");
        }
        bindings.set(decl.name.text, literal(decl.initializer));
      }
      continue;
    }

    if (index === statements.length - 1 && ts.isExportAssignment(statement) && !statement.isExportEquals) {
      const call = statement.expression;
      if (ts.isCallExpression(call) && ts.isIdentifier(call.expression) && call.expression.text === helper
          && call.arguments.length === 1 && !call.typeArguments && !call.questionDotToken) {
        const value = literal(call.arguments[0]);
        const fail = (container, key, reason, rewrite) => reject(
          container === undefined ? call.arguments[0].getStart(file) : at(container, key), reason, rewrite);
        if (helper === "defineRule") checkShape(value, fail, setNames);
        else checkSetShape(value, fail);
        return value;
      }
    }

    reject(statement.getStart(file), "Unsupported statement", `Use literal const declarations followed by one export default ${helper}({...}).`);
  }

  reject(file.end, helper === "defineRule" ? "Missing rule" : "Missing set", `Add export default ${helper}({...}) at the end of the file.`);
}

function encodeRule(path, read, setNames) {
  return encode(path, read, (source) => parse(path, source, "defineRule", setNames), "rule");
}

function encodeSet(path, read) {
  const name = setName(path);
  if (!SET_NAME.test(name) || name.length > 128) {
    throw new Error(`${path}:1:1: Invalid set name ${JSON.stringify(name)}. Name set files with lowercase letters, digits and hyphens, such as generated.set.ts.`);
  }
  return encode(path, read, (source) => ({ name, ...parse(path, source, "defineGlobSet") }), "set");
}

function encode(path, read, parseFile, noun) {
  let rule;
  try {
    rule = parseFile(read(path));
  } catch (error) {
    if (error.message.startsWith(`${path}:`)) throw error;
    throw new Error(`${path}:1:1: ${error.message}. Use a regular UTF-8 file with a small literal ${noun}.`);
  }

  let budget = maxPolicy;
  try {
    return JSON.stringify(rule, (key, value) => {
      budget -= 1 + Buffer.byteLength(key) + (typeof value === "string" ? Buffer.byteLength(value) : 0);
      if (budget < 0) throw new Error("size limit");
      return value;
    });
  } catch {
    throw new Error(`${path}:1:1: Expanded ${noun} is too large or too deeply nested. Reduce repeated or nested literals.`);
  }
}

function build(args) {
  if (args.length === 1 && args[0] === "--version") {
    const { version } = JSON.parse(readFileSync(new URL("../package.json", import.meta.url), "utf8"));
    console.log(`rulecheck-builder ${version} (TypeScript ${ts.version})`);
    return;
  }
  const options = new Map();
  for (let index = 0; index < args.length; index += 2) {
    const [flag, value] = args.slice(index, index + 2);
    if (!["--root", "--commit"].includes(flag) || !value || options.has(flag)) {
      throw new Error("Usage: npm run --silent build -- [--root DIR] [--commit SHA]");
    }
    options.set(flag, value);
  }
  const root = resolve(options.get("--root") ?? ".");
  const sha = options.get("--commit");
  // Rules and sets always come from the same place: one worktree, or one commit's tree.
  const committed = sha && committedRules(root, sha);
  const all = committed ? [...committed.keys()].sort() : null;
  const { rules: paths, sets: setPaths } = committed
    ? { rules: all.filter((path) => !path.startsWith(`${setsDir}/`)), sets: all.filter((path) => path.startsWith(`${setsDir}/`)) }
    : discover(root);
  const read = committed ? committedSources(root, committed) : (path) => readRule(join(root, path));
  // Without set files the policy is byte-for-byte what earlier builders emitted.
  const sets = setPaths.map((path) => encodeSet(path, read));
  const setNames = new Set(setPaths.map(setName));
  const header = `{"version":1,${sets.length ? `"sets":[${sets.join(",")}],` : ""}"rules":[`;
  const footer = "]}\n";
  let size = Buffer.byteLength(header + footer);
  if (size > maxPolicy) throw new Error(`${setsDir}:1:1: Policy exceeds 4 MiB. Reduce set sizes.`);
  const rules = [];
  for (const path of paths) {
    const json = encodeRule(path, read, setNames);
    size += Buffer.byteLength(json) + Number(rules.length > 0);
    if (size > maxPolicy) throw new Error(`${path}:1:1: Policy exceeds 4 MiB. Reduce rule and fixture sizes.`);
    rules.push(json);
  }
  process.stdout.write(header + rules.join(",") + footer);
}

try {
  build(process.argv.slice(2));
} catch (error) {
  console.error(`rulecheck build: ${error.message}`);
  process.exitCode = 2;
}
