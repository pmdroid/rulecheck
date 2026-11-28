import { defineRule } from "@pmdroid/rulecheck";

export default defineRule({
  id: "assets/manifest-files-exist",
  kind: "consistency",
  // The selected inventory: the manifest and the files it may name.
  paths: ["assets/**"],
  consistency: {
    form: "path-exists",
    // Each manifest line such as "file: icons/logo.svg" names a file next to the manifest.
    source: { name: "manifest", paths: ["assets/manifest.txt"], extract: "(?m)^file:(.*)$" },
    base: "file",
  },
  remediation: "Add the missing asset or remove its line from assets/manifest.txt.",
  fixtures: [
    // Every listed file exists: no finding. Surrounding spaces are trimmed and "./" is dropped.
    {
      files: [
        { path: "assets/manifest.txt", content: "file: icons/logo.svg\nfile:  ./fonts/body.woff2 \n" },
        { path: "assets/icons/logo.svg" },
        { path: "assets/fonts/body.woff2" },
      ],
      expectFindings: [],
    },
    // A missing file, and a reference that climbs out of the repository, are both reported.
    {
      files: [
        { path: "assets/manifest.txt", content: "file: icons/logo.svg\nfile: icons/gone.svg\nfile: ../../../etc/passwd\n" },
        { path: "assets/icons/logo.svg" },
      ],
      expectFindings: [
        { path: "assets/manifest.txt", line: 2, column: 7 },
        { path: "assets/manifest.txt", line: 3, column: 7 },
      ],
    },
  ],
});
