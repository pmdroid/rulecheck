---
title: CLI reference
description: Rulecheck commands, exit codes, and scan scope.
---

The installed `rulecheck` command builds policy from `.rulecheck/rules/**/*.rule.ts` in the selected checkout and pipes it to the evaluator. `--root DIR` picks the checkout root, which defaults to the current directory. Run from the checkout root or pass `--root`: a subdirectory exits 2 and the message names the root to pass. `--commit SHA` reads rules and files from a full commit instead. `--untracked` adds untracked files that Git doesn't ignore to a working-tree check. `--all-matches` makes `check` report every text match instead of the first per rule and file; see [Every match](#every-match). `--text-budget MIB` sets how much scoped text one `check` reads; see [Scan budget](#scan-budget).

| Command | Purpose |
| --- | --- |
| `rulecheck check [--root DIR] [--commit SHA \| --untracked] [--all-matches] [--text-budget MIB] [--format human\|json]` | Scan working-tree files tracked by Git (plus non-ignored untracked files with `--untracked`), or inspect exact committed blobs. |
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
| 2 | Could not judge, such as an invalid rule file, a builder failure, unreadable text, a file in a syntax rule's scope that doesn't parse, a text or syntax rule matching no files, a syntax rule selected with `--all-matches`, or a scan that stopped at its [text budget](#scan-budget) with partial findings. |

Findings and fixture mismatches go to stdout, one per line. Exit 2 diagnostics go to stderr. `validate` prints nothing on success, and `test` prints a count such as `4 fixtures passed`. Running `rulecheck` with no command prints the usage to stderr and exits 2, so a script that drops its command never reads as clean.

The builder checks each rule's property names and value types before the evaluator sees the policy, and reports a mistake at its place in the rule file with the fixture number:

```text
rulecheck build: .rulecheck/rules/demo.rule.ts:16:26: Unknown property col in fixture 2 expect. Use line and column.
rulecheck: could not build policy from /path/to/checkout/.rulecheck/rules
```

The evaluator still checks limits, rule IDs, globs, regex syntax, and syntax selector node kinds and fields. Those messages name the rule ID, such as `[config/no-legacy-mode] invalid regex: …`.

`--commit` takes a full 40- or 64-character SHA. A short SHA or `HEAD` is an argument error: it exits 2 with one usage line and, with `--format json`, no result document. Resolve it first:

```bash
rulecheck check --commit "$(git rev-parse HEAD)"
```

By default each rule reports at most one finding per file, at its first match. This is the v1 contract: further matches in the same file are not reported until the first is fixed. Pass `--all-matches` to count every text match instead; without it, output is unchanged. Columns count Unicode scalar values, not bytes; forbidden paths use `1:1`. Narrow broad globs if the scanner reports an input limit.

## Every match

`rulecheck check --all-matches` reports one finding for every match of a text rule in a file, so a file with three `TODO` markers gives three findings rather than one. Use it when the number of violations per file matters, such as to compare counts between two commits. The default first-match output can show one finding for a file whose count has grown.

```text
$ rulecheck check --all-matches
src/app.ts:1:4: [source/no-todo-markers] Resolve the marker or move it to the issue tracker.
src/app.ts:1:12: [source/no-todo-markers] Resolve the marker or move it to the issue tracker.
src/app.ts:2:11: [source/no-todo-markers] Resolve the marker or move it to the issue tracker.
```

The matches are deterministic, and the working tree and `--commit` report the same matches for the same file contents:

- **Literal patterns** are searched left to right without overlap: after a match, the search resumes at its end. `aa` matches `aaaaa` at columns 1 and 3, not 1, 2, 3 and 4.
- **Regex patterns** use the same Rust `regex` engine as first-match mode, through its `find_iter`. Each match is the leftmost-first one, which follows alternation order rather than taking the longest: `a|ab` matches `abab` at columns 1 and 3, each one byte long. Matches don't overlap.
- **Zero-width regex matches** such as `(?m)^` or `\b` are reported, with `byteStart` equal to `byteEnd`. An empty match never splits a UTF-8 character. An empty match that starts where the previous match ended is skipped, so `a*` on `baaab` reports columns 1 (empty), 2 (`aaa`) and 6 (empty at the end), not an empty match at column 5.
- **Positions** are the same as in first-match mode: a one-based line and Unicode scalar column, plus the zero-based, half-open UTF-8 byte range in JSON. The first all-match finding of a rule in a file is always the finding that first-match mode reports.
- **Order** is path, then line and column, then rule ID, as in first-match mode. When two rules match at the same position, both are listed in rule ID order.
- **Forbidden-path rules** still report at most one finding per path, at `1:1`.
- **Syntax rules** are not counted yet. Rulecheck hasn't defined how nested or repeated nodes would be counted, and reporting only their first match would undercount. If any selected rule is a syntax rule, `--all-matches` exits 2 before reading files and names the rule. Select only text and forbidden-path rules with `--id-prefix`, or drop the flag.
- **The findings cap** is still 10,000 per scan. A scan with more matches exits 2 with no findings listed, so narrow the scope instead of reading a partial count.

`--all-matches` works with `--commit`, `--untracked`, `--id-prefix` and both output formats. It applies to `check` only; `rulecheck test` checks each fixture in the mode its expectation names. See [count every match](../cookbook/#count-every-match) for `expectAll` fixtures.

### Compatibility

The flag is opt-in, and no default changes. Without it, the human lines, the JSON document, the finding counts and the fixture format are the same as before. The policy schema is still version 1, and the result document is still `resultVersion` 1. The result adds `source.matches: "all"` only when the flag is passed, the same way `source.untracked` appears only with `--untracked`. A consumer that compares counts should reject a result without `source.matches`, because that result holds first matches only.

`expectAll` is a new, optional fixture field. Rulecheck releases before it reject a policy that uses it, as they reject any unknown field, and so do releases without `--all-matches`. Rules that use only `expect` build the same policy bytes as before.

## Scan budget

A check reads each scoped text file once, however many rules apply to it, and counts its bytes once against the scan's text budget. The budget covers the union of the selected rules' scopes. Two rules over the same files cost the same as one, but rules over different directories add up: two disjoint scopes that each fit can exceed the budget together.

`--text-budget MIB` sets the budget for one `check`, as a whole number of MiB from 1 to 1024. The default is 64. Other values, including 0, signs, fractions, units and numbers too large to parse, exit 2 with a usage message before any file is read, and `--format json` writes no document for them. The flag works the same in working-tree and `--commit` checks, through both the installed `rulecheck` command and the low-level evaluator. The 1 MiB per-file limit and the 10,000-finding limit don't change with the budget.

Narrow the rule scopes first. Excluding generated, vendored and fixture directories keeps the scan small and fast. Raise the budget when the scopes are already tight and the rules have to run together. Running one `check` per `--id-prefix` also fits, but each run starts the builder again; see the [benchmark](#benchmark-one-run-against-per-prefix-runs).

### When a scan reaches the budget

Paths are scanned in sorted order. When the next scoped text file would take the total over the budget, the scan stops reading. The result is partial, and the check always exits 2:

- Findings from the files before the stop are kept, sorted, and printed as usual. The same snapshot and budget always stop at the same file with the same findings.
- Each rule that applies to the stopping file or any later file is not fully evaluated. It gets its own diagnostic on stderr, and in JSON, and must not be read as passing. Rules that don't apply to those files were evaluated on every file in scope.
- The last line explains the stop: the file, the total it would have reached, the budget, the bytes evaluated, and how many rules were not fully evaluated.

```text
$ rulecheck check --text-budget 1
api/a.txt:1:1: [api/no-marker] Remove the marker.
web/a.txt:1:1: [web/no-marker] Remove the marker.
[web/no-marker] not evaluated on every file in scope: the scan stopped at the text budget
rulecheck: scan stopped at web/b.txt: its text would bring the scan to 1228800 bytes, over the 1 MiB text budget, after 921600 bytes were evaluated; findings are partial and 1 rule was not evaluated on every file in scope. Narrow the rule path scopes or select fewer rules with --id-prefix; raise --text-budget (at most 1024) only if the scopes are already tight
```

A partial result is never a pass, and it is never a complete list of findings. Don't use it to accept a change, to compare counts, or to create or update a baseline of existing findings.

### Benchmark: one run against per-prefix runs

`node tools/bench-text-budget.mjs` builds a synthetic repository with 4 packages of 20 MiB of text each (80 MiB in total, 160 files of 512 KiB) and one text rule per package. It checks the same commit with `--commit` two ways, through the installed wrapper and the release evaluator, and takes the median of 3 runs. With the default 64 MiB budget, one run stops at the fourth package and exits 2. One sample on an 8-core Linux runner:

| Approach | Wrapper runs | Builder starts | Scoped text read | Wall time |
| --- | --- | --- | --- | --- |
| One `--id-prefix pkgN/` run per package, default budget | 4 | 4 | 80 MiB (20 MiB per run) | 4.5 s |
| One run with `--text-budget 80` | 1 | 1 | 80 MiB | 1.7 s |

Both report the same 4 findings. Each file is read once either way, so the scan volume is the same. The difference is the per-run cost of starting Node.js and the builder, which grows with the number of prefixes. Times vary by machine, and the script prints its own measurements. Use `--packages`, `--mib` and `--runs` to change the snapshot.

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
- Findings are sorted by path, then line and column, then rule ID. `line` and `column` are the one-based values from human output, with columns counted in Unicode scalar values. `byteStart` and `byteEnd` are the zero-based, half-open UTF-8 byte range of the match, so `file[byteStart..byteEnd]` is the matched text. By default that is the rule's first match in the file; with `--all-matches` there is one finding per match. Forbidden-path findings have no text and report `null` for both.
- `diagnostics` lists each reason the check could not judge, as `{ ruleId, message }`. `ruleId` is set for an empty text scope and `null` otherwise. Any diagnostic makes the outcome `error`.
- After an empty text scope, the findings for the other rules are complete and still listed. When a scan stops early on an unreadable file or an input limit such as the per-file or findings cap, `findings` is empty. Never treat an `error` result as clean.
- When the scan stopped at the [text budget](#scan-budget), the result also has a `partial` object, and only then. `partial.reason` is `"text-budget"`. `textBudgetBytes` is the budget, `textBytesEvaluated` is the scoped text evaluated before the stop, `stoppedAt` is the file that didn't fit, and `unevaluatedRuleIds` lists, in policy order, the rules not evaluated on every file in scope. Each of those rules also has a diagnostic, and a final diagnostic with `ruleId: null` explains the stop. `findings` holds the findings from before the stop. The outcome is `error` and `exitCode` is 2, so a consumer that predates `partial` still treats the result as unjudged.
- `policy.sha256` is the SHA-256 of the exact policy bytes the evaluator read. It is `null` when the policy could not be read. It differs from the shadow evidence's effective-policy digest, which hashes a canonical form. `policy.version` is `null` when the policy is invalid. `policy.ruleIds` lists the rules selected after `--id-prefix`.
- `source.mode` is `commit` with the exact `--commit` SHA, or `working-tree` with `commit: null`. A working-tree scan includes unstaged edits, so no commit describes it exactly. `source.untracked` is `true` when `--untracked` was passed and absent otherwise, so results without the flag keep the shape shown above. `source.matches` is `"all"` when `--all-matches` was passed and absent otherwise. `source.textBudgetMiB` is the `--text-budget` value when it was passed and absent otherwise, which means 64.
- `tools.evaluator` is the evaluator version. The installed `rulecheck` command sets `tools.builder`. The low-level evaluator reports `null` because it reads policy from stdin.
- If the builder fails, the installed command still writes an error result. It carries the builder's messages as diagnostics and `null` policy fields. Invalid arguments, including a short `--commit` value, `--untracked` with `--commit`, or a `--root` that is not the checkout root, produce no document, only a usage message and exit 2. Treat a missing or unparsable document as `error`.

## Limits

Every limit fails closed with exit 2 and a message naming the limit:

| Input | Limit |
| --- | --- |
| Rule source file | 128 KiB |
| Policy JSON | 1 MiB and 128 rules |
| Scoped text file | 1 MiB |
| Scoped text in one scan | 64 MiB by default; `--text-budget` sets 1 to 1024 MiB. Reaching it stops with partial findings. |
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

Rule sources under `.rulecheck/` are scan candidates like any other file, in both working-tree and `--commit` mode. This is part of the v1 contract; omitting them automatically is deferred to a later contract version. A broad scope such as `**/*.ts` can match a rule's own pattern or fixture text, so add `exclude: [".rulecheck/**"]` to that rule. The [rule sources recipe](../cookbook/#rule-sources-in-a-broad-scope) shows the finding without the exclude and the clean check with it.

## Low-level pipeline

For tooling, the two stages also run separately. `node tools/build.mjs [--root DIR] [--commit SHA]` (or `npm run --silent build -- ...`) in the Rulecheck checkout writes the policy JSON to stdout. The evaluator binary `target/release/rulecheck-evaluator` (or `target/debug/rulecheck-evaluator`) reads it from stdin with the same commands and flags:

```bash
set -o pipefail
node tools/build.mjs --root /path/to/checkout |
  target/release/rulecheck-evaluator check --root /path/to/checkout
```

The shadow CI workflow uses this pipeline with separately pinned tools.
