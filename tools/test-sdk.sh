#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
demo="$(mktemp -d)"
trap 'rm -rf -- "$demo"' EXIT

cmp "$root/LICENSE" "$root/sdk/LICENSE"
npm pack "$root/sdk" --ignore-scripts --pack-destination "$demo"
cd "$demo"
tarball=(pmdroid-rulecheck-*.tgz)
test "${#tarball[@]}" -eq 1
test "$(tar -tf "${tarball[0]}" | sort)" = "$(printf '%s\n' package/LICENSE package/README.md package/index.d.ts package/index.js package/package.json)"
printf '{"private":true,"type":"module"}\n' > package.json
npm install "./${tarball[0]}" --ignore-scripts --no-audit --no-fund --offline
node --input-type=module <<'JS'
import assert from "node:assert/strict";
import { defineRule } from "@pmdroid/rulecheck";
const rule = {};
assert.strictEqual(defineRule(rule), rule);
JS
git init -q
mkdir -p .rulecheck/rules
cp "$root/docs/examples/rules/literal.rule.ts" .rulecheck/rules/
"$root/node_modules/.bin/tsc" --strict --noEmit --module nodenext .rulecheck/rules/literal.rule.ts
node "$root/tools/build.mjs" --root "$demo" >/dev/null
