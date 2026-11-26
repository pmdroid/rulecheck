---
title: CLI reference
description: Rulecheck commands, exit codes, and scan scope.
---

The installed `rulecheck` command builds policy from `.rulecheck/rules/**/*.rule.ts` in the selected checkout and pipes it to the evaluator. `--root DIR` picks the checkout root, which defaults to the current directory. Run from the checkout root or pass `--root`: a subdirectory exits 2 and the message names the root to pass. `--commit SHA` reads rules and files from a full commit instead. `--untracked` adds untracked files that Git doesn't ignore to a working-tree check.

| Command | Purpose |
| --- | --- |
| `rulecheck check [--root DIR] [--commit SHA \| --untracked] [--format human\|json]` | Scan working-tree files tracked by Git (plus non-ignored untracked files with `--untracked`), or inspect exact committed blobs. |
| `rulecheck test [--root DIR] [--commit SHA]` | Run rule fixtures. |
| `rulecheck list [--root DIR] [--commit SHA]` | List rule IDs in sorted order. |
| `rulecheck validate [--root DIR] [--commit SHA]` | Check policy syntax, rule IDs, globs, and patterns. |
| `rulecheck schema` | Print the v1 schema. |
| `rulecheck help` | Print usage to stdout and exit 0. `-h` and `--help` do the same. |

`list`, `test`, and `check` accept `--id-prefix PREFIX`. An unmatched prefix is an error. Use `git add` for newly created files before running a working-tree check, or pass `--untracked`. In commit mode a scoped symlink or submodule is an error; neither the working tree nor the index is consulted.

| Exit | Meaning |
| --- | --- |
| 0 | Successful command; no violations or fixture mismatches. |
| 1 | One or more findings or fixture mismatches. |
| 2 | Could not judge, such as an invalid rule file, a builder failure, unreadable text, or a text rule matching no files. |

Findings and fixture mismatches go to stdout, one per line. Exit 2 diagnostics go to stderr. `validate` prints nothing on success, and `test` prints a count such as `4 fixtures passed`. Running `rulecheck` with no command prints the usage to stderr and exits 2, so a script that drops its command never reads as clean.

The builder checks each rule's property names and value types before the evaluator sees the policy, and reports a mistake at its place in the rule file with the fixture number:

```text
rulecheck build: .rulecheck/rules/demo.rule.ts:16:26: Unknown property col in fixture 2 expect. Use line and column.
rulecheck: could not build policy from /path/to/checkout/.rulecheck/rules
```

The evaluator still checks limits, rule IDs, globs, and regex syntax. Those messages name the rule ID, such as `[config/no-legacy-mode] invalid regex: …`.

`--commit` takes a full 40- or 64-character SHA. A short SHA or `HEAD` is an argument error: it exits 2 with one usage line and, with `--format json`, no result document. Resolve it first:

```bash
rulecheck check --commit "$(git rev-parse HEAD)"
```

Findings report the first match per rule and file. Columns count Unicode scalar values, not bytes; forbidden paths use `1:1`. Narrow broad globs if the scanner reports an input limit.

## Machine-readable results

`rulecheck check --format json` writes a single JSON document to stdout. It follows [`schema/result-v1.json`](https://github.com/pmdroid/rulecheck/blob/main/schema/result-v1.json). The default `--format human` prints the `path:line:column: [rule-id] remediation` lines shown above, and the two formats report the same findings in the same order. Diagnostics still go to stderr in human form, so the exit codes are unchanged.

```json
{
  "command": "check",
  "diagnostics": [],
  "exitCode": 1,
  "findings": [
    {
      "byteEnd": 28,
      "byteStart": 10,
      "column": 1,
      "line": 2,
      "path": "config/app.toml",
      "remediation": "Remove legacy_mode or set it to false.",
      "ruleId": "config/no-legacy-mode"
    }
  ],
  "outcome": "violation",
  "policy": {
    "idPrefix": null,
    "ruleIds": ["config/no-legacy-mode"],
    "sha256": "sha256:2c9b4ec5…",
    "version": 1
  },
  "resultVersion": 1,
  "source": { "commit": null, "mode": "working-tree" },
  "tools": {
    "builder": "rulecheck-builder 0.1.0 (TypeScript 5.9.3)",
    "evaluator": "rulecheck 0.1.0"
  }
}
```

The contract:

- `resultVersion` is 1. It versions this document separately from the policy `version`. Reject a version you don't know.
- `exitCode` always equals the process exit status, and `outcome` names it: `pass` (0), `violation` (1) or `error` (2).
- Findings are sorted by path, then line and column, then rule ID. `line` and `column` are the one-based values from human output, with columns counted in Unicode scalar values. `byteStart` and `byteEnd` are the zero-based, half-open UTF-8 byte range of the first match in the file, so `file[byteStart..byteEnd]` is the matched text. Forbidden-path findings have no text and report `null` for both.
- `diagnostics` lists each reason the check could not judge, as `{ ruleId, message }`. `ruleId` is set for an empty text scope and `null` otherwise. Any diagnostic makes the outcome `error`.
- After an empty text scope, the findings for the other rules are complete and still listed. When a scan stops early, for example on an unreadable file or an input limit, `findings` is empty. Never treat an `error` result as clean.
- `policy.sha256` is the SHA-256 of the exact policy bytes the evaluator read. It is `null` when the policy could not be read. It differs from the shadow evidence's effective-policy digest, which hashes a canonical form. `policy.version` is `null` when the policy is invalid. `policy.ruleIds` lists the rules selected after `--id-prefix`.
- `source.mode` is `commit` with the exact `--commit` SHA, or `working-tree` with `commit: null`. A working-tree scan includes unstaged edits, so no commit describes it exactly. `source.untracked` is `true` when `--untracked` was passed and absent otherwise, so results without the flag keep the shape shown above.
- `tools.evaluator` is the evaluator version. The installed `rulecheck` command sets `tools.builder`. The low-level evaluator reports `null` because it reads policy from stdin.
- If the builder fails, the installed command still writes an error result. It carries the builder's messages as diagnostics and `null` policy fields. Invalid arguments, including a short `--commit` value, `--untracked` with `--commit`, or a `--root` that is not the checkout root, produce no document, only a usage message and exit 2. Treat a missing or unparsable document as `error`.

## Limits

Every limit fails closed with exit 2 and a message naming the limit:

| Input | Limit |
| --- | --- |
| Rule source file | 128 KiB |
| Policy JSON | 1 MiB and 128 rules |
| Scoped text file | 1 MiB |
| Scoped text in one scan | 64 MiB |
| Git index or tree listing | 8 MiB |
| Findings | 10,000 |
| Builder run, and evaluator run under the installed command | 120 s each |

## Reproducibility

`node tools/reproduce.mjs REPO COMMIT_SHA EVALUATOR` builds policy twice from an exact commit. It runs the fixtures and the check with each build, then compares the policy bytes, test output, findings in order, and the full JSON results. The report records tool versions, policy digests and wall time for each step. Wall time is recorded but not judged, so a slow run is not a failure. The command exits 0 when both runs match, 1 when they differ, and 2 when a step cannot run. CI runs it from a fresh checkout with the pinned Node.js and Rust versions that shadow CI uses, and uploads the report as `rulecheck-reproduction`.

## What gets scanned

A file is in a rule's scope when it matches an entry in `paths` and no entry in the optional `exclude` list. Fixtures use the same scope. Globs and text patterns are case-sensitive on every platform, so `src/**` does not match `SRC/app.ts`. Use a regex with `(?i)` for case-insensitive text.

Without `--commit`, the candidate files are the regular files in the Git index, read from the working tree:

- Changes to tracked files are scanned without staging, including tracked files that match `.gitignore`.
- Untracked files are not scanned, whether ignored or not. Run `git add` first, or pass `--untracked`.
- A tracked file deleted from the working tree or the index is skipped. If that leaves a text rule with no files in scope, the check exits 2.
- Git symlinks and submodules are skipped. A scoped path that is now a directory, FIFO, or other non-regular file, or that sits under a symlinked directory, exits 2.

With `--untracked`, the candidates also include the untracked files that Git doesn't ignore, as listed by `git ls-files --others --exclude-standard`. They are checked the same way as index files:

- Ignored untracked files and tracked files deleted from the working tree are still skipped. A file both in the index and on disk is scanned once.
- An untracked symlink in scope exits 2; Git lists a symlinked directory as one entry and does not look inside it. Git never lists a FIFO or socket as untracked, so one is not a candidate, just as `git add -A` would not add it. A nested Git repository is skipped like a submodule.
- An untracked file can fill a text rule's scope, so a check that exits 2 for an empty scope can pass or fail with `--untracked`.
- `--untracked` with `--commit` is a usage error (exit 2): a commit has no untracked files.

With `--commit SHA`, the candidate files are the commit's tree, and the index and working tree are ignored. A scoped symlink or submodule exits 2.

## Rule sources

Every entry under `.rulecheck/rules` must be a regular file or a real directory, in both modes. Without `--commit`, a symlink, FIFO, socket, device, or symlinked directory there exits 2 with an error that names the path, even if its name doesn't end in `.rule.ts`. With `--commit SHA`, any entry that is not a regular blob or a tree exits 2 the same way. This keeps a rule from silently dropping out of the policy. Without `--commit`, nested Git repositories and submodules under `.rulecheck/rules` are skipped; in a commit a submodule there exits 2. Regular files that don't end in `.rule.ts` are ignored.

Without `--commit`, the builder reads `.rulecheck/rules` from disk, not from the Git index. An untracked `.rule.ts` file there is loaded as a rule whether or not you pass `--untracked`, even though, without the flag, untracked files are not scan candidates. Commit or delete experimental rule files before relying on a clean working-tree check, or use `--commit` to check exactly what is committed.

Rule sources under `.rulecheck/` are scanned like any other file. A broad scope such as `**/*.ts` can match a rule's own pattern or fixture text, so add `exclude: [".rulecheck/**"]` to that rule.

## Low-level pipeline

For tooling, the two stages also run separately. `node tools/build.mjs [--root DIR] [--commit SHA]` (or `npm run --silent build -- ...`) in the Rulecheck checkout writes the policy JSON to stdout. The evaluator binary `target/release/rulecheck` (or `target/debug/rulecheck`) reads it from stdin with the same commands and flags:

```bash
set -o pipefail
node tools/build.mjs --root /path/to/checkout |
  target/release/rulecheck check --root /path/to/checkout
```

The shadow CI workflow uses this pipeline with separately pinned tools.
