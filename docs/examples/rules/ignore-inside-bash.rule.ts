import { defineRule } from "@pmdroid/rulecheck";

export default defineRule({
  id: "scripts/no-force-push",
  kind: "text",
  paths: ["scripts/**/*.sh"],
  pattern: { kind: "regex", value: "\\bgit push (--force|-f)\\b" },
  // Skip a match that lies wholly inside a # comment or a quoted string.
  ignoreInside: { language: "bash", contexts: ["comment", "string"] },
  remediation: "Push with --force-with-lease, or open a pull request.",
  fixtures: [
    // Mentions in comments and quoted strings are not commands.
    {
      path: "scripts/release.sh",
      content: "# never git push --force here\necho 'git push -f is disabled'\n",
      expect: null,
    },
    // A skipped mention does not hide the command after it on the same line.
    {
      path: "scripts/release.sh",
      content: "echo \"git push -f\"; git push -f origin main\n",
      expect: { line: 1, column: 21 },
    },
    // A $(...) substitution inside double quotes is code, so it is reported.
    {
      path: "scripts/release.sh",
      content: "echo \"pushed: $(git push --force)\"\n",
      expect: { line: 1, column: 17 },
    },
    // A heredoc body is data, never a comment or string, so it is reported.
    {
      path: "scripts/release.sh",
      content: "ssh host <<'EOF'\ngit push --force\nEOF\n",
      expect: { line: 2, column: 1 },
    },
  ],
});
