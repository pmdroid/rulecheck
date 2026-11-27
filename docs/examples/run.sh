#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cli="${RULECHECK_BIN:-$root/target/debug/rulecheck}"
demo="$(mktemp -d)"
trap 'rm -rf -- "$demo"' EXIT

if [[ ! -x "$cli" ]]; then
  printf 'Build the evaluator with cargo build --locked first.\n' >&2
  exit 2
fi

check_result() {
  local expected_status="$1" expected_output="$2" status=0
  shift 2
  local output
  output="$("$@" 2>&1)" || status=$?
  if [[ "$status" != "$expected_status" || "$output" != "$expected_output" ]]; then
    printf 'Expected exit %s and %q, got exit %s and %q\n' \
      "$expected_status" "$expected_output" "$status" "$output" >&2
    return 1
  fi
}

if [[ "$#" == 0 ]]; then
  set -- literal regex path scope exclude rule-sources multi-scope path-globs syntax syntax-reuse syntax-rust
fi

for recipe in "$@"; do
  case "$recipe" in
    literal|regex|path|scope|exclude|rule-sources|multi-scope|path-globs|syntax|syntax-reuse|syntax-rust) ;;
    *) printf 'Unknown cookbook recipe: %s\n' "$recipe" >&2; exit 2 ;;
  esac
  working="$demo/$recipe"
  mkdir -p "$working/.rulecheck/rules" "$working/config" "$working/src" "$working/generated"
  git -C "$working" init -q
  cp "$root/docs/examples/rules/$recipe.rule.ts" "$working/.rulecheck/rules/"
  policy="$(node "$root/tools/build.mjs" --root "$working")"
  "$cli" validate <<<"$policy" >/dev/null
  "$cli" test <<<"$policy"

  case "$recipe" in
    literal)
      printf 'legacy_mode = false\n' > "$working/config/app.toml"
      finding='config/app.toml:2:1: [config/no-legacy-spelling] Remove the legacy setting or set it to false.'
      ;;
    regex)
      printf 'debug = false\n' > "$working/config/app.toml"
      finding='config/app.toml:1:1: [config/no-debug-true] Set debug to false before committing.'
      ;;
    path)
      printf '# normal configuration\n' > "$working/config/app.toml"
      finding='config/local.override.toml:1:1: [config/no-local-overrides] Move the local override outside the tracked checkout.'
      ;;
    scope)
      printf 'currentMarker();\n' > "$working/src/app.ts"
      printf 'legacyMarker();\n' > "$working/generated/app.ts"
      finding='src/app.ts:1:1: [source/no-legacy-marker] Replace the legacy marker in source.'
      ;;
    exclude)
      mkdir -p "$working/src/generated"
      printf 'logger.info(user);\n' > "$working/src/app.ts"
      printf 'console.log(user);\n' > "$working/src/generated/client.ts"
      printf 'console.log(user);\n' > "$working/src/app.test.ts"
      finding='src/app.ts:2:3: [source/no-console-log] Use the project logger instead of console.log.'
      ;;
    rule-sources)
      mkdir -p "$working/scripts"
      printf 'export const ready = true;\n' > "$working/src/app.ts"
      printf 'export const version = "1.0.0";\n' > "$working/scripts/release.ts"
      finding='scripts/release.ts:2:3: [source/no-debugger] Remove the debugger statement.'
      ;;
    multi-scope)
      mkdir -p "$working/packages/core/src" "$working/services/"{api,web}
      printf '// NEW_NAME\n' > "$working/packages/core/src/index.ts"
      printf '// old_name\n' > "$working/services/api/index.ts"
      printf '// OLD_NAME\n' > "$working/services/web/index.ts"
      finding=$'packages/core/src/index.ts:1:6: [source/no-old-name] Use NEW_NAME instead.\nservices/api/index.ts:1:4: [source/no-old-name] Use NEW_NAME instead.'
      ;;
    path-globs)
      mkdir -p "$working/config/team" "$working/examples"
      touch "$working/config/team/"{safe-1a,local-aa,local-1ab}.toml
      touch "$working/examples/docker-compose.override.yml"
      finding=$'config/private-2b.toml:1:1: [config/no-machine-specific-files] Keep machine-specific files out of Git.\nconfig/team/local-1a.toml:1:1: [config/no-machine-specific-files] Keep machine-specific files out of Git.\ndocker-compose.override.yml:1:1: [config/no-machine-specific-files] Keep machine-specific files out of Git.'
      ;;
    syntax)
      # Comments outside function bodies, a TODO, and JSX text are all allowed.
      printf '// Totals for the report.\nexport function total(xs: number[]): number {\n  // TODO: use reduce\n  let sum = 0;\n  for (const x of xs) sum += x;\n  return sum;\n}\n' > "$working/src/total.ts"
      printf 'export const Hint = () => <p>// type a path</p>;\n' > "$working/src/hint.tsx"
      finding='src/total.ts:2:3: [source/no-function-body-comments] Move the explanation out of the function body or into a name.'
      ;;
    syntax-reuse)
      printf 'export function first(xs: string[]): string {\n  return xs[0] ?? "";\n}\n' > "$working/src/first.ts"
      finding='src/first.ts:2:10: [source/no-non-null-assertion] Check for null or undefined instead of asserting with !.'
      ;;
    syntax-rust)
      # Doc comments, a SAFETY comment, a TODO and "//" in a string are all allowed.
      printf '//! Totals for the report.\n\n/// Sums the values.\npub fn total(xs: &[i32]) -> i32 {\n    // TODO: overflow check\n    let sep = "// none";\n    let _ = sep;\n    xs.iter().sum()\n}\n\npub fn read(p: *const i32) -> i32 {\n    // SAFETY: callers pass a valid pointer.\n    unsafe { *p }\n}\n' > "$working/src/lib.rs"
      finding='src/lib.rs:2:5: [source/no-rust-function-body-comments] Move the explanation out of the function body or into a name.'
      ;;
  esac

  git -C "$working" add -f -- .
  if [[ "$recipe" == rule-sources ]]; then
    # The same policy without exclude reports the rule's own source, at its
    # first match only, in both working-tree and commit mode.
    unexcluded="$(node -e '
      const policy = JSON.parse(require("fs").readFileSync(0, "utf8"));
      for (const rule of policy.rules) delete rule.exclude;
      process.stdout.write(JSON.stringify(policy));
    ' <<<"$policy")"
    git -C "$working" -c user.name=Rulecheck -c user.email=rulecheck@example.invalid \
      -c commit.gpgsign=false commit -qm 'Add rule'
    head="$(git -C "$working" rev-parse HEAD)"
    own='.rulecheck/rules/rule-sources.rule.ts:8:39: [source/no-debugger] Remove the debugger statement.'
    check_result 1 "$own" "$cli" check --root "$working" <<<"$unexcluded"
    check_result 1 "$own" "$cli" check --root "$working" --commit "$head" <<<"$unexcluded"
    check_result 0 '' "$cli" check --root "$working" --commit "$head" <<<"$policy"
    printf 'rule-sources: rule source reported without exclude, clean with it, in both modes\n'
  fi
  check_result 0 '' "$cli" check --root "$working" <<<"$policy"

  case "$recipe" in
    literal) printf '# settings\nlegacy_mode = true\n' > "$working/config/app.toml" ;;
    regex) printf 'debug=true # temporary\n' > "$working/config/app.toml" ;;
    path)
      touch "$working/config/local.override.toml"
      git -C "$working" add -- config/local.override.toml
      ;;
    scope) printf 'legacyMarker();\n' > "$working/src/app.ts" ;;
    exclude) printf 'export function run(user) {\n  console.log(user);\n}\n' > "$working/src/app.ts" ;;
    rule-sources) printf 'export function release() {\n  debugger;\n}\n' > "$working/scripts/release.ts" ;;
    multi-scope)
      printf '// π OLD_NAME OLD_NAME\n' > "$working/packages/core/src/index.ts"
      printf '// OLD_NAME\n' > "$working/services/api/index.ts"
      ;;
    path-globs)
      touch "$working/docker-compose.override.yml" "$working/config/private-2b.toml" "$working/config/team/local-1a.toml"
      git -C "$working" add -f -- docker-compose.override.yml config/private-2b.toml config/team/local-1a.toml
      ;;
    syntax) printf 'export function total(xs: number[]): number {\n  // add them up\n  return xs.reduce((a, b) => a + b, 0);\n}\n' > "$working/src/total.ts" ;;
    syntax-reuse) printf 'export function first(xs: string[]): string {\n  return xs[0]!;\n}\n' > "$working/src/first.ts" ;;
    syntax-rust) printf 'pub fn total(xs: &[i32]) -> i32 {\n    // add them up\n    xs.iter().sum()\n}\n' > "$working/src/lib.rs" ;;
  esac

  check_result 1 "$finding" "$cli" check --root "$working" <<<"$policy"
  if [[ "$recipe" == scope ]]; then
    printf 'currentMarker();\n' > "$working/src/app.ts"
    check_result 0 '' "$cli" check --root "$working" <<<"$policy"
  fi
  printf '%s: pass, finding, and expected location verified\n' "$recipe"
done

status=0
error="$("$cli" validate 2>&1 <<<'{"version":42,"rules":[]}')" || status=$?
[[ "$status" == 2 && "$error" == *'unsupported policy version 42'* ]]
printf 'Invalid policy: exit 2 verified\n'
