// Coverage report over a rule inventory: which required tags, such as architecture
// decision records ("adr/0007") or domains ("domain/billing"), have at least one rule.
// It reads only the `rulecheck list --format json` document, never rule sources.
//
//   rulecheck list --format json | node tools/coverage.mjs --required coverage.txt [--require TAG]... [--format human|json]
//
// The required file lists one tag per line; blank lines and lines starting with # are
// ignored. Exit 0: every required tag has a rule. Exit 1: some tag has none.
// Exit 2: the inventory or the arguments cannot be read.
import { readFileSync } from "node:fs";

const TAG = /^[a-z0-9-]+(\/[a-z0-9-]+)*$/;
const USAGE = "usage: rulecheck list --format json | node tools/coverage.mjs "
  + "[--required FILE] [--require TAG]... [--format human|json]";

class InputError extends Error {}

function parse(args) {
  const required = new Set();
  let format = "human";
  const add = (tag, where) => {
    if (!(TAG.test(tag) && tag.length <= 64)) {
      throw new InputError(`${where}: ${JSON.stringify(tag)} is not a tag; use lowercase letters, digits, hyphens and slashes`);
    }
    required.add(tag);
  };
  for (let index = 0; index < args.length; index += 1) {
    const [flag, value] = [args[index], args[++index]];
    if (value === undefined) throw new InputError(USAGE);
    if (flag === "--require") add(value, "--require");
    else if (flag === "--required") {
      const lines = readFileSync(value, "utf8").split("\n");
      for (const [number, line] of lines.entries()) {
        const text = line.trim();
        if (text && !text.startsWith("#")) add(text, `${value}:${number + 1}`);
      }
    } else if (flag === "--format" && ["human", "json"].includes(value)) format = value;
    else throw new InputError(USAGE);
  }
  if (required.size === 0) throw new InputError(`no required tags; ${USAGE}`);
  return { required: [...required].sort(), format };
}

function readInventory(text) {
  let inventory;
  try {
    inventory = JSON.parse(text);
  } catch {
    throw new InputError("stdin is not JSON; pipe rulecheck list --format json");
  }
  if (inventory?.inventoryVersion !== 1 || !Array.isArray(inventory.rules)
      || !inventory.rules.every((rule) => typeof rule?.id === "string" && Array.isArray(rule.tags))) {
    throw new InputError("stdin is not a version 1 rule inventory; pipe rulecheck list --format json");
  }
  return inventory;
}

function coverage(inventory, required) {
  const rulesByTag = new Map(required.map((tag) => [tag, []]));
  const untagged = [];
  for (const rule of inventory.rules) {
    if (rule.tags.length === 0) untagged.push(rule.id);
    for (const tag of rule.tags) rulesByTag.get(tag)?.push(rule.id);
  }
  const tags = required.map((tag) => ({ tag, rules: rulesByTag.get(tag).sort() }));
  return {
    coverageVersion: 1,
    policySha256: inventory.policy?.sha256 ?? null,
    required: tags,
    missing: tags.filter((entry) => entry.rules.length === 0).map((entry) => entry.tag),
    untaggedRules: untagged.sort(),
  };
}

function human(report) {
  const lines = report.required.map(({ tag, rules }) =>
    rules.length ? `covered ${tag}: ${rules.join(", ")}` : `missing ${tag}: no rule has this tag`);
  if (report.untaggedRules.length) lines.push(`untagged rules: ${report.untaggedRules.join(", ")}`);
  const covered = report.required.length - report.missing.length;
  lines.push(`${covered} of ${report.required.length} required tags covered`);
  return `${lines.join("\n")}\n`;
}

function main(args) {
  const { required, format } = parse(args);
  const report = coverage(readInventory(readFileSync(0, "utf8")), required);
  process.stdout.write(format === "json" ? `${JSON.stringify(report, null, 2)}\n` : human(report));
  return report.missing.length ? 1 : 0;
}

try {
  process.exitCode = main(process.argv.slice(2));
} catch (error) {
  console.error(`coverage: ${error.message}`);
  process.exitCode = 2;
}
