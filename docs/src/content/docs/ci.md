---
title: CI and Paws
description: How the exact-commit shadow workflow and evidence work.
---

The [Rulecheck shadow workflow](https://github.com/pmdroid/rulecheck/blob/main/.github/workflows/rulecheck-shadow.yml) builds the trusted tools from the workflow commit. It checks the exact candidate commit against base policy, then validates and tests proposed policy and checks the same candidate against it. Proposed changes cannot erase a base-policy violation on that pull request.

The `rulecheck-evidence` artifact records the base and candidate SHAs, tool versions, effective policy digests, phase results, and changed rule IDs. A broken rule or evaluator error is different from a finding.

When the base commit has a `.rulecheck/baseline.json`, `base-check` uses that baseline and `proposed-check` uses the candidate's baseline with `--strict-baseline`. A pull request can't raise its own allowance, and a clean-up must ship a tightened baseline. Raising an allowance takes a pull request that changes only the baseline and has exact-head owner approval. See [Baselines in exact-commit CI](../baselines/#in-exact-commit-ci).

Policy changes require exact-head owner approval for the shadow review phase. Rerun after approval to refresh the receipt. A new head needs a new receipt.

The `Rust CI` workflow's `reproduce` job builds the tools from a fresh checkout with the same pinned Node.js and Rust versions. It then builds and checks the commit twice with `tools/reproduce.mjs`, and its `rulecheck-reproduction` artifact records whether policy bytes and results matched, with wall time per step. It does not replace the shadow evidence.

**Availability:** This workflow is advisory. It does not enable required Rulecheck checks, deploy the docs, or replace Paws Closeout. The Paws receipt comparison is a separate pilot, not an available integration.

## Moving to a required check

Rulecheck stays in shadow mode until every control below holds for one pilot repository. If the repository can't enforce one of them, it stays in shadow.

| Control | What has to be true |
| --- | --- |
| Protected workflow and tools | Branch protection or a ruleset requires code-owner review for `.github/`, `tools/`, `src/`, `schema/`, `sdk/` and the lockfiles, and no bot or app can bypass it. Actions stay pinned by commit SHA, and Node.js and Rust stay pinned by version. |
| Exact-head owner review | A policy owner who did not author the pull request approves its exact head, and `proposed-owner-review` passes on that head. |
| Owner-reviewed enrollment | The policy on the default branch got to `main` through a pull request whose owner review passed. |
| Real recurrence | A hosted draft pull request that repeats a repaired finding fails `base-check` with the rule's ID and remediation. The corrected head and an unchanged control both pass. |
| CI and Paws parity | For the same head, the CI receipt and the Paws receipt agree on SHAs, policy digests, tool versions, phases and outcome. |
| Fail-closed errors | A build, validation, evaluator or review-read error exits 2 and blocks. It never counts as a pass. |
| Merge queue | If the repository uses a merge queue, the workflow runs on `merge_group`, judges the actual merge-group SHA, and requires owner review for policy changes combined in the group. |
| Switch-back | Turning the required check back off has been tried once, and the shadow workflow went on reporting afterwards. |

### Current decision: stay in shadow

Recorded on 2026-11-27 for `pmdroid/rulecheck`. The check stays advisory:

- **Tools aren't independently protected.** Pull requests #16 and #17 changed `tools/build.mjs`, `src/` and `schema/`, and the Paws app merged them with no review. So the trusted tools can change without the owner.
- **No owner-reviewed enrollment.** The `config/no-legacy-mode` repair (#15) merged without a review while `exact-commit` was red. The owner waived the review for that milestone.
- **No hosted recurrence yet.** The repair-and-recurrence loop has only been previewed locally with `tools/check-ci.mjs`.
- **No recorded CI and Paws parity** on a real head of this repository.
- **No merge-group coverage.** This workflow runs only on `pull_request_target`, so a merge queue would get no result on the merge-group SHA.
- **Switch-back hasn't been tried.**

The fail-closed paths do hold: `tests/cli.rs` covers build, validation, evaluator and missing-review errors, and each one exits 2. Exact-head owner review holds too: a stale, dismissed or non-owner approval fails, and GitHub won't let authors approve their own pull requests.

Nothing above changes repository settings or turns on enforcement. Both need the owner's explicit approval.
