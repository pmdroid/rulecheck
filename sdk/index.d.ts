export interface Location {
  line: number;
  column: number;
}

interface RuleFields {
  id: string;
  /** Positive, case-sensitive repository-relative globs. A file is in scope when any entry matches. */
  paths: readonly string[];
  /** Optional globs removed from `paths`. A file matching any entry is out of scope for checks and fixtures. */
  exclude?: readonly string[];
  remediation: string;
}

export type Rule = RuleFields & (
  | {
      kind: "text";
      pattern: { kind: "literal" | "regex"; value: string };
      fixtures: readonly { path: string; content: string; expect: Location | null }[];
    }
  | {
      kind: "forbidden-path";
      fixtures: readonly { path: string; expect: Location | null }[];
    }
);

export declare function defineRule(rule: Rule): Rule;
