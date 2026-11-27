import { spawnSync } from "node:child_process";
import { constants, closeSync, fstatSync, lstatSync, opendirSync, openSync, readFileSync, readSync } from "node:fs";
import { join, resolve } from "node:path";
import ts from "typescript";

const maxSource = 128 * 1024;
const maxPolicy = 1024 * 1024;
const maxRules = 128;
const maxEntries = 20_000;
const marker = "@rulecheck/sdk";
const rulesDir = ".rulecheck/rules";
const utf8 = new TextDecoder("utf-8", { fatal: true });
const FULL_SHA = /^(?:[a-f0-9]{40}|[a-f0-9]{64})$/i;
const COMMIT_HINT = '--commit requires a full 40- or 64-character Git commit SHA; resolve it first, e.g. --commit "$(git rev-parse HEAD)"';
const ROOT_HINT = "run from the checkout root or pass --root";

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

function git(root, args, maxBuffer = 8 * 1024 * 1024) {
  const result = spawnSync("git", ["--no-replace-objects", "-C", root, ...args], {
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

  for (const path of [".rulecheck", rulesDir]) {
    if (gitlinks.has(path) || !statIfPresent(join(root, path))?.isDirectory()
        || statIfPresent(join(root, path, ".git"))) {
      throw new Error(`${path}: use a regular directory inside the checkout`);
    }
  }

  let entries = 0;
  const rules = [];
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
        if (kind.isFile() && entry.name.endsWith(".rule.ts")) {
          if (rules.length >= maxRules) throw new Error(`${rulesDir}: at most ${maxRules} rules are supported`);
          rules.push(path);
        }
      }
    } finally {
      handle.closeSync();
    }
  }
  walk(rulesDir, 0);
  if (rules.length === 0) throw new Error(`${rulesDir}: add a .rule.ts file`);
  return rules.sort();
}

function committedRules(root, sha) {
  if (!FULL_SHA.test(sha)) throw new Error(COMMIT_HINT);
  const type = spawnSync("git", ["--no-replace-objects", "-C", root, "cat-file", "-t", sha], { timeout: 15_000 });
  if (type.error || type.status !== 0 || type.stdout.toString() !== "commit\n") {
    throw new Error(`--commit ${sha} is not a commit in ${root}; fetch it or pass the right --root`);
  }
  const tree = utf8.decode(git(root, ["ls-tree", "--full-tree", "-r", "-t", "-z", sha, "--", ".rulecheck"]));
  const rules = new Map();
  const paths = new Set();
  for (const entry of tree.split("\0").slice(0, -1)) {
    const tab = entry.indexOf("\t");
    const path = entry.slice(tab + 1);
    if (path !== ".rulecheck" && path !== rulesDir && !path.startsWith(`${rulesDir}/`)) continue;
    const [mode, type, oid] = entry.slice(0, tab).split(" ");
    const segments = path.split("/");
    if (paths.has(path) || paths.size >= maxEntries || segments.length > 35
        || Buffer.byteLength(path) > 4096 || /[\p{Cc}\\]/u.test(path)
        || segments.some((part) => ["", ".", "..", ".git"].includes(part))) {
      throw new Error(`${JSON.stringify(path)}: unsupported or excessive Git policy entries`);
    }
    paths.add(path);
    const directory = mode === "040000" && type === "tree";
    if (path === ".rulecheck" || path === rulesDir) {
      if (!directory) throw new Error(`${path}: expected a Git tree`);
    } else if (!directory) {
      if (!["100644", "100755"].includes(mode) || type !== "blob") {
        throw new Error(`${path}: expected a regular Git blob`);
      }
      if (path.endsWith(".rule.ts")) rules.set(path, oid);
    }
  }
  if (rules.size === 0 || rules.size > maxRules) {
    throw new Error(`${rulesDir}: expected 1 to ${maxRules} regular .rule.ts files`);
  }
  return rules;
}

const isObject = (value) => value !== null && typeof value === "object" && !Array.isArray(value);
const isStrings = (value) => Array.isArray(value) && value.every((item) => typeof item === "string");
const isCount = (value) => Number.isInteger(value) && value >= 1;
// Mirrors the provenance pattern and maxLength in schema/policy-v1.json and src/policy.rs.
const isFindingUrl = (value) => typeof value === "string" && [...value].length <= 2048
  && /^https:\/\/[^\p{White_Space}\p{Cc}]+$/u.test(value);
const list = (names) => names.length === 1 ? names[0] : `${names.slice(0, -1).join(", ")} and ${names.at(-1)}`;

// Checks the property names and value types of schema/policy-v1.json, so a typo is
// reported in the rule file. The evaluator still validates limits, IDs, globs, and
// regex syntax. fail(container, key, reason, rewrite) throws at the source position of
// container[key], or of the whole rule when container is undefined.
function checkShape(rule, fail) {
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
  fields(rule, "the rule", ["id", "kind", "paths", "exclude", "remediation", "pattern", "selector", "provenance", "fixtures"],
    ["id", "kind", "paths", "remediation", "fixtures"]);
  expect(rule, "id", "rule", (value) => typeof value === "string", "a string");
  expect(rule, "kind", "rule", (value) => ["text", "forbidden-path", "syntax"].includes(value),
    '"text", "forbidden-path" or "syntax"');
  expect(rule, "paths", "rule", isStrings, "an array of glob strings");
  expect(rule, "exclude", "rule", isStrings, "an array of glob strings");
  expect(rule, "remediation", "rule", (value) => typeof value === "string", "a string");
  expect(rule, "fixtures", "rule", (value) => Array.isArray(value), "an array of fixtures");
  if (Object.hasOwn(rule, "provenance")) {
    expect(rule, "provenance", "rule", isObject, "an object { finding }");
    fields(rule.provenance, "the provenance", ["finding"], ["finding"]);
    expect(rule.provenance, "finding", "provenance", isFindingUrl, "an https:// URL of at most 2048 characters");
  }
  const syntax = rule.kind === "syntax";
  const text = rule.kind === "text" || syntax;
  if (rule.kind === "text" && !Object.hasOwn(rule, "pattern")) {
    fail(rule, undefined, "Text rules require a pattern", 'Add pattern: { kind: "literal", value: "..." }.');
  }
  if (rule.kind !== "text" && Object.hasOwn(rule, "pattern")) {
    fail(rule, "pattern", `${syntax ? "Syntax" : "Forbidden-path"} rules have no pattern`,
      syntax ? "Use selector instead of pattern." : "Remove pattern.");
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
    expect(selector, "language", "selector", (value) => value === "typescript", '"typescript"');
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
  if (rule.kind === "text") {
    expect(rule, "pattern", "rule", isObject, "a pattern object");
    fields(rule.pattern, "the pattern", ["kind", "value"], ["kind", "value"]);
    expect(rule.pattern, "kind", "pattern", (value) => ["literal", "regex"].includes(value), '"literal" or "regex"');
    expect(rule.pattern, "value", "pattern", (value) => typeof value === "string", "a string");
  }

  for (const [index, fixture] of rule.fixtures.entries()) {
    const label = `fixture ${index + 1}`;
    if (!isObject(fixture)) fail(rule.fixtures, index, `${label} must be an object`, "Write { path, content, expect }.");
    fields(fixture, label, ["path", "content", "expect"], text ? ["path", "content", "expect"] : ["path", "expect"]);
    expect(fixture, "path", label, (value) => typeof value === "string", "a string");
    expect(fixture, "content", label, (value) => typeof value === "string", "a string");
    if (!text && Object.hasOwn(fixture, "content")) {
      fail(fixture, "content", `${label} has content, but forbidden-path fixtures match the path only`, "Remove content.");
    }
    expect(fixture, "expect", label, (value) => value === null || isObject(value),
      "null or { line, column }");
    if (fixture.expect !== null) {
      fields(fixture.expect, `${label} expect`, ["line", "column"], ["line", "column"]);
      expect(fixture.expect, "line", `${label} expect`, isCount, "a one-based integer");
      expect(fixture.expect, "column", `${label} expect`, isCount, "a one-based integer");
    }
  }
}

function parse(path, source) {
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
  if (!imported || imported.length !== 1 || imported[0].name.text !== "defineRule"
      || imported[0].propertyName || imported[0].isTypeOnly || clause.isTypeOnly || clause.name
      || first.attributes || !ts.isStringLiteral(first.moduleSpecifier) || first.moduleSpecifier.text !== marker) {
    reject(first?.getStart(file) ?? 0, "Expected the SDK marker import", `Start with import { defineRule } from "${marker}".`);
  }

  for (const [index, statement] of statements.entries()) {
    if (ts.isVariableStatement(statement) && !statement.modifiers?.length
        && statement.declarationList.flags & ts.NodeFlags.Const) {
      for (const decl of statement.declarationList.declarations) {
        if (!ts.isIdentifier(decl.name) || !decl.initializer || decl.type
            || decl.name.text === "defineRule" || bindings.has(decl.name.text)) {
          reject(decl.getStart(file), "Unsupported const declaration", "Use a unique name and const name = literal.");
        }
        bindings.set(decl.name.text, literal(decl.initializer));
      }
      continue;
    }

    if (index === statements.length - 1 && ts.isExportAssignment(statement) && !statement.isExportEquals) {
      const call = statement.expression;
      if (ts.isCallExpression(call) && ts.isIdentifier(call.expression) && call.expression.text === "defineRule"
          && call.arguments.length === 1 && !call.typeArguments && !call.questionDotToken) {
        const rule = literal(call.arguments[0]);
        checkShape(rule, (container, key, reason, rewrite) => reject(
          container === undefined ? call.arguments[0].getStart(file) : at(container, key), reason, rewrite));
        return rule;
      }
    }

    reject(statement.getStart(file), "Unsupported statement", "Use literal const declarations followed by one export default defineRule({...}).");
  }

  reject(file.end, "Missing rule", "Add export default defineRule({...}) at the end of the file.");
}

function encodeRule(path, read) {
  let rule;
  try {
    rule = parse(path, read(path));
  } catch (error) {
    if (error.message.startsWith(`${path}:`)) throw error;
    throw new Error(`${path}:1:1: ${error.message}. Use a regular UTF-8 file with a small literal rule.`);
  }

  let budget = maxPolicy;
  try {
    return JSON.stringify(rule, (key, value) => {
      budget -= 1 + Buffer.byteLength(key) + (typeof value === "string" ? Buffer.byteLength(value) : 0);
      if (budget < 0) throw new Error("size limit");
      return value;
    });
  } catch {
    throw new Error(`${path}:1:1: Expanded rule is too large or too deeply nested. Reduce repeated or nested literals.`);
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
  const committed = sha && committedRules(root, sha);
  const paths = committed ? [...committed.keys()].sort() : discover(root);
  const read = (path) => {
    if (!committed) return readRule(join(root, path));
    const oid = committed.get(path);
    if (git(root, ["cat-file", "-t", oid], 32).toString() !== "blob\n") {
      throw new Error("expected a Git blob object");
    }
    return utf8.decode(git(root, ["cat-file", "blob", oid], maxSource));
  };
  const header = '{"version":1,"rules":[';
  const footer = "]}\n";
  let size = Buffer.byteLength(header + footer);
  const rules = [];
  for (const path of paths) {
    const json = encodeRule(path, read);
    size += Buffer.byteLength(json) + Number(rules.length > 0);
    if (size > maxPolicy) throw new Error(`${path}:1:1: Policy exceeds 1 MiB. Reduce rule and fixture sizes.`);
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
