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

for recipe in literal regex path scope; do
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
  esac

  git -C "$working" add -- config src generated
  check_result 0 '' "$cli" check --root "$working" <<<"$policy"

  case "$recipe" in
    literal) printf '# settings\nlegacy_mode = true\n' > "$working/config/app.toml" ;;
    regex) printf 'debug=true # temporary\n' > "$working/config/app.toml" ;;
    path)
      touch "$working/config/local.override.toml"
      git -C "$working" add -- config/local.override.toml
      ;;
    scope) printf 'legacyMarker();\n' > "$working/src/app.ts" ;;
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
