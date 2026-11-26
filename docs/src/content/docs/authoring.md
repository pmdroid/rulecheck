---
title: Authoring rules
description: Define, test, and retire a rule safely.
---

1. Copy a tested rule from the Rule cookbook into `.rulecheck/rules/`. Nested directories work. Files must end in `.rule.ts`.
2. Choose a stable lowercase ID such as `config/no-debug-true`, positive repository-relative `paths` globs, and a concrete remediation.
3. Add at least one fixture that finds the bug and one that passes. A finding uses exact one-based line and column; `null` means no finding. Add a fixture outside the scope if the scope matters.
4. Type-check against the SDK, validate the built policy, and run the fixtures. Inspect findings in a representative checkout before making a rule authoritative.

```bash
set -o pipefail
npm run typecheck
npm run --silent build | target/debug/rulecheck validate
npm run --silent build | target/debug/rulecheck test
```

Rule files import `defineRule` from `@rulecheck/sdk` and export one `defineRule({...})` declaration. The builder accepts literals, arrays, objects with explicit keys, and references to earlier literal constants. It does not execute TypeScript modules. The versioned JSON travels on stdout/stdin; there is no generated policy file to commit.

To retire a rule, remove the rule source in a reviewed change and document what replaced it or why it no longer applies. The shadow workflow still checks the candidate against base policy, even when proposed policy removes a rule. Retire an obsolete rule separately rather than weakening it to get another change through.
