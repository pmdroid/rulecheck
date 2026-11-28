export interface Location {
  line: number;
  column: number;
}

/**
 * Selects nodes of one tree-sitter node kind. "typescript" parses .ts, .mts and .cts files
 * with the TypeScript grammar and .tsx files with the TSX grammar. "rust" parses .rs files,
 * "go" parses .go files and "python" parses .py and .pyi files with that language's grammar.
 * Names are the grammar's named node kinds and fields, checked when the policy loads. Rust has
 * no single comment kind: use "line_comment" or "block_comment".
 */
export interface Selector {
  language: "typescript" | "rust" | "go" | "python";
  /** Named node kind to report, such as "comment". */
  node: string;
  /**
   * Keep only nodes with an ancestor of one of `kinds`. With `field`, the node must also sit
   * within that ancestor's `field` child, such as the `body` of a function.
   */
  inside?: { kinds: readonly string[]; field?: string };
  /** Drop a node whose source text contains a literal or matches a Rust regex. */
  ignore?: readonly { kind: "literal" | "regex"; value: string }[];
}

/**
 * Skips an in-scope file whose first `firstBytes` bytes (1 to 65536) contain a literal or match
 * a Rust regex, such as a generated-file header. It runs after `paths`, `exclude` and
 * `excludeSets`, in checks and fixtures alike. A file Rulecheck cannot read is never skipped.
 */
export interface SkipIfContent {
  kind: "literal" | "regex";
  value: string;
  firstBytes: number;
}

/**
 * Skips a text rule's match that lies wholly inside one of `contexts`, as parsed by a pinned
 * grammar. "typescript" parses .ts, .mts and .cts files, and .tsx with the TSX grammar: "comment"
 * covers line and block comments, and "string" covers quoted strings and the literal parts of
 * template strings, not their `${...}` substitutions. "bash" parses .sh and .bash files:
 * "comment" covers `#` comments, and "string" covers `'...'`, `$'...'` and the literal parts of
 * `"..."`, not the expansions or `$(...)` inside. Heredoc bodies are never skipped. Each match is
 * judged on its own, so a skipped one never hides a later match. When the pattern matches in a
 * file the grammar does not cover or cannot parse, or a Bash file with a construct the grammar
 * reads differently from bash, the match cannot be classified and the check exits 2.
 */
export interface IgnoreInside {
  language: "typescript" | "bash";
  contexts: readonly ("comment" | "string")[];
}

/**
 * A named, reusable list of exclusion globs in `.rulecheck/sets/<name>.set.ts`. Rules refer to
 * it with `excludeSets: ["<name>"]`. Rulecheck reads it as static data from the same worktree or
 * commit as the rules; nothing in it runs. `include` adds other sets' globs.
 */
export interface GlobSet {
  version: 1;
  globs: readonly string[];
  include?: readonly string[];
}

/**
 * One step of a structure path. `key` selects a mapping value by its key text, `index` a
 * zero-based sequence item, and `each` every value of a mapping or every item of a sequence.
 * A step that does not fit the node selects nothing.
 */
export type StructurePathSegment =
  | { key: string }
  | { index: number }
  | { each: "mapping" | "sequence" };

/**
 * What a structured rule asserts about the nodes its path selects, in each YAML document.
 * "present" reports a document where the path selects nothing. "absent", "equals",
 * "not-equals" and "has-key" report every selected node that breaks the assertion, so a
 * missing path reports nothing for them. Scalars compare by their text after unquoting.
 */
export type StructureAssert =
  | { kind: "present" }
  | { kind: "absent" }
  | { kind: "equals"; value: string }
  | { kind: "not-equals"; value: string }
  | { kind: "has-key"; key: string };

/**
 * Reads .yml and .yaml files as YAML without executing or expanding anything. A file that
 * does not parse or has a duplicate key, and a path that reaches an alias or a `<<` merge
 * key, cannot be judged.
 */
export interface Structure {
  format: "yaml";
  path: readonly StructurePathSegment[];
  assert: StructureAssert;
}

/**
 * One named scope of a consistency rule: files inside the rule's `paths` that also match these
 * `paths` and no `exclude` glob. `extract` is a Rust regex with exactly one capture group around
 * the value; use `(?m)` for `^` and `$` at line boundaries. Values are trimmed of surrounding
 * whitespace, empty values are dropped and a repeated value counts once.
 */
export interface ConsistencyScope {
  /** Lowercase letters, digits, hyphens and slashes; shown in findings. */
  name: string;
  paths: readonly string[];
  exclude?: readonly string[];
  extract: string;
}

/**
 * `same-values` reports each value found in one scope but not the other. `path-exists` reports
 * each value of `source` that does not resolve to a regular file of the rule's inventory (its
 * `paths`, `exclude` and `excludeSets`). References resolve lexically from the repository root
 * (`base: "root"`) or the referencing file's directory (`base: "file"`); one that is absolute
 * or climbs above the root is reported as escaping the repository.
 */
export type Consistency =
  | { form: "same-values"; left: ConsistencyScope; right: ConsistencyScope }
  | { form: "path-exists"; source: ConsistencyScope; base: "root" | "file" };

/** A consistency fixture: a synthetic inventory and every finding the rule reports on it. */
export interface ConsistencyFixture {
  files: readonly { path: string; content?: string }[];
  /** Ordered by path, then line and column. An empty list expects none. */
  expectFindings: readonly { path: string; line: number; column: number }[];
}

interface RuleFields {
  id: string;
  /** Positive, case-sensitive repository-relative globs. A file is in scope when any entry matches. */
  paths: readonly string[];
  /** Optional globs removed from `paths`. A file matching any entry is out of scope for checks and fixtures. */
  exclude?: readonly string[];
  /** Optional names of sets in .rulecheck/sets whose globs are added to `exclude`. */
  excludeSets?: readonly string[];
  remediation: string;
  /**
   * Optional link to the review finding that motivated the rule: an https:// URL of at most
   * 2048 characters. Matching ignores it and findings don't show it. Rulecheck releases
   * before provenance support reject policies that use it.
   */
  provenance?: { finding: string };
  /**
   * Optional stable labels for coverage reports, such as an architecture decision record
   * ("adr/0007") or a domain ("domain/billing"): 1 to 16 distinct tags of at most 64
   * lowercase letters, digits, hyphens and slashes, shaped like a rule ID. Matching,
   * fixtures and findings ignore them; `rulecheck list --format json` reports them sorted.
   * Rulecheck releases before tag support reject policies that use them.
   */
  tags?: readonly string[];
}

/**
 * A text fixture asserts either the first match with `expect` (null for none), or every match
 * with `expectAll`: the locations that `rulecheck check --all-matches` reports for the content,
 * in ascending order. An empty `expectAll` expects no match.
 */
export type TextFixture = { path: string; content: string } & (
  | { expect: Location | null; expectAll?: never }
  | { expectAll: readonly Location[]; expect?: never }
);

export type Rule = RuleFields & (
  | {
      kind: "text";
      pattern: { kind: "literal" | "regex"; value: string };
      /**
       * Makes the rule conditional: a file where `pattern` matches must also contain this text
       * somewhere. Findings point at the `pattern` matches, and a file that matches `requires`
       * anywhere reports none of them. A regex that can match empty text is refused.
       */
      requires?: { kind: "literal" | "regex"; value: string };
      ignoreInside?: IgnoreInside;
      skipIfContent?: SkipIfContent;
      /**
       * Optional per-finding detail for a regex pattern: plain text with `{name}` references to
       * the pattern's named groups, such as `"{key} is set to {value}"`, and `{{` or `}}` for a
       * literal brace. Every reference must name a group. Findings report the rendered text in
       * `message` beside the fixed `remediation`; captured text is never evaluated.
       */
      message?: string;
      fixtures: readonly TextFixture[];
    }
  | {
      kind: "syntax";
      selector: Selector;
      skipIfContent?: SkipIfContent;
      fixtures: readonly { path: string; content: string; expect: Location | null }[];
    }
  | {
      kind: "structured";
      structure: Structure;
      fixtures: readonly TextFixture[];
    }
  | {
      kind: "forbidden-path";
      /**
       * Optional globs for files that show the guarded area exists, such as a file that always
       * sits next to the forbidden one. When no file matches any anchor, the check exits 2
       * instead of passing, because the area moved or vanished. Without anchors, no matching
       * forbidden path is a pass. An anchor must not match a fixture that expects a finding.
       */
      anchors?: readonly string[];
      skipIfContent?: never;
      fixtures: readonly { path: string; expect: Location | null }[];
    }
  | {
      kind: "consistency";
      consistency: Consistency;
      skipIfContent?: never;
      fixtures: readonly ConsistencyFixture[];
    }
);

export declare function defineRule(rule: Rule): Rule;
export declare function defineGlobSet(set: GlobSet): GlobSet;
