export interface Location {
  line: number;
  column: number;
}

interface RuleFields {
  id: string;
  paths: readonly string[];
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
