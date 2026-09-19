import { defineConfig } from 'skills-npm'

export default defineConfig({
  // Scan installed packages; @antfu/design is a dep of the nested apps/web package
  source: 'node_modules',
  recursive: true,
  // `.agents/skills` — the agentskills.io standard dir Devin CLI reads
  agents: ['universal', 'windsurf'],
})
