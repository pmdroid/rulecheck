---
title: Quickstart
description: Build Rulecheck from a fresh clone and try a rule.
---

The npm package `@pmdroid/rulecheck` is the TypeScript authoring SDK, not the CLI. Install the CLI separately as shown below.

## Install the `rulecheck` command

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
[Install from release archives](../install/).

## Set up the compiler from a mirror or offline

The first-run setup reads its settings only from the environment of whoever runs
`rulecheck`. It ignores every `.npmrc` (in the checked repository, your home
directory and npm's global config), `npm_config_*` variables and
`NODE_AUTH_TOKEN`, and it runs `npm ci --ignore-scripts` against Rulecheck's
embedded lockfile, so the TypeScript version and its `sha512` integrity stay
pinned whatever the source. Set at most one source:

| Setting | Source |
| --- | --- |
| nothing | `https://registry.npmjs.org/` |
| `RULECHECK_NPM_REGISTRY=https://npm.example.internal/npm/` | That registry or mirror. The lockfile's tarball URL is rewritten to it. |
| `RULECHECK_COMPILER_TARBALL=/abs/path/typescript-5.9.3.tgz` | A preseeded tarball, installed with `npm --offline` and no network access. |

With a private registry, keep credentials out of the URL. Put them in an npmrc
file outside the checked repository and point `RULECHECK_NPM_USERCONFIG` at its
absolute path:

```sh
cat > "$HOME/.config/rulecheck-mirror.npmrc" <<'NPMRC'
//npm.example.internal/npm/:_authToken=${MIRROR_TOKEN}
NPMRC
export RULECHECK_NPM_REGISTRY=https://npm.example.internal/npm/
export RULECHECK_NPM_USERCONFIG="$HOME/.config/rulecheck-mirror.npmrc"
rulecheck test
```

If the configured registry is unreachable, refuses the credentials or serves a
tarball whose integrity differs from the lockfile, setup exits 2 and says so.
Rulecheck never retries against the public registry. A URL with a user name or
password, a relative path, or a user config inside the checked repository is
rejected before npm runs.

For an air-gapped machine, fetch the tarball where the network is available and
copy it over:

```sh
curl -fsSLo typescript-5.9.3.tgz https://registry.npmjs.org/typescript/-/typescript-5.9.3.tgz
# on the offline machine
RULECHECK_COMPILER_TARBALL="$PWD/typescript-5.9.3.tgz" rulecheck test
```

Rulecheck hashes the file and compares it with the lockfile's
`sha512-jl1vZzPDinLr9eUt3J/t7V6FgNEw9QjvBPdysz9KfQDD41fQrC2Y4vKQdiaUpFT4bXlb1RHhLpp8wtm6M5TgSw==`
before npm sees it. A different file exits 2 with the expected and actual
integrity, and nothing is cached. Once setup has succeeded, later runs use the
cache and need no source or network. A cache directory
(`$XDG_CACHE_HOME/rulecheck/compiler-*`) copied from another machine with the
same Rulecheck version is used as is.

### What still needs Node

Rulecheck still needs Node.js 20 or later on `PATH` for every rule command
(`check`, `baseline`, `list`, `test`, `validate`), because the builder runs on
the pinned TypeScript compiler. The first-run setup also needs the `npm` that
ships with Node, including the offline tarball path. `rulecheck --version` and
`rulecheck schema` need neither.

## Try a rule

After adding the CLI to your PATH, try a literal rule in a throwaway Git repository:

```sh
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
git add .rulecheck/rules config/app.toml

rulecheck test
rulecheck check
printf 'legacy_mode = true\n' > config/app.toml
rulecheck check   # exit 1: config/app.toml:1:1 [config/no-legacy-mode]
```

## Check your repository

Put rules in `.rulecheck/rules/**/*.rule.ts`, stage new candidate files, and run one command from the checkout root:

```bash
git add .rulecheck/rules path/to/file-you-want-to-check
rulecheck test    # run rule fixtures
rulecheck check   # 0 clean, 1 findings, 2 cannot judge
```

A text rule whose `paths` match no tracked file makes `check` exit 2 with `empty text scope`, so stage the files the rule covers before the first check. To type-check rules against the SDK, see [Authoring rules](../authoring/#type-check-rules-in-your-repository).

From another directory, use `rulecheck check --root /path/to/checkout`. The policy goes straight from the builder to the evaluator through a pipe. No `policy.json` is written, and nothing from the checked repository is imported or run, including its package scripts and `node_modules`.

`rulecheck check` reads the working-tree contents of tracked regular files. Pass a full `--commit SHA` to inspect committed blobs instead.

## Common surprises when moving rules over

These come up most when you port checks from a grep script, a lint rule or a CI job into Rulecheck:

- **A new file isn't reported.** `check` scans files in the Git index. Run `git add` first, or pass `--untracked` for a local check before staging. In CI, check the exact commit with `--commit "$(git rev-parse HEAD)"` instead. See [when to pass `--untracked`](../cli/#when-to-pass---untracked).
- **The rule reports its own file.** Rule sources under `.rulecheck/` are scanned like any other file, so a broad scope such as `**/*.ts` finds the rule's pattern and fixture text. Add `exclude: [".rulecheck/**"]` to that rule ([recipe](../cookbook/#rule-sources-in-a-broad-scope)).
- **A JavaScript regex doesn't compile or matches differently.** Patterns use the Rust `regex` dialect, with no look-around or backreferences, Unicode `\d`, `\w` and `\b`, nested character classes, and inline flags instead of `/…/i`. See [Port a JavaScript regex](../authoring/#port-a-javascript-regex) for the differences and rewrites, and add a fixture for each input the old rule caught.
- **Only the first match per file is reported.** That is the v1 default. Pass `--all-matches` to count every text match ([Every match](../cli/#every-match)).
- **A command from these docs isn't installed.** `node tools/build.mjs` and the other `node tools/...` scripts need a Rulecheck source checkout. Cargo and the release archives install only `rulecheck` and `rulecheck-evaluator` ([Installed commands and checkout tools](../cli/#installed-commands-and-checkout-tools)).
