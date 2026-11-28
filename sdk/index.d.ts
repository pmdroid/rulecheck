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

interface RuleFields {
  id: string;
  /** Positive, case-sensitive repository-relative globs. A file is in scope when any entry matches. */
  paths: readonly string[];
  /** Optional globs removed from `paths`. A file matching any entry is out of scope for checks and fixtures. */
  exclude?: readonly string[];
  remediation: string;
  /**
   * Optional link to the review finding that motivated the rule: an https:// URL of at most
   * 2048 characters. Matching ignores it and findings don't show it. Rulecheck releases
   * before provenance support reject policies that use it.
   */
  provenance?: { finding: string };
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
      fixtures: readonly TextFixture[];
    }
  | {
      kind: "syntax";
      selector: Selector;
      fixtures: readonly { path: string; content: string; expect: Location | null }[];
    }
  | {
      kind: "forbidden-path";
      fixtures: readonly { path: string; expect: Location | null }[];
    }
);

export declare function defineRule(rule: Rule): Rule;
