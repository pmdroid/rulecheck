---
title: Rule cookbook
description: Four runnable rules for matching text, paths, and scoped files.
---

Each recipe links to the exact source used by the example script. From the repository root run `bash docs/examples/run.sh` after installing dependencies and building the CLI.

| Recipe | When to use it | Expected finding |
| --- | --- | --- |
| [Literal](https://github.com/pmdroid/rulecheck/blob/main/docs/examples/rules/literal.rule.ts) | Stop an exact legacy setting. Literal matching is case-sensitive. | `config/app.toml:2:1` |
| [Regex](https://github.com/pmdroid/rulecheck/blob/main/docs/examples/rules/regex.rule.ts) | Match a setting with optional whitespace. Uses Rust regex syntax, not JavaScript regex flags. | `config/app.toml:1:1` |
| [Forbidden path](https://github.com/pmdroid/rulecheck/blob/main/docs/examples/rules/path.rule.ts) | Prevent a local-only path from being checked in. No content inspection is needed. | `config/local.override.toml:1:1` |
| [Scoped source check](https://github.com/pmdroid/rulecheck/blob/main/docs/examples/rules/scope.rule.ts) | Block a marker in source but not generated siblings. Scope by positive `paths` globs. | `src/app.ts:1:1`; the generated file does not report a finding. |

Use slash-separated globs. A single `*` stays within a path segment; `**` can span directories. For a text rule, pick a scope with at least one matching tracked file. For a forbidden-path rule, zero matching files is a pass.

The scoped recipe deliberately writes the same marker into both files, then fixes only `src/app.ts`. The retained marker under `generated/` does not cause a finding. Version 1 does not have an `exclude` field.
