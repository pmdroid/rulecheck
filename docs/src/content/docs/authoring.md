---
title: Authoring rules
description: Define, test, and retire a rule safely.
---

1. Copy a tested rule from the Rule cookbook into `.rulecheck/rules/`. Nested directories work. Files must end in `.rule.ts`.
2. Choose a stable lowercase ID such as `config/no-debug-true`, repository-relative `paths` globs, an optional `exclude` list for files to leave out, and a concrete remediation.
3. Add at least one fixture that finds the bug and one that passes. A finding uses exact one-based line and column; `null` means no finding. Add a fixture outside the scope if the scope matters.
   Fixtures are virtual: `rulecheck test` checks each fixture's `content` as if it lived at its `path`, without reading the working tree or the Git index. The fixture `path` is matched against the rule's `paths`, so a fixture at an out-of-scope path with `expect: null` proves the scope excludes it.
4. Type-check against the SDK, validate the built policy, and run the fixtures. Inspect findings in a representative checkout before making a rule authoritative.

```bash
rulecheck validate
rulecheck test
```

A fixture mismatch exits 1 and prints one line per failing fixture on stdout. The line starts with the fixture path and the location found, or the expected location when nothing was found, and names the fixture by its one-based position:

```text
src/lib/math.ts:2:3: [source/no-console-log] fixture 1 expected a violation at 2:4, found a violation at 2:3. Use the logger instead. Update the rule or fixture.
```

## Type-check rules in your repository

In the Rulecheck checkout, `npm run typecheck` checks only Rulecheck's own rules and cookbook examples. Don't pass file names to it: `tsc` then ignores the project config and writes `.rule.js` files beside your rules. To check the rules of another repository against the SDK types, run the installed TypeScript with a throwaway config from that repository's root:

```bash
rulecheck_home=~/.local/share/rulecheck  # your Rulecheck install
config="$(mktemp -d)/tsconfig.json"
cat > "$config" <<EOF
{
  "compilerOptions": {
    "strict": true,
    "noEmit": true,
    "types": [],
    "paths": { "@rulecheck/sdk": ["$rulecheck_home/sdk/index.d.ts"] }
  },
  "include": ["$PWD/.rulecheck/rules/**/*.rule.ts"]
}
EOF
"$rulecheck_home/node_modules/.bin/tsc" -p "$config"
```

A typo such as `col` instead of `column` then fails with `TS2353` at the rule file and line. Without the type check, `rulecheck validate` still exits 2, but the error points at a position in the generated policy JSON.

Rule files import `defineRule` from `@rulecheck/sdk` and export one `defineRule({...})` declaration. The builder accepts literals, arrays, objects with explicit keys, and references to earlier literal constants. It does not execute TypeScript modules. `rulecheck` pipes the versioned JSON from the builder to the evaluator; there is no generated policy file to commit.

To retire a rule, remove the rule source in a reviewed change and document what replaced it or why it no longer applies. The shadow workflow still checks the candidate against base policy, even when proposed policy removes a rule. Retire an obsolete rule separately rather than weakening it to get another change through.
