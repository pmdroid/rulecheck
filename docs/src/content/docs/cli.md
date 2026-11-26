---
title: CLI reference
description: Rulecheck commands, exit codes, and scan scope.
---

| Command | Purpose |
| --- | --- |
| `npm run --silent build -- [--root DIR] [--commit SHA]` | Emit policy from a checkout or a full commit. |
| `rulecheck schema` | Print the v1 schema without policy input. |
| `rulecheck validate` | Check policy syntax, rule IDs, globs, and patterns from stdin. |
| `rulecheck list` | List rule IDs in sorted order. |
| `rulecheck test` | Run rule fixtures from stdin. |
| `rulecheck check [--root DIR] [--commit SHA]` | Scan working-tree files tracked by Git, or inspect exact committed blobs. |

`list`, `test`, and `check` accept `--id-prefix PREFIX`. An unmatched prefix is an error. Use `git add` for newly created files before running a working-tree check. In commit mode a scoped symlink or submodule is an error; neither the working tree nor the index is consulted.

| Exit | Meaning |
| --- | --- |
| 0 | Successful command; no violations or fixture mismatches. |
| 1 | One or more findings or fixture mismatches. |
| 2 | Could not judge, such as invalid policy, unreadable text, or a text rule matching no files. |

Findings report the first match per rule and file. Columns count Unicode scalar values, not bytes; forbidden paths use `1:1`. Narrow broad globs if the scanner reports an input limit.
