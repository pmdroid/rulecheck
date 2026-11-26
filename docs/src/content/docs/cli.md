---
title: CLI reference
description: Rulecheck commands, exit codes, and scan scope.
---

The installed `rulecheck` command builds policy from `.rulecheck/rules/**/*.rule.ts` in the selected checkout and pipes it to the evaluator. `--root DIR` picks the checkout root, which defaults to the current directory. `--commit SHA` reads rules and files from a full commit instead.

| Command | Purpose |
| --- | --- |
| `rulecheck check [--root DIR] [--commit SHA]` | Scan working-tree files tracked by Git, or inspect exact committed blobs. |
| `rulecheck test [--root DIR] [--commit SHA]` | Run rule fixtures. |
| `rulecheck list [--root DIR] [--commit SHA]` | List rule IDs in sorted order. |
| `rulecheck validate [--root DIR] [--commit SHA]` | Check policy syntax, rule IDs, globs, and patterns. |
| `rulecheck schema` | Print the v1 schema. |

`list`, `test`, and `check` accept `--id-prefix PREFIX`. An unmatched prefix is an error. Use `git add` for newly created files before running a working-tree check. In commit mode a scoped symlink or submodule is an error; neither the working tree nor the index is consulted.

| Exit | Meaning |
| --- | --- |
| 0 | Successful command; no violations or fixture mismatches. |
| 1 | One or more findings or fixture mismatches. |
| 2 | Could not judge, such as an invalid rule file, a builder failure, unreadable text, or a text rule matching no files. |

Findings report the first match per rule and file. Columns count Unicode scalar values, not bytes; forbidden paths use `1:1`. Narrow broad globs if the scanner reports an input limit.

## Low-level pipeline

For tooling, the two stages also run separately. `node tools/build.mjs [--root DIR] [--commit SHA]` (or `npm run --silent build -- ...`) in the Rulecheck checkout writes the policy JSON to stdout. The evaluator binary `target/release/rulecheck` (or `target/debug/rulecheck`) reads it from stdin with the same commands and flags:

```bash
set -o pipefail
node tools/build.mjs --root /path/to/checkout |
  target/release/rulecheck check --root /path/to/checkout
```

The shadow CI workflow uses this pipeline with separately pinned tools.
