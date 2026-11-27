---
title: Quickstart
description: Build Rulecheck from a fresh clone and try a rule.
---

The npm package `@pmdroid/rulecheck` is the TypeScript authoring SDK, not the CLI. The full CLI currently installs from the GitHub checkout below. The Rust crate is not on crates.io yet.

## Install the `rulecheck` command

Install Git, Node.js 20 or later with npm, and Rust via rustup on Linux or macOS.

Install once from a clone that stays in place. The command uses the builder, pinned TypeScript, and release evaluator from that clone, never from the repository it checks.

```bash
git clone https://github.com/pmdroid/rulecheck.git ~/.local/share/rulecheck
cd ~/.local/share/rulecheck
npm ci --ignore-scripts
cargo build --locked --release
mkdir -p ~/.local/bin
ln -sf "$PWD/bin/rulecheck.mjs" ~/.local/bin/rulecheck
export PATH="$HOME/.local/bin:$PATH"  # add this line to your shell profile too
```

## Try a rule

After adding the CLI to your PATH, try the bundled literal rule in a throwaway Git repository:

```sh
demo=$(mktemp -d)
cd "$demo"
git init -q
mkdir -p .rulecheck/rules config
cp ~/.local/share/rulecheck/docs/examples/rules/literal.rule.ts .rulecheck/rules/
printf 'legacy_mode = false\n' > config/app.toml
git add .rulecheck/rules config/app.toml

rulecheck test
rulecheck check
printf 'legacy_mode = true\n' > config/app.toml
rulecheck check   # exit 1: config/app.toml:1:1 [config/no-legacy-spelling]
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
