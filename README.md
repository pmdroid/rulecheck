# Rulecheck

Rulecheck reads TypeScript rules as data and checks repository files for forbidden text, paths, syntax, or YAML keys and values. The installed `rulecheck` command finds `.rulecheck/rules/**/*.rule.ts` in the checkout, has the builder parse them as static data, and streams the policy straight into the Rust evaluator. No policy file is written. Rule modules are never imported or executed.

## Install the CLI

The npm package [`@pmdroid/rulecheck`](https://www.npmjs.com/package/@pmdroid/rulecheck)
is the TypeScript authoring SDK, not the CLI.

Install Git, Node.js 20 or later with npm, and [Rust via rustup](https://rustup.rs/)
on Linux or macOS. Install the CLI from [crates.io](https://crates.io/crates/rulecheck):

```sh
cargo install --locked rulecheck
rulecheck --version
```

Cargo installs `rulecheck` and its companion `rulecheck-evaluator`. On the first
rule command, Rulecheck installs its pinned TypeScript compiler into a user cache
(`$XDG_CACHE_HOME/rulecheck` or `~/.cache/rulecheck`) using the npm registry.
Later runs reuse the cache. No project `npm install`, separate CLI build step, or
generated policy file is needed. Keep the two installed executables together.

To install without Rust, download a checksummed native archive for Linux x86-64 or
macOS on Apple silicon from a GitHub release, directly or with mise, as described in
[Install from release archives](docs/src/content/docs/install.md).

To install the compiler from a private registry mirror (`RULECHECK_NPM_REGISTRY`,
with credentials in `RULECHECK_NPM_USERCONFIG`) or from a preseeded tarball with
no network (`RULECHECK_COMPILER_TARBALL`), see
[Set up the compiler from a mirror or offline](docs/src/content/docs/quickstart.md#set-up-the-compiler-from-a-mirror-or-offline).
Neither falls back to public npm, and both keep the lockfile's version and
integrity. Rule commands still need Node.js 20 or later, and the first-run
setup needs npm.

## Check a repository

Run these commands in any other Git repository. This demo creates a throwaway one:

```bash
demo=$(mktemp -d)
cd "$demo"
git init -q
mkdir -p .rulecheck/rules config
cat > .rulecheck/rules/no-legacy-mode.rule.ts <<'RULE'
import { defineRule } from "@pmdroid/rulecheck";

export default defineRule({
  id: "config/no-legacy-mode",
  kind: "text",
  paths: ["config/*.toml"],
  pattern: { kind: "literal", value: "legacy_mode = true" },
  remediation: "Remove legacy_mode or set it to false.",
  fixtures: [
    { path: "config/app.toml", content: "legacy_mode = false\n", expect: null },
    { path: "config/app.toml", content: "legacy_mode = true\n", expect: { line: 1, column: 1 } },
  ],
});
RULE
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
# rulecheck build: .rulecheck/rules/broken.rule.ts:1:1: Expected the SDK marker import. Start with import { defineRule } from "@pmdroid/rulecheck".
# rulecheck: could not build policy from /tmp/.../.rulecheck/rules
echo "$?"  # 2

cd - && rm -r -- "$demo"
```

Run from the checkout root, or pass `--root DIR` from anywhere else. A subdirectory is rejected.

## Author and run rules

For editor types in another project, install `npm install -D @pmdroid/rulecheck typescript` and include `.rulecheck/rules/**/*.rule.ts` in that project’s TypeScript configuration. The CLI still auto-compiles the static rule files with its own pinned TypeScript.

Copy the [example rule](.rulecheck/rules/no-legacy-mode.rule.ts) to create a rule. The builder discovers `.rulecheck/rules/**/*.rule.ts` in path order. Each file starts with `import { defineRule } from "@pmdroid/rulecheck"` (older rules can still use `@rulecheck/sdk`) and ends with one `export default defineRule({...})`. Values can be literals, arrays, objects with explicit keys, and references to earlier literal `const` declarations. Keep runtime code outside rule files. `npm run typecheck` in the Rulecheck checkout checks only Rulecheck's own rules. To type-check another repository's rules, install the SDK as shown in the [authoring guide](docs/src/content/docs/authoring.md#type-check-rules-in-your-repository). `rulecheck test` checks fixture `content` at the fixture `path` without reading the working tree, so an out-of-scope fixture `path` with `expect: null` proves the scope.

- `rulecheck check [--root DIR]` builds policy from the checkout's rules and checks the working-tree contents of regular files in the Git index. Unstaged edits are checked. Untracked files are not, even if they aren't ignored, so stage new paths with `git add` first or pass `--untracked`. Tracked files that are deleted are skipped. Git symlinks and submodules are skipped too. A scoped path that is now a directory, FIFO, or symlinked directory yields exit 2. Rule sources under `.rulecheck/` are scanned like other files, in this mode and with `--commit`. That is the v1 contract, and omitting them is deferred to a later version, so give a rule with a broad scope such as `**/*.ts` an `exclude: [".rulecheck/**"]` ([recipe](docs/src/content/docs/cookbook.mdx)). Every entry under `.rulecheck/rules` must be a regular file or a real directory. A symlink, FIFO, socket, device, or symlinked directory there yields exit 2 and names the path, whatever its name, so a rule can't silently drop out. Nested Git repositories and submodules under `.rulecheck/rules` are skipped.
- `rulecheck check --untracked` is for local checks before staging; CI should check the exact commit with `--commit` instead ([when to use it](docs/src/content/docs/cli.md#when-to-pass---untracked)). It also scans untracked files that Git doesn't ignore, the files `git ls-files --others --exclude-standard` lists. They are read with the same rules as index files, so an untracked symlink in scope yields exit 2, while ignored files and deleted tracked files are still skipped. Git never lists FIFOs or sockets as untracked, so they stay out of the scan. `--untracked` can't be combined with `--commit` (exit 2), and `--format json` records it as `source.untracked: true`. The flag only widens the candidate files: the builder always reads `.rulecheck/rules` from disk, so an untracked `.rule.ts` there is loaded as a rule with or without it.
- `rulecheck test`, `rulecheck list`, and `rulecheck validate` run fixture expectations, print sorted rule IDs, and validate the built policy, including rule IDs, globs, and regex syntax. Each accepts `--root DIR` to pick the rules checkout.
- `rulecheck baseline --baseline .rulecheck/baseline.json --write` records how many findings each rule has in each file, from a complete all-match scan. `rulecheck check --baseline FILE` then fails only a rule and path with more findings than recorded, including a new path. `--strict-baseline` also fails counts that went down until the baseline is regenerated. Shadow CI applies the base commit's baseline, so a pull request can't raise its own allowance. A count-only baseline doesn't notice one finding replaced by another in the same file. See [Baselines](docs/src/content/docs/baselines.md).
- `rulecheck schema` prints the version 1 JSON Schema.
- Pass `--commit SHA` to read rules and files from a full commit in Git, including in a bare repository. Rules come from committed blobs, and any entry under `.rulecheck/rules` that is not a regular blob or a tree yields exit 2. The evaluator reports an error for a scoped symlink or submodule. In this mode neither the index nor the checkout is consulted.

Use `--id-prefix PREFIX` with `list`, `test`, `check`, or `baseline` to select rules by ID prefix. A prefix with zero matches, an empty text scope, an unreadable scoped file, an invalid rule file, or a builder failure yields exit 2, with diagnostics on stderr. A forbidden-path rule passes when no existing tracked regular file matches. Add `anchors`, globs for files that always sit next to the forbidden path, to make the rule exit 2 instead when its directory is moved or deleted. Exit 1 means a violation or fixture mismatch. Exit 0 means the command succeeded.

Use slash-separated relative globs in `paths`, plus an optional `exclude` list of up to 64 globs. Named, reusable exclusion sets in `.rulecheck/sets/*.set.ts` add globs through `excludeSets`, and `skipIfContent` skips files by a bounded header check; see the authoring guide. A file is in scope when it matches `paths` and nothing in `exclude`. Fixtures follow the same rule. Globs are case-sensitive. A single `*` stays within one path segment and `**` can span directories. Text rules search case-sensitive literals or Rust regex patterns in UTF-8 text. Each rule reports at most one finding per file, at the first match, with one-based lines and Unicode scalar columns. A forbidden-path finding uses location 1:1. First match per rule per file is the v1 default: later matches in the same file appear once the first is fixed. `rulecheck check --all-matches` reports every text match instead, for per-file counts; syntax rules can't be counted yet and exit 2 under that flag. A fixture uses `expect: null` for no finding or an exact `{ line, column }` for the first finding. A text fixture can use `expectAll: [{ line, column }, ...]` instead to assert every match. Text fixtures supply `content`.

A `kind: "syntax"` rule takes a `selector` instead of a `pattern`: a named tree-sitter node kind, an optional enclosing node kind and field, and optional `ignore` text filters. `language: "typescript"` parses `.ts`, `.mts` and `.cts` files with the TypeScript grammar and `.tsx` files with the TSX grammar, `language: "rust"` parses `.rs` files, `language: "go"` parses `.go` files and `language: "python"` parses `.py` and `.pyi` files with that language's grammar, pinned to tree-sitter 0.27.2, tree-sitter-typescript 0.23.2, tree-sitter-rust 0.24.2, tree-sitter-go 0.25.0 and tree-sitter-python 0.25.0. The rule reports the first selected node in each file, so a comment rule can tell a real comment in a function body from `//` or `#` inside a string. A scoped file that doesn't parse, or a file with another extension, yields exit 2. The selector is data, so a new syntax rule needs no evaluator change. See [Syntax rules](docs/src/content/docs/authoring.md#syntax-rules) and the [cookbook recipes](docs/src/content/docs/cookbook.mdx).

A `kind: "structured"` rule takes a `structure`: `format: "yaml"`, an ordered `path` of `{ key }`, `{ index }` and `{ each: "mapping" | "sequence" }` segments, and one `assert` (`present`, `absent`, `equals`, `not-equals` or `has-key`). It reads `.yml` and `.yaml` files with the pinned saphyr-parser 0.4.1 and checks every document without executing or expanding anything. Each failing node is one finding, so `--all-matches` and baselines count them. Malformed YAML, duplicate keys, and a path through an alias or a `<<` merge key yield exit 2. See [Structured YAML rules](docs/src/content/docs/authoring.md#structured-yaml-rules).

A rule can link the review finding behind it with an optional `provenance: { finding: "https://..." }`. `finding` is the only key and must be an `https://` URL of at most 2048 characters. Matching, fixtures and finding output ignore it, but it is part of the policy, so changing it changes the policy digest. Older Rulecheck releases reject policies that use it as an unknown field, so update every evaluator that checks the repository first. See [Record where a rule came from](docs/src/content/docs/authoring.md#record-where-a-rule-came-from).

A text rule can be conditional with an optional `requires: { kind, value }`: in a file where `pattern` matches, the `requires` text must also appear somewhere in that file, or each `pattern` match is a finding at that match. Only text rules take it, and older releases reject it as an unknown field. See [Require text in the same file](docs/src/content/docs/authoring.md#require-text-in-the-same-file).

A regex text rule can add an optional `message` template such as `"{key} is set to {value}."`, whose `{name}` references must name the pattern's named groups. Each finding then reports the rendered text in `message`, beside the fixed `remediation`. Captured text is escaped and length-capped, and is never evaluated. Rules without `message` keep their output unchanged. See [Per-finding messages](docs/src/content/docs/authoring.md#per-finding-messages).

Pass `--format json` to `check` for one versioned result document on stdout ([`schema/result-v1.json`](schema/result-v1.json)). It lists findings with their line, column, and zero-based half-open UTF-8 `byteStart`/`byteEnd`, plus diagnostics, the SHA-256 of the exact policy bytes, tool versions, and the commit in `--commit` mode. Its `exitCode` matches the process exit. The default human output is unchanged. See the [CLI reference](docs/src/content/docs/cli.md#machine-readable-results) for the full contract.

The builder limits each source to 128 KiB. Policy input is capped at 4 MiB and 1024 rules, the same in the builder, the installed command, the evaluator and the schema (see [Limits](docs/src/content/docs/cli.md#limits)). The scanner limits each scoped text file to 1 MiB, total scoped text to 64 MiB by default (`check --text-budget MIB` sets 1 to 1024 MiB; a scan that reaches it exits 2 with partial findings, see [Scan budget](docs/src/content/docs/cli.md#scan-budget)), Git index output to 8 MiB, and output to 10,000 findings. The installed command stops the builder and the evaluator after 120 s each. Run against a checkout that stays stable during the command.

To verify reproducible output, run from a Rulecheck checkout `node tools/reproduce.mjs /path/to/repo "$SHA" target/release/rulecheck-evaluator`. It builds policy from that commit twice, checks the commit with each build, and exits 0 only when the policy bytes, fixture results, findings and their order all match. It records wall time without judging it. CI runs it from a fresh checkout as the `reproduce` job.

### Low-level pipeline

Tooling can still run the two stages separately from a Rulecheck source checkout, after `npm ci --ignore-scripts` and `cargo build --release` there. Neither `cargo install` nor the release archives install the builder, and no installed command prints the policy JSON; the installed `rulecheck-evaluator` reads policy from stdin. See [Installed commands and checkout tools](docs/src/content/docs/cli.md#installed-commands-and-checkout-tools). The builder writes the version 1 policy JSON to stdout. The evaluator binary reads it from stdin with the same commands and flags. If you previously called the raw Rust binary at `target/release/rulecheck`, switch that pipeline to `target/release/rulecheck-evaluator`:

```bash
set -o pipefail
node tools/build.mjs --root /path/to/checkout |
  target/release/rulecheck-evaluator check --root /path/to/checkout
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
node tools/check-ci.mjs /path/to/git-repo "$BASE_SHA" "$CANDIDATE_SHA" target/debug/rulecheck-evaluator > evidence.json
```

## Development

```sh
npm ci --ignore-scripts
npm run typecheck
cargo fmt --all -- --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-targets --all-features
cargo build --locked
node --test tests/cookbook.test.mjs
```

## Releasing the Rust CLI

In the [crate settings on crates.io](https://crates.io/crates/rulecheck/settings),
add a GitHub Actions trusted publisher for **`pmdroid/rulecheck`**, workflow
**`crates-publish.yml`**, with no environment. No long-lived token belongs in
GitHub. After the first successful OIDC release, enable **Trusted Publishing Only**
in the crate settings and revoke any bootstrap token you no longer need.

Bump the version in `Cargo.toml` and update `Cargo.lock` in a PR. After CI
passes and the PR is merged, publish a GitHub Release from that commit on
`main` tagged `rust-v<version>` (for example, `rust-v0.1.1`). The workflow
verifies and publishes that version directly to crates.io. An `sdk-v` release
does not publish the crate.

## Documentation

Build or preview the [Astro Starlight docs](docs/) with Node.js 24.17 or later:

```sh
npm ci --prefix docs --ignore-scripts
npm run build --prefix docs
npm run preview --prefix docs
```

Set `RULECHECK_DOCS_SITE` to the intended site origin and `RULECHECK_DOCS_BASE` to its path prefix when building for a subpath. The build output is in `docs/dist/`.
