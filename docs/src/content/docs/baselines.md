---
title: Baselines
description: Adopt a rule on a repository that already violates it, without letting new violations through.
---

A new rule often finds problems that are already in the repository. A baseline lets you turn the rule on anyway. It records how many findings each rule has in each file today. A check then fails only where a file has more findings than that, and the counts can only go down over time.

The examples on this page use a synthetic `source/no-todo` rule that reports every `TODO` in `src/**`.

## Record a baseline

`rulecheck baseline` scans the checkout, counts every match of every selected rule, and prints the baseline. Pass `--baseline FILE --write` to save it:

```bash
rulecheck baseline --baseline .rulecheck/baseline.json --write
```

```json
{
  "baselineVersion": 1,
  "matches": "all",
  "rules": {
    "source/no-todo": {
      "matcher": "sha256:b87a3a8c…",
      "paths": {
        "src/a.ts": 2,
        "src/b.ts": 1
      }
    }
  }
}
```

The file is small and deterministic:

- **Counts by rule and path.** `rules` is keyed by rule ID, and `paths` by the repository-relative path with forward slashes. Each count is the number of [all-match](../cli/#every-match) findings, from 1 to 10,000. Rules and paths with no findings are left out.
- **Versioned.** `baselineVersion` is 1 and `matches` is `"all"`. [`schema/baseline-v1.json`](https://github.com/pmdroid/rulecheck/blob/main/schema/baseline-v1.json) describes the format.
- **Canonical.** Keys are sorted, indented with two spaces, and the file ends in one newline. The same counts always give the same bytes, so `git diff` shows only real changes. Rulecheck refuses any other form, including hand edits that change the layout.
- **Separate from policy.** The baseline holds counts only. Rules are still built from `.rulecheck/rules` on every run, and no generated policy is committed.

`.rulecheck/baseline.json` is the path that [exact-commit CI](#in-exact-commit-ci) reads. Locally, `FILE` can be any path, and a relative path is resolved from the current directory. `--write` replaces the file in one step and does not create directories. Without `--write`, the new baseline goes to stdout and no file changes.

A baseline is recorded only from a complete scan. In each of these cases `baseline` exits 2 and writes nothing:

- The scan stops at the [text budget](../cli/#scan-budget).
- A text rule matches no files.
- The scan finds more than 10,000 findings in total.
- A syntax rule has a finding. Syntax rules [don't count matches](../cli/#every-match), so their findings can't be recorded. Fix them or narrow the rule's scope. A syntax rule with no findings is fine, and gets no entry.

The other options are the same as for `check`: `--root`, `--commit`, `--untracked`, `--id-prefix` and `--text-budget`.

## Check against the baseline

```bash
rulecheck check --baseline .rulecheck/baseline.json
```

`check --baseline` counts every match, as `--all-matches` does, and compares each rule and path with the baseline. It never writes the file. For each rule and path:

| Findings compared with the baseline | Normal check | `--strict-baseline` |
| --- | --- | --- |
| Equal | pass | pass |
| More, including a path the baseline doesn't list | fail (exit 1) | fail (exit 1) |
| Fewer, including a listed path with no findings left | pass, with a note on stderr | fail (exit 1) until the baseline is tightened |
| Entry for a rule the policy no longer has | pass, with a note on stderr | fail (exit 1) until it is removed |

When a file has more findings than allowed, the check lists every finding of that rule in that file and then the count:

```text
$ rulecheck check --baseline .rulecheck/baseline.json
src/a.ts:1:4: [source/no-todo] Remove the marker.
src/a.ts:2:15: [source/no-todo] Remove the marker.
src/a.ts:3:4: [source/no-todo] Remove the marker.
src/a.ts: [source/no-todo] 3 findings, baseline allows 2
```

A count can't say which of the three findings is new, so all three are listed. Findings within their allowance are not printed.

Syntax rules have no allowance in a baseline check. Any syntax finding fails, at its first match in each file, as in a normal check.

A partial scan is never compared. When the scan stops at the text budget, the check exits 2 with its partial findings, as it would without a baseline.

### Equal-count replacements are not detected

A baseline stores counts, not locations. If a change removes one finding from a file and adds a different one to the same file, the count stays the same and the check passes. Only an increase is caught. To catch every new finding in a file, fix the existing ones and remove the file from the baseline.

## Tighten after a clean-up

When you fix baselined findings, regenerate the baseline in the same pull request:

```bash
rulecheck baseline --baseline .rulecheck/baseline.json --write
```

The counts go down, and a fully fixed file or rule leaves the baseline. Exact-commit CI runs the candidate's own baseline with `--strict-baseline`, so a clean-up that doesn't tighten the baseline fails there. That keeps the allowance from staying higher than the code needs.

### Regenerate selected rules

`--id-prefix PREFIX` regenerates only the rules whose ID starts with the prefix. Entries for other rules are copied from the existing file unchanged, so the existing file must be valid. A missing file is treated as empty. Entries under the prefix for rules no longer in the policy are dropped.

```bash
rulecheck baseline --baseline .rulecheck/baseline.json --id-prefix source/no-todo --write
```

Without `--id-prefix`, the baseline is rebuilt from the scan alone, and the existing file isn't read.

## Renames, removed rules and changed rules

None of these grants an allowance automatically:

- **Renamed or moved files.** The baseline doesn't follow renames. A moved file's findings are at a new path with no allowance, so the check fails, and the old path's entry has 0 findings left. A pull request can't move the allowance with the file, because `base-check` uses the base commit's baseline. Fix the findings in the moved file before or while moving it. Otherwise only a later [baseline-only pull request](#raising-an-allowance) can record the new path.
- **Removed rules.** A rule no longer in the policy is not checked. Its entry is stale. A normal check prints a note, and `--strict-baseline` fails until you regenerate the baseline.
- **Renamed rule IDs.** Renaming is a removal plus a new rule. The new ID has no allowance, and the old ID's entry is stale.
- **Changed match semantics.** Each entry records a `matcher` digest of the rule's `kind`, `pattern` and `selector`. If those change, the old counts no longer describe the rule, so the check exits 2 and names the rule. Regenerate that rule's entry in a reviewed pull request. Changes to the rule's paths, `exclude`, remediation, provenance or fixtures don't change what matches inside a file, so they keep the digest. A file that enters a wider scope has no allowance. A file that leaves the scope leaves a stale count that strict mode asks you to remove.
- **Rulecheck releases.** The digest also covers a counting version. A release that changes how matches are counted changes every digest, so existing baselines fail closed until they are regenerated.

## Invalid input fails closed

`check --baseline` exits 2 when the baseline can't be used. A file that can't be read or parsed stops the check before any file is scanned. With `--format json`, the result has a diagnostic, no findings, and no `baseline` object. These all exit 2:

- The file is missing, isn't a regular file, or is larger than 4 MiB.
- The file isn't JSON, has an unknown field, or is missing `baselineVersion`, `matches`, `rules`, `matcher` or `paths`.
- `baselineVersion` isn't 1 or `matches` isn't `"all"`.
- A rule ID is invalid, a path isn't normalized (for example `./src/a.ts`, `src/../a.ts` or a backslash), a count is 0 or above 10,000, or a rule lists no paths.
- The file isn't in canonical form, or has duplicate keys.
- An entry's `matcher` digest doesn't match the rule, or the entry is for a syntax rule.

## In exact-commit CI

The [shadow workflow](../ci/) reads `.rulecheck/baseline.json` from Git, never from the checkout:

- **`base-check`** checks the candidate with the base commit's policy and the **base commit's baseline**. So a pull request that edits the baseline can't raise its own allowance. Its new findings still fail against the base counts.
- **`proposed-check`** checks the candidate with its own policy and its own baseline, using `--strict-baseline`. A clean-up has to ship the tightened baseline, and a baseline padded above the real counts fails.
- **`proposed-owner-review`** applies because the baseline is under `.rulecheck`. A baseline change needs the policy owner's approval of the exact head, like a rule change.
- The evidence records, for each check, which baseline it used and its SHA-256. `baselineChange` lists every rule and path whose allowance the candidate raised or lowered.

A baseline that is missing at a commit means no allowance. A baseline that is a symlink or too large at either commit is a `baseline-read` error, and the run exits 2.

### Raising an allowance

Normal pull requests can only lower allowances. Raising one, for example after a change was merged over its allowance, needs its own pull request that changes nothing but `.rulecheck/baseline.json`. For that pull request:

1. `base-check` uses the proposed baseline. No code or rule changed, so this can't let in a finding that the same pull request adds.
2. Strict `proposed-check` makes the proposed counts equal the counts the base commit already has, so the baseline can't be padded for later changes.
3. `proposed-owner-review` must pass on the exact head, even when the run wasn't otherwise set up to read reviews.

`baselineChange.rebaseline` is `true` for these pull requests, and `raised` lists each allowance that went up for the reviewer.

## Result document

With `--baseline`, `check --format json` adds:

- `source.matches: "all"`, `source.baselineSha256` (the SHA-256 of the baseline bytes, or `null` if they couldn't be read), and `source.strictBaseline: true` with `--strict-baseline`.
- A `baseline` object when a complete scan was compared: `strict`, `allowedFindings` (findings within their allowance, not listed), and `exceeded` and `loose` lists of `{ ruleId, path, count, allowed }`, plus `staleRuleIds`.

`findings` lists only the findings of rules and paths in `exceeded`. Without `--baseline`, none of these fields appear, so existing results keep their shape. See the [result contract](../cli/#machine-readable-results).
