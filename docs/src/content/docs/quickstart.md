---
title: Quickstart
description: Build Rulecheck from a fresh clone and try a rule.
---

Install Git, Node.js 20 or later with npm, and Rust via rustup. Run in Bash on Linux or macOS.

```bash
git clone https://github.com/pmdroid/rulecheck.git
cd rulecheck
npm ci --ignore-scripts
cargo build --locked
bash docs/examples/run.sh
```

The script uses the debug evaluator from `cargo build --locked`. To run it against a release build instead, set `RULECHECK_BIN=target/release/rulecheck`. It runs the actual builder and evaluator in a temporary Git repository. It checks expected exit codes and output for all cookbook recipes and an invalid policy. It removes the temporary repository on exit.

## Install the `rulecheck` command

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

## Check your repository

Put rules in `.rulecheck/rules/**/*.rule.ts`, stage new candidate files, and run one command from the checkout root:

```bash
git add -A
rulecheck test    # run rule fixtures
rulecheck check   # 0 clean, 1 findings, 2 cannot judge
```

A text rule whose `paths` match no tracked file makes `check` exit 2 with `empty text scope`, so stage the files the rule covers before the first check. To type-check rules against the SDK, see [Authoring rules](../authoring/#type-check-rules-in-your-repository).

From another directory, use `rulecheck check --root /path/to/checkout`. The policy goes straight from the builder to the evaluator through a pipe. No `policy.json` is written, and nothing from the checked repository is imported or run, including its package scripts and `node_modules`.

`rulecheck check` reads the working-tree contents of tracked regular files. Pass a full `--commit SHA` to inspect committed blobs instead.
