import { defineRule } from "@pmdroid/rulecheck";

export default defineRule({
  id: "config/no-service-overrides",
  kind: "forbidden-path",
  paths: ["services/billing/config/*.local.toml"],
  // A file that always sits next to the forbidden ones. If services/billing
  // is moved or deleted, the check exits 2 instead of passing.
  anchors: ["services/billing/config/app.toml"],
  remediation: "Keep local overrides out of Git.",
  fixtures: [
    { path: "services/billing/config/app.toml", expect: null },
    { path: "services/billing/config/dev.local.toml", expect: { line: 1, column: 1 } },
    { path: "services/payments/config/dev.local.toml", expect: null },
  ],
});
