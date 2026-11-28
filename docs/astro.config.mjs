import { defineConfig } from "astro/config";
import starlight from "@astrojs/starlight";

export default defineConfig({
  output: "static",
  site: process.env.RULECHECK_DOCS_SITE || undefined,
  base: process.env.RULECHECK_DOCS_BASE || "/",
  integrations: [
    starlight({
      title: "Rulecheck",
      social: [{ icon: "github", label: "GitHub", href: "https://github.com/pmdroid/rulecheck" }],
      sidebar: [
        { label: "Overview", slug: "" },
        { label: "Quickstart", slug: "quickstart" },
        { label: "Install from release archives", slug: "install" },
        { label: "Authoring rules", slug: "authoring" },
        { label: "Rule cookbook", slug: "cookbook" },
        { label: "CLI reference", slug: "cli" },
        { label: "Baselines", slug: "baselines" },
        { label: "CI and Paws", slug: "ci" },
      ],
    }),
  ],
});
