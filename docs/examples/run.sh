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
  set -- literal regex path scope multi-scope path-globs
fi

for recipe in "$@"; do
  case "$recipe" in
    literal|regex|path|scope|multi-scope|path-globs) ;;
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
  esac

  git -C "$working" add -f -- .
  check_result 0 '' "$cli" check --root "$working" <<<"$policy"

  case "$recipe" in
    literal) printf '# settings\nlegacy_mode = true\n' > "$working/config/app.toml" ;;
    regex) printf 'debug=true # temporary\n' > "$working/config/app.toml" ;;
    path)
      touch "$working/config/local.override.toml"
      git -C "$working" add -- config/local.override.toml
      ;;
    scope) printf 'legacyMarker();\n' > "$working/src/app.ts" ;;
    multi-scope)
      printf '// π OLD_NAME OLD_NAME\n' > "$working/packages/core/src/index.ts"
      printf '// OLD_NAME\n' > "$working/services/api/index.ts"
      ;;
    path-globs)
      touch "$working/docker-compose.override.yml" "$working/config/private-2b.toml" "$working/config/team/local-1a.toml"
      git -C "$working" add -f -- docker-compose.override.yml config/private-2b.toml config/team/local-1a.toml
      ;;
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
