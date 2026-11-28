---
title: Quickstart
description: Build Rulecheck from a fresh clone and try a rule.
---

The npm package `@pmdroid/rulecheck` is the TypeScript authoring SDK, not the CLI. Install the CLI separately as shown below.

## Install the `rulecheck` command

Install Git, Node.js 20 or later with npm, and [Rust via rustup](https://rustup.rs/)
on Linux or macOS. Install the CLI from [crates.io](https://crates.io/crates/rulecheck):

```sh
cargo install --locked rulecheck
rulecheck --version
```

Cargo installs `rulecheck` and its companion `rulecheck-evaluator`. On the first
rule command, Rulecheck installs its pinned TypeScript compiler into a user cache
(`$XDG_CACHE_HOME/rulecheck` or `~/.cache/rulecheck`) using the npm registry.
Later runs reuse the cache. No project `npm install`, separate CLI build step, or
generated policy file is needed. Keep the two installed executables together.

## Try a rule

After adding the CLI to your PATH, try a literal rule in a throwaway Git repository:

```sh
demo=$(mktemp -d)
cd "$demo"
git init -q
mkdir -p .rulecheck/rules config
cat > .rulecheck/rules/no-legacy-mode.rule.ts <<'RULE'
import { defineRule } from "@pmdroid/rulecheck";

export default defineRule({
  id: "config/no-legacy-mode",
  kind: "text",
  paths: ["config/*.toml"],
  pattern: { kind: "literal", value: "legacy_mode = true" },
  remediation: "Remove legacy_mode or set it to false.",
  fixtures: [
    { path: "config/app.toml", content: "legacy_mode = false\n", expect: null },
    { path: "config/app.toml", content: "legacy_mode = true\n", expect: { line: 1, column: 1 } },
  ],
});
RULE
printf 'legacy_mode = false\n' > config/app.toml
git add .rulecheck/rules config/app.toml

rulecheck test
rulecheck check
printf 'legacy_mode = true\n' > config/app.toml
rulecheck check   # exit 1: config/app.toml:1:1 [config/no-legacy-mode]
```

## Check your repository

Put rules in `.rulecheck/rules/**/*.rule.ts`, stage new candidate files, and run one command from the checkout root:

```bash
git add .rulecheck/rules path/to/file-you-want-to-check
rulecheck test    # run rule fixtures
rulecheck check   # 0 clean, 1 findings, 2 cannot judge
```

A text rule whose `paths` match no tracked file makes `check` exit 2 with `empty text scope`, so stage the files the rule covers before the first check. To type-check rules against the SDK, see [Authoring rules](../authoring/#type-check-rules-in-your-repository).

From another directory, use `rulecheck check --root /path/to/checkout`. The policy goes straight from the builder to the evaluator through a pipe. No `policy.json` is written, and nothing from the checked repository is imported or run, including its package scripts and `node_modules`.

`rulecheck check` reads the working-tree contents of tracked regular files. Pass a full `--commit SHA` to inspect committed blobs instead.
