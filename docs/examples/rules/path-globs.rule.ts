import { defineRule } from "@pmdroid/rulecheck";

export default defineRule({
  id: "config/no-machine-specific-files",
  kind: "forbidden-path",
  paths: ["docker-compose.override.yml", "config/**/{private,local}-[0-9]?.toml"],
  remediation: "Keep machine-specific files out of Git.",
  fixtures: [
    { path: "docker-compose.override.yml", expect: { line: 1, column: 1 } },
    { path: "config/team/local-1a.toml", expect: { line: 1, column: 1 } },
    { path: "config/private-2b.toml", expect: { line: 1, column: 1 } },
    { path: "config/safe-1a.toml", expect: null },
    { path: "config/team/local-aa.toml", expect: null },
    { path: "config/team/local-1ab.toml", expect: null },
    { path: "examples/docker-compose.override.yml", expect: null },
  ],
});
