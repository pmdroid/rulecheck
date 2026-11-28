import { defineGlobSet } from "@pmdroid/rulecheck";

export default defineGlobSet({
  version: 1,
  globs: ["**/__generated__/**", "**/*.pb.ts"],
  include: ["third-party"],
});
