# Rulecheck SDK

```sh
npm install -D @pmdroid/rulecheck typescript
```

Use `import { defineRule } from "@pmdroid/rulecheck"` in a `.rule.ts` file.
This package provides authoring types and a small identity helper. It does not install
the Rust evaluator or the CLI. [See the Rulecheck repository](https://github.com/pmdroid/rulecheck)
for CLI setup and the [rule cookbook](https://github.com/pmdroid/rulecheck/blob/main/docs/src/content/docs/cookbook.mdx).
Include `.rulecheck/rules/**/*.rule.ts` in your TypeScript project's `include`, then run
`npx tsc --noEmit` to check the types. To evaluate rules and run their fixtures, use the
separately installed CLI: `rulecheck check` and `rulecheck test`. Rule modules are parsed
as data, not executed, and no policy JSON file needs to be generated or committed.

## Releases

Increment `sdk/package.json` in a reviewed PR. From the merged `main`, run
**Stage npm SDK** and approve the staged version in npm with 2FA.

For the first release only, a maintainer of the npm scope signs in locally and runs
`npm stage publish ./sdk --access public` with npm 11.15 or newer, then reviews
and approves the stage. No CI token is needed. Once the package exists, configure
a [trusted publisher](https://docs.npmjs.com/trusted-publishers/) under the npm package settings:

- Provider: GitHub Actions
- Owner: `pmdroid`
- Repository: `rulecheck`
- Workflow: `npm-sdk.yml`
- Environment: leave blank
- Allowed operation: stage only

Set up the trusted publisher when the next version is ready to stage, since new
connections must be validated within two days. Never add an npm token to GitHub Actions.
