---
title: Authoring rules
description: Define, test, and retire a rule safely.
---

1. Copy a tested rule from the Rule cookbook into `.rulecheck/rules/`. Nested directories work. Files must end in `.rule.ts`.
2. Choose a stable lowercase ID such as `config/no-debug-true`, repository-relative `paths` globs, an optional `exclude` list for files to leave out, and a concrete remediation. Optionally link the review finding that led to the rule with [`provenance`](#record-where-a-rule-came-from).
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

## Record where a rule came from

A rule can name the review finding that led to it:

```ts
export default defineRule({
  id: "config/no-legacy-mode",
  // ...
  provenance: { finding: "https://github.com/acme/app/pull/42#discussion_r1234" },
  remediation: "Remove legacy_mode or set it to false.",
  // ...
});
```

`provenance` is optional and holds exactly one key, `finding`: an `https://` URL of at most 2048 characters, with no spaces or control characters. The builder rejects a non-https URL, an over-long URL, an empty object or any other key at the rule file's line and column. The evaluator rejects the same policy with exit 2 and names the rule.

Matching ignores `provenance`. A rule finds the same files and lines with or without it, fixtures run the same way, and findings in human or `--format json` output don't include it. It is still part of the rule, so the policy bytes and the shadow workflow's policy digest change when you add or edit it, and the rule is listed as changed in `effectivePolicyDiff`. Policies without `provenance` build exactly as before.

Rulecheck releases from before `provenance` reject a policy that uses it as an unknown field (exit 2). Before you add it, make sure every evaluator that checks the repository, including the trusted base tools in CI, is a release that supports it.

## Syntax rules

A rule with `kind: "syntax"` takes a `selector` instead of a `pattern`. Rulecheck parses each file in scope and reports the first selected node in document order, with the same one finding per rule per file as text rules. Fixtures need `content`, as text fixtures do. The cookbook has tested examples for [TypeScript](../cookbook/#comments-in-function-bodies) and [Rust](../cookbook/#comments-in-rust-function-bodies).

```ts
selector: {
  language: "typescript",
  node: "comment",
  inside: { kinds: ["function_declaration", "arrow_function", "method_definition"], field: "body" },
  ignore: [{ kind: "regex", value: "\\b(?:TODO|FIXME)\\b" }],
}
```

- `language` is `"typescript"` or `"rust"`. With `"typescript"`, files ending in `.ts`, `.mts` or `.cts` are parsed with the TypeScript grammar and `.tsx` files with the TSX grammar. With `"rust"`, files ending in `.rs` are parsed with the Rust grammar. Any other file in scope can't be judged, so scope syntax rules to the language's extensions. One rule has one language; write a rule per language.
- `node` is a named node kind from the grammar, such as `comment`, `non_null_expression` or `call_expression` in TypeScript, or `line_comment`, `block_comment` or `unsafe_block` in Rust. The Rust grammar has no single comment kind: `line_comment` covers `//`, `///` and `//!`, and `block_comment` covers `/* */`, so a rule for both needs two rules with the same `inside`.
- `inside` is optional. It keeps a node only when one of its ancestors has a kind in `kinds` (1 to 16 entries). With `field`, the node must also lie within that ancestor's `field` child. For example, `body` selects the function body and leaves out the parameters and return type.
- `ignore` is optional. It drops a node whose source text contains a `literal` or matches a Rust `regex` (1 to 16 filters, each 1 to 4096 bytes). A dropped node doesn't hide a later one, so the next selected node is reported.

Node kinds and fields are the grammar's own names, and the evaluator checks them against the language's grammars when it loads the policy: both TypeScript and TSX for `"typescript"`, and the Rust grammar for `"rust"`. An unknown name, one that only TSX has, such as `jsx_element`, or one from the other language, such as `comment` in a Rust rule, exits 2 and names the rule. Rulecheck pins tree-sitter 0.27.2, tree-sitter-typescript 0.23.2 and tree-sitter-rust 0.24.2 exactly, because a grammar update can change which nodes a selector picks. A file in scope that doesn't parse without errors can't be judged: `check` exits 2 and names the file and position, and `test` reports that fixture as one that cannot be judged and exits 1.

Rulecheck releases from before syntax rules reject a policy that uses `kind: "syntax"` (exit 2), and releases from before Rust support reject `language: "rust"` the same way. Update every evaluator that checks the repository, including the trusted base tools in CI, before you add one.

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
