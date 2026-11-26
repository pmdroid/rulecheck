# Rulecheck

Rulecheck reads TypeScript rules as data and checks repository files for forbidden text or paths. The TypeScript builder writes a versioned JSON policy to stdout. The Rust CLI reads that policy from stdin.

## Fresh-clone demo

Install Git, Node.js 20 or later with npm, and [Rust via rustup](https://rustup.rs/). Run these commands from the Rulecheck checkout on Linux or macOS.

```bash
npm ci --ignore-scripts
cargo build --locked

set -o pipefail
npm run --silent build | target/debug/rulecheck test
# 2 fixtures passed

demo=$(mktemp -d)
git -C "$demo" init -q
mkdir "$demo/config"
printf 'legacy_mode = false\n' > "$demo/config/app.toml"
git -C "$demo" add config/app.toml

npm run --silent build | target/debug/rulecheck check --root "$demo"
echo "$?"  # 0

printf 'name = "app"\nlegacy_mode = true\n' > "$demo/config/app.toml"
npm run --silent build | target/debug/rulecheck check --root "$demo"
# config/app.toml:2:1: [config/no-legacy-mode] Remove legacy_mode or set it to false.
echo "$?"  # 1

printf '%s\n' '{"version":42,"rules":[]}' | target/debug/rulecheck check --root "$demo"
echo "$?"  # 2

rm -r -- "$demo"
```

## Author and run rules

Copy the [example rule](.rulecheck/rules/no-legacy-mode.rule.ts) to create a rule. The builder discovers `.rulecheck/rules/**/*.rule.ts` in path order. Each file starts with `import { defineRule } from "@rulecheck/sdk"` and ends with one `export default defineRule({...})`. Values can be literals, arrays, objects with explicit keys, and references to earlier literal `const` declarations. Keep runtime code outside rule files. Use `npm run typecheck` to check rules against the SDK types.

- `npm run --silent build -- [--root DIR]` emits the policy. Pass `--root` after `--` to select a separate rules checkout.
- `rulecheck schema` prints the version 1 JSON Schema.
- `rulecheck validate` validates a policy from stdin, including rule IDs, globs, and regex syntax.
- `rulecheck test` runs fixture expectations, and `rulecheck list` prints sorted rule IDs. Both read stdin.
- `rulecheck check [--root DIR]` reads policy from stdin and checks the working-tree contents of regular files tracked in the selected Git checkout. Stage new paths with `git add` before checking. Git symlinks and submodules are skipped.
- Pass `--commit SHA` to the builder or `rulecheck check` to read a full commit from Git, including in a bare repository. The builder reads committed rule blobs. The evaluator reports an error for a scoped symlink or submodule. Neither command consults the index or checkout in this mode.

Use `--id-prefix PREFIX` with `list`, `test`, or `check` to select rules by ID prefix. A prefix with zero matches, an empty text scope, or an unreadable scoped file yields exit 2. A forbidden-path rule passes when no existing tracked regular file matches. Exit 1 means a violation or fixture mismatch. Exit 0 means the command succeeded.

Use slash-separated relative globs in `paths`. A single `*` stays within one path segment and `**` can span directories. Text rules search case-sensitive literals or Rust regex patterns in UTF-8 text. Each rule reports the first match per file, with one-based lines and Unicode scalar columns. A forbidden-path finding uses location 1:1. A fixture uses `expect: null` for no finding or an exact `{ line, column }` for a finding. Text fixtures supply `content`.

The builder limits each source to 128 KiB. Policy input is capped at 1 MiB and 128 rules. The scanner limits each scoped text file to 1 MiB, total scoped text to 64 MiB, Git index output to 8 MiB, and output to 10,000 findings. Run against a checkout that stays stable during the command.

## Rulecheck shadow CI

The `Rulecheck shadow` workflow runs for pull requests targeting `main` once the workflow is on the trusted branch. It builds the tools from the workflow commit and fetches the event's base and PR head into a bare repository. The first check uses base policy against the PR head. Separately, the workflow validates and tests policy from the PR head and checks that same head with it. A policy removal still leaves the rule active in the base check on that PR.

The `rulecheck-evidence` artifact records both commits, tool provenance and versions, phase outcomes, and SHA-256 digests of the validated policy inputs. Its diff lists added, removed, and changed rules by ID. An unavailable or invalid policy has no digest. The wrapper exits 0 on success, 1 for findings or failing fixtures, and 2 when a phase cannot judge. An error takes precedence if both occur.

GitHub requests review for policy and tool changes using the base branch's `.github/CODEOWNERS`. When the committed `.rulecheck` tree changes, the shadow report requires an approval from the policy owner on the exact PR head. Rerun the workflow after approval to refresh that phase. Approval cannot clear a base-policy violation. Verify that repository settings enforce code-owner approval before making the shadow check required.

To reproduce with trusted tools and both Git objects available locally:

```sh
npm ci --ignore-scripts
cargo build --locked
node tools/check-ci.mjs /path/to/git-repo "$BASE_SHA" "$CANDIDATE_SHA" target/debug/rulecheck > evidence.json
```

## Development

```sh
npm ci --ignore-scripts
npm run typecheck
cargo fmt --all -- --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-targets --all-features
```

## Documentation

Build or preview the [Astro Starlight docs](docs/) with Node.js 24.17 or later:

```sh
npm ci --prefix docs --ignore-scripts
npm run build --prefix docs
npm run preview --prefix docs
```

Run the four cookbook recipes after building the CLI: `bash docs/examples/run.sh`.
Set `RULECHECK_DOCS_SITE` to the intended site origin and `RULECHECK_DOCS_BASE` to its path prefix when building for a subpath. The build output is in `docs/dist/`.
