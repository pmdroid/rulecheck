---
title: Authoring rules
description: Define, test, and retire a rule safely.
---

1. Copy a tested rule from the Rule cookbook into `.rulecheck/rules/`. Nested directories work. Files must end in `.rule.ts`.
2. Choose a stable lowercase ID such as `config/no-debug-true`, repository-relative `paths` globs, an optional `exclude` list or [`excludeSets`](#reusable-exclusion-sets-and-content-skips) for files to leave out, and a concrete remediation. A forbidden-path rule for a path inside a directory that could move should also list [`anchors`](../cookbook/#forbidden-path-with-an-anchor), so the rule exits 2 instead of passing when the directory disappears. Optionally link the review finding that led to the rule with [`provenance`](#record-where-a-rule-came-from), and give a regex rule a [per-finding `message`](#per-finding-messages).
3. Add at least one fixture that finds the bug and one that passes. `expect` takes the first finding's exact one-based line and column; `null` means no finding. A text fixture can use `expectAll: [{ line, column }, ...]` instead, to assert every match that `check --all-matches` reports, in ascending order ([example](../cookbook/#count-every-match)). Add a fixture outside the scope if the scope matters.
   Fixtures are virtual: `rulecheck test` checks each fixture's `content` as if it lived at its `path`, without reading the working tree or the Git index. The fixture `path` is matched against the rule's `paths`, so a fixture at an out-of-scope path with `expect: null` proves the scope excludes it.
4. Type-check against the SDK, validate the built policy, and run the fixtures. Inspect findings in a representative checkout before making a rule authoritative.

```bash
rulecheck validate
rulecheck test
```

A fixture mismatch exits 1 and prints one line per failing fixture on stdout. The line starts with the fixture path and the location found, or the expected location when nothing was found, and names the fixture by its one-based position:

```text
src/lib/math.ts:2:3: [source/no-console-log] fixture 1 expected a violation at 2:4, found a violation at 2:3. Use the logger instead. Update the rule or fixture.
```

An `expectAll` mismatch starts with the first position where the lists differ and shows both lists, up to 16 positions each:

```text
src/lib/math.ts:4:3: [source/no-console-log] fixture 2 expected 1 violation at 2:3, found 2 violations at 2:3, 4:3. Use the logger instead. Update the rule or fixture.
```

## Pair a positive and a negative fixture

Give every rule at least two fixtures: a positive one that expects a violation, and a negative one that expects none. The positive fixture proves the rule catches the bug. The negative fixture proves it leaves the fixed code alone. Either one alone can pass for the wrong reason. A rule with only negative fixtures passes `rulecheck test` even if its pattern, selector or path globs can never match anything.

```ts
fixtures: [
  // Positive: the bug is reported at its exact place.
  { path: "config/app.toml", content: "name = \"app\"\nlegacy_mode = true\n", expect: { line: 2, column: 1 } },
  // Negative: the fixed setting is clean.
  { path: "config/app.toml", content: "legacy_mode = false\n", expect: null },
],
```

The pair applies to every rule kind. A forbidden-path rule pairs a fixture at a forbidden path, `expect: { line: 1, column: 1 }`, with one at an allowed path, `expect: null`. A syntax rule pairs content with the node, such as a comment, with content that has none. An `expectAll` fixture counts as positive when it lists at least one location; `expectAll: []` is negative.

### Strict fixture coverage

`--strict-fixtures` on `test` or `validate` fails any selected rule that has no positive fixture:

```bash
rulecheck test --strict-fixtures
```

```text
4 fixtures passed
config/app.toml:1:1: [config/no-debug-true] has no positive fixture: its only fixture expects no violation, so the rule would pass even if it never matched. Add a fixture that expects a violation.
```

Each line names the rule's first fixture path. The command exits 1 when a rule has no positive fixture, and prints `every selected rule has a positive fixture` when all have one. `--id-prefix` limits the check to the selected rules. A rule with only positive fixtures passes; add a negative one anyway. A rule with no fixtures is not a coverage gap: the policy loader rejects it in every mode, and the command exits 2.

The flag is opt-in, so existing callers are unaffected. Plain `rulecheck test` and `rulecheck validate`, their output and their exit codes are the same as before, and the policy format does not change. To adopt it, run `rulecheck test --strict-fixtures` locally, add a positive fixture to each rule it reports, then add the flag to your CI step. Shadow CI (`tools/check-ci.mjs`) still runs plain `test`.

Fixture coverage and [baselines](../baselines/) are separate. A baseline allows existing findings in the repository; it never counts fixtures, and `--strict-fixtures` never reads a baseline. A rule with a baseline allowance still needs its own positive fixture.

## Require text in the same file

A text rule with `requires` reports its `pattern` matches only in files where the `requires` text appears nowhere:

```ts
export default defineRule({
  id: "source/interval-needs-clear",
  kind: "text",
  paths: ["src/**/*.ts"],
  pattern: { kind: "regex", value: "\\bsetInterval\\(" },
  requires: { kind: "literal", value: "clearInterval(" },
  remediation: "Keep the interval handle and call clearInterval when the work stops.",
  // ...
});
```

Give such a rule fixtures for all three cases: no trigger (`expect: null`), the trigger with its requirement (`expect: null`), and the trigger without it (`expect` at the first trigger). Findings always point at the trigger. Default checks report the first trigger and `--all-matches` reports every trigger. `rulecheck check` and `rulecheck check --commit` judge the same file contents the same way. The [cookbook recipe](../cookbook/#require-text-in-the-same-file) runs both.

`requires` is a `{ kind: "literal" | "regex", value }` object, like `pattern`. The builder rejects it on syntax and forbidden-path rules at the rule file's line and column. The evaluator rejects a regex requirement that can match empty text, because every file would meet it. The baseline `matcher` digest includes `requires`, so changing the requirement invalidates that rule's recorded counts. Rulecheck releases from before `requires` reject a policy that uses it as an unknown field (exit 2), so update every evaluator that checks the repository, including the trusted base tools in CI, before you use it.

## Skip matches in comments and strings

A text rule with `ignoreInside` skips a `pattern` match that lies wholly inside a parsed comment or string:

```ts
export default defineRule({
  id: "source/no-eval",
  kind: "text",
  paths: ["src/**/*.ts", "src/**/*.tsx"],
  pattern: { kind: "regex", value: "\\beval\\(" },
  ignoreInside: { language: "typescript", contexts: ["comment", "string"] },
  remediation: "Parse the input instead of calling eval.",
  // ...
});
```

The filter applies to each match, not to the file. A skipped match never hides a later one, in the default first-match check, with `--all-matches` and in fixtures. `language` is `"typescript"` or `"bash"`. With `"typescript"`, `.ts`, `.mts` and `.cts` files parse with the TypeScript grammar and `.tsx` files with the TSX grammar. With `"bash"`, `.sh` and `.bash` files parse with the pinned Bash grammar; see [Shell scripts](#shell-scripts) below. `contexts` lists `"comment"`, `"string"` or both, each once. A string context is a quoted string or the literal text of a template string, not a `${...}` substitution. With `requires`, only the `pattern` matches are filtered.

The file is parsed only when the pattern matches in it. If it can't be classified, because it doesn't parse or the grammar doesn't cover its extension, `check` exits 2 and `test` reports the fixture as one that cannot be judged. Rulecheck never guesses whether such a match counts. Narrow `paths` to the files the grammar covers. Only text rules take `ignoreInside`. The builder rejects it on syntax and forbidden-path rules, and the baseline `matcher` digest includes it. Rulecheck releases from before `ignoreInside` reject a policy that uses it as an unknown field (exit 2), so update every evaluator that checks the repository, including the trusted base tools in CI, before you use it. The [cookbook recipe](../cookbook/#skip-matches-in-comments-and-strings) runs it.

### Shell scripts

With `language: "bash"`, `"comment"` covers `#` comments and `"string"` covers `'...'`, `$'...'` and the literal text of `"..."`. Inside double quotes, `$x`, `${...}`, `$((...))` and `$(...)` are code, so a match there is reported. A heredoc body is data the script passes to a command, so it is never a comment or a string, whether or not its delimiter is quoted. A `#` inside a word, as in `a#b` or `x=#y`, does not start a comment.

The pinned grammar, tree-sitter-bash 0.25.1, mostly reads a script the way bash does, but it doesn't in a few places. In each one, the grammar can read code that bash runs as a comment or string. A file that has one of these constructs can't be classified, and `check` exits 2 once the pattern matches in it:

- A heredoc that isn't closed by a line that is exactly its delimiter, after tabs only for `<<-`. The grammar also ends a heredoc at ` EOF`, `EOF ` or `EOF;`, then reads the rest of the body as code. The delimiter must be letters, digits and `_`, bare, `\`-escaped or in one pair of quotes.
- A `#` that bash doesn't treat as the start of a word, such as one right after a `\`-newline continuation. For bash, `true\` followed by `# ; rm x` on the next line runs `rm x`. The grammar reads it as a comment.
- A carriage return, vertical tab or form feed anywhere in the file. The grammar reads them as blanks, but bash reads them as part of a word, so `true` followed by a carriage return and `# ; rm x` runs `rm x`. Convert CRLF scripts to LF.
- A backquoted `` `...` `` substitution. The grammar parses it into the same `command_substitution` node as `$(...)`, but bash re-reads backquoted text with its own escaping, so nested `` \` `` runs a command that the grammar reads as a word. Write `$(...)` instead.

A string is skipped even when it holds code that a command runs later, as in `sh -c '...'`, `eval "..."` or `ssh host '...'`. The grammar can't tell which arguments a command executes, so `"string"` suits rules whose quoted mentions really are just text.

Rulecheck also looked at classifying which command segment a match is in: the command name or its arguments, or the stage of a pipeline or list. The grammar gives each of these its own node (`command_name`, `command`, `pipeline`, `list`, `command_substitution`, `subshell`), so finding the segment is deterministic. Skipping by segment would not be fail-closed. Whether an argument runs depends on the command (`sudo rm`, `xargs rm`, `env rm`, `find -exec rm`), and nothing in the parse tree shows that. Backquoted and `$(...)` substitutions also share one node kind. So `ignoreInside` offers no segment contexts.

## Record where a rule came from

A rule can name the review finding that led to it:

```ts
export default defineRule({
  id: "config/no-legacy-mode",
  // ...
  provenance: { finding: "https://github.com/acme/app/pull/42#discussion_r1234" },
  remediation: "Remove legacy_mode or set it to false.",
  // ...
});
```

`provenance` is optional and holds exactly one key, `finding`: an `https://` URL of at most 2048 characters, with no spaces or control characters. The builder rejects a non-https URL, an over-long URL, an empty object or any other key at the rule file's line and column. The evaluator rejects the same policy with exit 2 and names the rule.

Matching ignores `provenance`. A rule finds the same files and lines with or without it, fixtures run the same way, and findings in human or `--format json` output don't include it. It is still part of the rule, so the policy bytes and the shadow workflow's policy digest change when you add or edit it, and the rule is listed as changed in `effectivePolicyDiff`. Policies without `provenance` build exactly as before.

Rulecheck releases from before `provenance` reject a policy that uses it as an unknown field (exit 2). Before you add it, make sure every evaluator that checks the repository, including the trusted base tools in CI, is a release that supports it.

## Tag rules for coverage

A rule can carry stable tags, such as the architecture decision record it enforces or the domain it guards:

```ts
export default defineRule({
  id: "billing/no-float-amounts",
  // ...
  tags: ["adr/0007", "domain/billing"],
  remediation: "Parse amounts as integer cents (ADR 0007).",
  // ...
});
```

`tags` is optional. It holds 1 to 16 distinct tags. Each tag is at most 64 characters and is shaped like a rule ID: lowercase letters, digits and hyphens in parts joined by `/`. A tag is a label, not a link, so keep the same spelling for the same decision or domain across rules and repositories. The builder rejects an empty list, a non-string, a malformed tag or a repeated tag at the rule file's line and column. The schema (`uniqueItems`) and the evaluator reject the same policy with exit 2 and name the rule.

Matching ignores tags, as it ignores `provenance`. A rule finds the same files and lines with or without them, fixtures run the same way, findings in human or `--format json` output don't include them, and the baseline `matcher` digest leaves them out, so recorded counts stay valid. Tags are still part of the rule, so the policy bytes and the shadow workflow's policy digest change when you add, remove or reorder them. `rulecheck list` prints the same rule IDs as before. `rulecheck list --format json` prints each rule's tags, sorted, in the [rule inventory](../cli/#rule-inventory), and the [coverage recipe](../cookbook/#tag-coverage) reports which required tags have a rule.

Rulecheck releases from before `tags` reject a policy that uses them as an unknown field (exit 2). Before you add them, make sure every evaluator that checks the repository, including the trusted base tools in CI, is a release that supports them.

## Reusable exclusion sets and content skips

A rule can leave files out in three ways. They run in this order, in `rulecheck check` and `rulecheck test` alike:

1. `paths` puts a file in scope.
2. `exclude` and `excludeSets` take it out by path. Rulecheck doesn't open a file excluded this way, so an excluded file can't make the scan fail.
3. `skipIfContent` takes out a file that is still in scope, based on its first bytes.

### Named exclusion sets

Put a list of exclusion globs that several rules share in `.rulecheck/sets/<name>.set.ts`:

```ts
import { defineGlobSet } from "@pmdroid/rulecheck";

export default defineGlobSet({
  version: 1,
  globs: ["**/__generated__/**", "**/*.pb.ts"],
  include: ["third-party"],
});
```

The file name is the set's name: `generated.set.ts` is `generated`, and `web/vendor.set.ts` is `web/vendor`. Names use lowercase letters, digits and hyphens. A rule adds a set's globs to its `exclude` with `excludeSets: ["generated"]`. `include` adds the globs of other sets. A path matching any resolved glob is out of scope. Lists have no order or precedence, so a later set can't put a file back in scope.

Set files follow the same static subset as rule files: one import of `defineGlobSet`, literal `const` declarations and one `export default defineGlobSet({...})`. The builder parses them without running them, and a rule can't `import` a set. With `--commit`, the builder reads sets and rules from the same commit tree. A working-tree build reads both from the checkout. A set added only to the working tree is unknown to a commit build, which fails at the `excludeSets` entry that names it. The builder also rejects a `.rulecheck/sets` that is a symlink or a nested repository. `version: 1` is the only set version.

Limits, checked when the policy loads (exit 2):

| Limit | Value |
| --- | --- |
| `paths` and `exclude` globs per rule | 1 to 64 each (16 before sets) |
| Globs per set | up to 256, each up to 256 bytes |
| Sets per policy | 64 |
| Names in `excludeSets` or `include` | 1 to 16, no duplicates |
| Include depth | 4 levels, counting the set a rule names |
| Include cycles | rejected, with the cycle path |
| Distinct exclusion globs per rule after resolving sets | 256 |
| `paths` plus resolved exclusion globs over all rules | 4096 |

Every set is checked even when no rule uses it. Shared globs and a set reached twice count once. The caps come from timing `globset` on 200,000 synthetic paths: matching cost grows about linearly with the glob count, from 0.16 µs per path at 16 globs to 0.28 µs at 64 and 0.86 µs at 256. The policy-wide total bounds the work per path.

### Skip files by content

`skipIfContent` skips an in-scope file whose first `firstBytes` bytes contain a `literal` or match a Rust `regex`, for example a generated-file header:

```ts
skipIfContent: { kind: "regex", value: "^// @generated", firstBytes: 64 },
```

- Only text and syntax rules accept it, because forbidden-path rules never read content.
- `firstBytes` is required, from 1 to 65536. Rulecheck checks the predicate against those bytes only. A match must end inside them, and the cut can fall inside a multi-byte character.
- A syntax rule skips the file before parsing it, so a generated file that doesn't parse is skipped instead of failing.
- Fixtures go through the same check. A fixture whose `content` starts with the header expects `null`.
- The predicate only sees content that Rulecheck has read. A file in scope that can't be read still can't be judged and the check exits 2, even if its header would match. That covers a symlink, a file over 1 MiB and a file that isn't UTF-8. A file read for the predicate also counts toward `check --text-budget`: when it would exceed the budget, the scan stops before the predicate runs and reports the rule as not evaluated.
- A skipped file counts as out of scope. A text rule whose every in-scope file is skipped has an empty scope, and the check exits 2.

Policies without `excludeSets`, `skipIfContent` or set files build exactly as before. Releases from before these fields reject them as unknown (exit 2). Before you use them, make sure every evaluator that checks the repository supports them, including the trusted base tools in CI.

## Per-finding messages

A regex text rule can add a `message` template that tells each finding what it matched. The template is plain text with `{name}` references to the pattern's named groups:

```ts
export default defineRule({
  id: "config/no-legacy-settings",
  kind: "text",
  paths: ["config/*.toml"],
  pattern: { kind: "regex", value: "(?m)^(?<key>legacy_[a-z_]+) = (?<value>[^\\n]*)" },
  message: "{key} is set to {value}.",
  remediation: "Remove legacy_* settings; the defaults replace them.",
  // ...
});
```

A finding on `legacy_port = 8080` reads `legacy_port is set to 8080. Remove legacy_* settings; the defaults replace them.` The fixed `remediation` is always reported too, so the message carries the detail and the remediation says what to do. Write the message as a sentence. Human output prints it, a space, then the remediation.

The rules for a template:

- Only `kind: "text"` rules with a `regex` pattern can have one. A literal pattern has no groups to reference, so the builder and the evaluator reject `message` on literal, forbidden-path and syntax rules.
- A reference is `{name}`, where the name starts with a letter or underscore and continues with letters, digits or underscores. Every reference must name a group of the pattern, written `(?<name>...)` or `(?P<name>...)`. Numbered references such as `{1}` are not supported. Write `{{` or `}}` for a literal brace.
- A template is a single line of at most 512 bytes, with no control characters, and references 1 to 16 groups.
- The evaluator checks every reference when the policy loads and exits 2 with the rule ID on an unknown name, an unclosed `{` or a stray `}`.

How a message renders:

- Each reference is replaced by the text its group captured in that match. A group that did not take part in the match, such as an optional `(?<note> #old)?`, renders as empty text, the same as a group that matched nothing.
- Control characters in captured text (newlines, tabs, carriage returns, terminal escapes, DEL and C1) and the invisible Unicode format characters that reorder or hide text (bidirectional controls, zero-width characters, the BOM) are written as `\n`, `\r`, `\t` or `\u{hex}`. A finding stays one line and shows what is really in the file.
- Each captured value keeps at most 64 characters after escaping and ends with `…` when cut, without splitting an escape. The whole message is cut to 1024 bytes the same way.
- Rendering is a plain copy. Captured text that looks like a reference, `$1`, or regex syntax stays literal text, and neither the template nor the captured text is evaluated or compiled as a regex.

`message` does not change what matches. Fixtures, `expectAll` and the baseline matcher digest are the same with or without it. Rules without `message` produce exactly the output they did before. Rulecheck releases from before `message` reject a policy that uses it as an unknown field (exit 2), so update every evaluator that checks the repository, including the trusted base tools in CI, before you add one.
## Consistency rules

A rule with `kind: "consistency"` takes a `consistency` block instead of a `pattern` or `selector` and relates files of its inventory (`paths`, `exclude` and `excludeSets`) to each other. `form: "same-values"` compares the values that two named scopes, `left` and `right`, extract. `form: "path-exists"` checks that every reference a `source` scope extracts names a regular file of the inventory, resolved from the repository root (`base: "root"`) or the referencing file's directory (`base: "file"`). Each scope has a `name`, its own `paths` and optional `exclude` inside the inventory, and an `extract` regex with exactly one capture group. The cookbook shows [both forms](../cookbook/#keep-two-files-in-step) with their normalization, traversal and limit rules.

Fixtures for consistency rules list a synthetic inventory instead of one file:

```ts
fixtures: [
  {
    files: [
      { path: "docs/flags.md", content: "- `beta_search`\n" },
      { path: "config/flags/a.json", content: "{ \"beta_search\": true }\n" },
    ],
    expectFindings: [],
  },
],
```

Files in a scope need `content`; other files only need a `path`. `expectFindings` lists every finding as `{ path, line, column }`, ordered by path, then location. A fixture in which a scope matches no file fails, because the check could not judge it either. The baseline `matcher` digest includes the `consistency` block. Releases from before consistency rules reject `kind: "consistency"` (exit 2), so update every evaluator that checks the repository, including the trusted base tools in CI, before you add one.

## Syntax rules

A rule with `kind: "syntax"` takes a `selector` instead of a `pattern`. Rulecheck parses each file in scope and reports the first selected node in document order, with the same one finding per rule per file as text rules. Fixtures need `content`, as text fixtures do. The cookbook has tested examples for [TypeScript](../cookbook/#comments-in-function-bodies), [Rust](../cookbook/#comments-in-rust-function-bodies), [Go](../cookbook/#comments-in-go-function-bodies) and [Python](../cookbook/#comments-in-python-functions).

```ts
selector: {
  language: "typescript",
  node: "comment",
  inside: { kinds: ["function_declaration", "arrow_function", "method_definition"], field: "body" },
  ignore: [{ kind: "regex", value: "\\b(?:TODO|FIXME)\\b" }],
}
```

- `language` is `"typescript"`, `"rust"`, `"go"` or `"python"`. With `"typescript"`, files ending in `.ts`, `.mts` or `.cts` are parsed with the TypeScript grammar and `.tsx` files with the TSX grammar. With `"rust"`, files ending in `.rs` are parsed with the Rust grammar, with `"go"` files ending in `.go` with the Go grammar, and with `"python"` files ending in `.py` or `.pyi` with the Python grammar. Any other file in scope can't be judged, so scope syntax rules to the language's extensions. One rule has one language; write a rule per language.
- `node` is a named node kind from the grammar, such as `comment`, `non_null_expression` or `call_expression` in TypeScript, or `line_comment`, `block_comment` or `unsafe_block` in Rust. The Rust grammar has no single comment kind: `line_comment` covers `//`, `///` and `//!`, and `block_comment` covers `/* */`, so a rule for both needs two rules with the same `inside`. Go and Python each have one `comment` kind; Go's covers both `//` and `/* */`.
- `inside` is optional. It keeps a node only when one of its ancestors has a kind in `kinds` (1 to 16 entries). With `field`, the node must also lie within that ancestor's `field` child. For example, `body` selects the function body and leaves out the parameters and return type. In Python, a comment before a function's first statement belongs to `function_definition`, not its `body`, so a Python rule that should see it omits `field`.
- `ignore` is optional. It drops a node whose source text contains a `literal` or matches a Rust `regex` (1 to 16 filters, each 1 to 4096 bytes). A dropped node doesn't hide a later one, so the next selected node is reported.

Node kinds and fields are the grammar's own names, and the evaluator checks them against the language's grammars when it loads the policy: both TypeScript and TSX for `"typescript"`, and the one grammar for `"rust"`, `"go"` or `"python"`. An unknown name, one that only TSX has, such as `jsx_element`, or one from the other language, such as `comment` in a Rust rule, exits 2 and names the rule. Rulecheck pins tree-sitter 0.27.2, tree-sitter-typescript 0.23.2, tree-sitter-rust 0.24.2, tree-sitter-go 0.25.0 and tree-sitter-python 0.25.0 exactly, because a grammar update can change which nodes a selector picks. A file in scope that doesn't parse without errors can't be judged: `check` exits 2 and names the file and position, and `test` reports that fixture as one that cannot be judged and exits 1. Files are read up to 1 MiB, and matching walks the tree once, so deep nesting doesn't make a check slow. Syntax rules report the first selected node only, and `check --all-matches` refuses them.

Rulecheck releases from before syntax rules reject a policy that uses `kind: "syntax"` (exit 2), and releases from before Rust, Go or Python support reject that `language` the same way. Update every evaluator that checks the repository, including the trusted base tools in CI, before you add one.

## Port a JavaScript regex

Every regex in a rule (`pattern`, `requires`, `skipIfContent` and a selector `ignore` filter) uses the dialect of the Rust [`regex`](https://docs.rs/regex/1/regex/#syntax) crate. That is the only dialect: there is no JavaScript mode and no second engine. The crate matches in time linear in the file size, whatever the pattern and the input. Write the pattern as a string, not a `/…/` literal, and double each backslash inside the TypeScript string. Write JavaScript flags as inline flags: `(?i)`, `(?m)` and `(?s)`. `g` has no equivalent and isn't needed: `check` reports the first match, and `--all-matches` reports every match.

Most JavaScript patterns mean the same thing in Rust. These don't:

| JavaScript | Rust regex | Write instead |
| --- | --- | --- |
| `[` inside a class is a literal bracket: `[^[]`, `[[\]]` | `[` opens a nested class, so these don't compile | `[^\[]`, `[\[\]]` |
| `[^]` matches any character | doesn't compile | `(?s:.)` |
| `&&`, `--` and `~~` inside a class are literal characters (without the `v` flag) | intersection, difference and symmetric difference: `[a&&b]` compiles and never matches | `[a\&\&b]`, `[a\-\-b]`, `[a\~\~b]` |
| `[[a]]` is the class `[[a]` followed by a literal `]` | the class `[a]` | `[\[a]\]` |
| `\d`, `\w` and `\b` are ASCII | Unicode: `\d` matches `٣`, `\w` matches `é`, and `\bfoo` doesn't match in `éfoo` | `[0-9]`, `(?-u:\w)`, `(?-u:\b)` |
| `.` excludes `\n`, `\r`, `\u2028` and `\u2029` | `.` excludes only `\n` | `[^\n\r\u{2028}\u{2029}]` |
| `\b(?=\w)` and `(?<=\w)\b`, the start and end of a word | no look-around | `\b{start}` and `\b{end}` |
| look-ahead `(?=…)`, `(?!…)` and look-behind `(?<=…)`, `(?<!…)` | not supported | a [rewrite](../cookbook/#port-a-javascript-regex), `paths` and `exclude`, or a [syntax rule](#syntax-rules) |
| backreferences `\1`, `\k<name>` | not supported | list the alternatives ([example](../cookbook/#port-a-javascript-regex)) |
| `a{,3}` is the literal text `a{,3}` | doesn't compile | `a{0,3}`, or `a\{,3\}` for the text |

A pattern that doesn't compile exits 2 and names the rule. For a nested-class mistake, look-around and backreferences, a `hint:` line after the regex crate's message says what to write instead:

```text
rulecheck: [source/no-console-output] invalid regex: regex parse error:
    \bconsole\.(?!error\b)\w+\(
               ^^^
error: look-around, including look-ahead and look-behind, is not supported
hint: the Rust regex dialect has no look-around. Match a token edge with \b, \b{start} or \b{end}; consume the next character, as in name(?:[^_]|$), instead of a look-ahead; or move the condition into paths, exclude or a syntax selector
```

A nested class or a set operator compiles, and Rulecheck runs it with its Rust meaning, because a rule may intend exactly that. When a fixture of such a rule fails, the failure line ends with a note on how JavaScript reads the same text:

```text
src/app.ts:1:1: [source/no-and-operator] fixture 1 expected a violation at 1:1, found no violation. Replace the operator. Update the rule or fixture. This pattern uses the && set operator inside a character class, which the Rust regex dialect applies as set operations but JavaScript without the v flag reads as literal characters; to match the operator characters literally, escape each one with a backslash.
```

The note only appears with a failing fixture, and only for these class constructs in `pattern`, `requires` or an `ignore` filter. A `skipIfContent` regex gets the compile hints but no note. Add a fixture for each input that the JavaScript rule was meant to catch, so a difference shows up in `rulecheck test`, not in a missed finding.

The rewrites have limits. A negative look-behind rewrite consumes the character before the match, so the finding starts one column early. A list of banned names doesn't catch a name that the list leaves out, where a negative look-ahead would. Rulecheck doesn't offer a backtracking engine for the remaining cases, because such an engine can take exponential time on crafted input.

## Structured YAML rules

A rule with `kind: "structured"` takes a `structure` instead of a `pattern`. Rulecheck reads each `.yml` or `.yaml` file in scope as YAML, walks an ordered `path` through every document, and applies one `assert`. Nothing in the file is executed, and aliases, merge keys and tags are not expanded. Fixtures need `content`, as text fixtures do. The cookbook has tested recipes for [a key every job must declare](../cookbook/#a-key-every-entry-must-declare) and [a forbidden value](../cookbook/#a-forbidden-yaml-value).

```ts
structure: {
  format: "yaml",
  path: [{ key: "jobs" }, { each: "mapping" }],
  assert: { kind: "has-key", key: "timeout-minutes" },
}
```

- `format` is `"yaml"`. Any other file in scope can't be judged, so scope structured rules to `.yml` and `.yaml` files.
- `path` lists up to 16 segments, applied in order. `{ key: "jobs" }` selects the value of the mapping key whose text is `jobs`. `{ index: 0 }` selects a zero-based sequence item. `{ each: "mapping" }` selects every value of a mapping and `{ each: "sequence" }` every item of a sequence. A segment that doesn't fit the node, such as a missing key, an index past the end, or `key` on a scalar or a sequence, selects nothing there.
- `assert` is one of:
  - `{ kind: "present" }`: report a document where the path selects nothing, at the deepest node the path reached. A file with no documents, such as an empty file or one with only comments, is reported at 1:1.
  - `{ kind: "absent" }`: report each selected node, at the key that selected it.
  - `{ kind: "equals", value }`: report each selected node that isn't a scalar with that text.
  - `{ kind: "not-equals", value }`: report each selected scalar with that text.
  - `{ kind: "has-key", key }`: report each selected node that isn't a mapping with that key, at the key or item that selected it. With `each`, this checks every entry, for example that every job or every step declares a key.

`present` and `absent` need at least one segment. `value` and `key` are up to 1024 bytes without control characters.

Matching follows these rules:

- **Missing paths.** Only `present` reports a missing path. `absent`, `equals`, `not-equals` and `has-key` report nothing when the path selects nothing, so pair them with a `present` rule when the key is required.
- **Multiple documents.** Each document of a `---`-separated file is checked on its own, and findings from every document are reported.
- **Scalars.** Keys and values compare by their text after unquoting. Quoting style and tags are ignored and no type resolution happens, so `true`, `"true"` and `!!str true` all have the text `true`, and `on` stays the key `on`. A key that is itself a mapping or a sequence never matches a `key` segment.
- **Locations.** A finding has a one-based line and Unicode scalar column and the byte range of the node it points at, quotes included. Findings in a file come in ascending byte order, and each failing node is one finding, so `check --all-matches` and baselines count them and fixtures can use `expectAll`.

A file in scope that Rulecheck can't judge makes `check` exit 2 and name the rule, file and position, in the working tree and with `--commit` alike, and `test` reports such a fixture as one that cannot be judged. That happens when:

- the file isn't valid YAML, including tabs used for indentation and invalid UTF-8;
- any mapping in the file has a duplicate key, because tools disagree on which value wins;
- the path reaches an alias (`*name`) or looks up a key in a mapping with a `<<` merge key, because Rulecheck doesn't expand them. An alias elsewhere in the file is fine;
- the file nests deeper than 128 levels or has more than 200,000 nodes.

Files are read up to 1 MiB and count toward `--text-budget`. Parsing is a single pass over the parser's events, without recursion, and each path segment visits each selected node once. Rulecheck pins saphyr-parser 0.4.1 exactly, because a parser update can change how a file reads.

Rulecheck releases from before structured rules reject a policy that uses `kind: "structured"` (exit 2). Update every evaluator that checks the repository, including the trusted base tools in CI, before you add one. JSON, TOML and HCL formats are not available yet.

## Type-check rules in your repository

In the Rulecheck checkout, `npm run typecheck` checks the bundled rules and examples. In your own repository, install the SDK and TypeScript:

```bash
npm install -D @pmdroid/rulecheck typescript
```

Add `.rulecheck/rules/**/*.rule.ts` to your TypeScript project's `include` and run
`npx tsc --noEmit`. No path alias or Rulecheck source checkout is needed for type checking.
Use a Rulecheck CLI version that recognizes the new package name before updating existing rules.

A typo such as `col` instead of `column` then fails with `TS2353` at the rule file and line. Without the type check, `rulecheck validate` still exits 2, but the error points at a position in the generated policy JSON.

Rule files import `defineRule` from `@pmdroid/rulecheck` and export one `defineRule({...})` declaration. The builder still accepts the legacy `@rulecheck/sdk` marker for existing rules. The builder accepts literals, arrays, objects with explicit keys, and references to earlier literal constants. It does not execute TypeScript modules. `rulecheck` pipes the versioned JSON from the builder to the evaluator; there is no generated policy file to commit.

To retire a rule, remove the rule source in a reviewed change and document what replaced it or why it no longer applies. The shadow workflow still checks the candidate against base policy, even when proposed policy removes a rule. Retire an obsolete rule separately rather than weakening it to get another change through.
