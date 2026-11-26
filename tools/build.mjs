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

function discover(root) {
  if (!lstatSync(root).isDirectory() || git(root, ["rev-parse", "--show-prefix"]).toString().trim()) {
    throw new Error("use the root directory of a Git checkout");
  }

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
        if (entry.isDirectory() && !statIfPresent(join(root, path, ".git"))) walk(path, depth + 1);
        if (entry.isFile() && entry.name.endsWith(".rule.ts")) {
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
  if (!/^(?:[a-f0-9]{40}|[a-f0-9]{64})$/i.test(sha)
      || git(root, ["cat-file", "-t", sha], 32).toString() !== "commit\n") {
    throw new Error("--commit requires a full Git commit SHA");
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
    if (ts.isArrayLiteralExpression(node)) return node.elements.map(child);
    if (ts.isObjectLiteralExpression(node)) {
      const object = Object.create(null);
      for (const property of node.properties) {
        const key = property.name;
        if (!ts.isPropertyAssignment(property) ||
            !(ts.isIdentifier(key) || ts.isStringLiteral(key) || ts.isNumericLiteral(key))) {
          reject(property.getStart(file), "Unsupported object property", "Write a plain key: value property.");
        }
        if (Object.hasOwn(object, key.text)) {
          reject(key.getStart(file), `Duplicate property ${key.text}`, "Keep one value for the property.");
        }
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
        return literal(call.arguments[0]);
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
