import { defineRule } from "@pmdroid/rulecheck";

export default defineRule({
  id: "billing/no-float-amounts",
  kind: "text",
  paths: ["services/billing/**/*.ts"],
  tags: ["adr/0007", "domain/billing"],
  pattern: { kind: "literal", value: "parseFloat(" },
  remediation: "Parse amounts as integer cents (ADR 0007).",
  fixtures: [
    { path: "services/billing/invoice.ts", content: "const cents = Number.parseInt(text, 10);\n", expect: null },
    { path: "services/billing/invoice.ts", content: "const total = parseFloat(text);\n", expect: { line: 1, column: 15 } },
  ],
});
