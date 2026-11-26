---
title: CI and Paws
description: How the exact-commit shadow workflow and evidence work.
---

The [Rulecheck shadow workflow](https://github.com/pmdroid/rulecheck/blob/main/.github/workflows/rulecheck-shadow.yml) builds the trusted tools from the workflow commit. It checks the exact candidate commit against base policy, then validates and tests proposed policy and checks the same candidate against it. Proposed changes cannot erase a base-policy violation on that pull request.

The `rulecheck-evidence` artifact records the base and candidate SHAs, tool versions, effective policy digests, phase results, and changed rule IDs. A broken rule or evaluator error is different from a finding.

Policy changes require exact-head owner approval for the shadow review phase. Rerun after approval to refresh the receipt. A new head needs a new receipt.

**Availability:** This workflow is advisory. It does not enable required Rulecheck checks, deploy the docs, or replace Paws Closeout. The Paws receipt comparison is a separate pilot, not an available integration.
