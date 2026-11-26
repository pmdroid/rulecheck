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

The script runs the actual builder and evaluator in a temporary Git repository. It checks expected exit codes and output for all cookbook recipes and an invalid policy. It removes the temporary repository on exit.

For a rule in your own checkout, stage new candidate files first, then pipe the built policy into the evaluator. Leave `pipefail` on so builder errors propagate.

```bash
set -o pipefail
npm run --silent build -- --root /path/to/checkout |
  target/debug/rulecheck check --root /path/to/checkout
```

The evaluator reads the working-tree contents of tracked regular files. Pass a full `--commit SHA` to inspect committed blobs instead.
