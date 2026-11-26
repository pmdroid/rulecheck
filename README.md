# Rulecheck

Rulecheck reads TypeScript rules as data and checks repository files for forbidden text or paths. The installed `rulecheck` command finds `.rulecheck/rules/**/*.rule.ts` in the checkout, has the builder parse them as static data, and streams the policy straight into the Rust evaluator. No policy file is written. Rule modules are never imported or executed.

## Install once

Install Git, Node.js 20 or later with npm, and [Rust via rustup](https://rustup.rs/). On Linux or macOS, clone Rulecheck into a directory of its own, install its pinned dependencies, build the release evaluator, and link the shipped entry point into a directory on your `PATH`:

```bash
git clone https://github.com/pmdroid/rulecheck.git ~/.local/share/rulecheck
cd ~/.local/share/rulecheck
npm ci --ignore-scripts
cargo build --locked --release
mkdir -p ~/.local/bin
ln -s "$PWD/bin/rulecheck.mjs" ~/.local/bin/rulecheck
```

`rulecheck` always uses the builder, TypeScript package, and `target/release/rulecheck` evaluator from that installation. It never takes them from the repository being checked or from `PATH`. Repeat the `npm ci` and `cargo build` steps after you update the clone.

## Check a repository

Run these commands in any other Git repository. This demo creates a throwaway one:

```bash
demo=$(mktemp -d)
cd "$demo"
git init -q
mkdir -p .rulecheck/rules config
cp ~/.local/share/rulecheck/.rulecheck/rules/no-legacy-mode.rule.ts .rulecheck/rules/
printf 'legacy_mode = false\n' > config/app.toml
git add -A

rulecheck test
# 2 fixtures passed
rulecheck check
echo "$?"  # 0

printf 'name = "app"\nlegacy_mode = true\n' > config/app.toml
rulecheck check
# config/app.toml:2:1: [config/no-legacy-mode] Remove legacy_mode or set it to false.
echo "$?"  # 1

printf 'export const x = 1;\n' > .rulecheck/rules/broken.rule.ts
rulecheck check
# rulecheck build: .rulecheck/rules/broken.rule.ts:1:1: Expected the SDK marker import. Start with import { defineRule } from "@rulecheck/sdk".
# rulecheck: could not build policy from /tmp/.../.rulecheck/rules
echo "$?"  # 2

cd - && rm -r -- "$demo"
```

Run from the checkout root, or pass `--root DIR` from anywhere else. A subdirectory is rejected.

## Author and run rules

Copy the [example rule](.rulecheck/rules/no-legacy-mode.rule.ts) to create a rule. The builder discovers `.rulecheck/rules/**/*.rule.ts` in path order. Each file starts with `import { defineRule } from "@rulecheck/sdk"` and ends with one `export default defineRule({...})`. Values can be literals, arrays, objects with explicit keys, and references to earlier literal `const` declarations. Keep runtime code outside rule files. Run `npm run typecheck` in the Rulecheck checkout to check rules against the SDK types.

- `rulecheck check [--root DIR]` builds policy from the checkout's rules and checks the working-tree contents of regular files tracked there. Stage new paths with `git add` before checking. Git symlinks and submodules are skipped.
- `rulecheck test`, `rulecheck list`, and `rulecheck validate` run fixture expectations, print sorted rule IDs, and validate the built policy, including rule IDs, globs, and regex syntax. Each accepts `--root DIR` to pick the rules checkout.
- `rulecheck schema` prints the version 1 JSON Schema.
- Pass `--commit SHA` to read rules and files from a full commit in Git, including in a bare repository. Rules come from committed blobs. The evaluator reports an error for a scoped symlink or submodule. In this mode neither the index nor the checkout is consulted.

Use `--id-prefix PREFIX` with `list`, `test`, or `check` to select rules by ID prefix. A prefix with zero matches, an empty text scope, an unreadable scoped file, an invalid rule file, or a builder failure yields exit 2, with diagnostics on stderr. A forbidden-path rule passes when no existing tracked regular file matches. Exit 1 means a violation or fixture mismatch. Exit 0 means the command succeeded.

Use slash-separated relative globs in `paths`. A single `*` stays within one path segment and `**` can span directories. Text rules search case-sensitive literals or Rust regex patterns in UTF-8 text. Each rule reports the first match per file, with one-based lines and Unicode scalar columns. A forbidden-path finding uses location 1:1. A fixture uses `expect: null` for no finding or an exact `{ line, column }` for a finding. Text fixtures supply `content`.

The builder limits each source to 128 KiB. Policy input is capped at 1 MiB and 128 rules. The scanner limits each scoped text file to 1 MiB, total scoped text to 64 MiB, Git index output to 8 MiB, and output to 10,000 findings. Run against a checkout that stays stable during the command.

### Low-level pipeline

Tooling can still run the two stages separately. The builder writes the version 1 policy JSON to stdout. The evaluator binary reads it from stdin with the same commands and flags:

```bash
set -o pipefail
node tools/build.mjs --root /path/to/checkout |
  target/release/rulecheck check --root /path/to/checkout
```

`npm run --silent build -- [--root DIR] [--commit SHA]` runs the same builder. Exact-commit shadow CI uses this pipeline with separately pinned tools.

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

Run the cookbook recipes after building the CLI: `bash docs/examples/run.sh`.
Set `RULECHECK_DOCS_SITE` to the intended site origin and `RULECHECK_DOCS_BASE` to its path prefix when building for a subpath. The build output is in `docs/dist/`.
