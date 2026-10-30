import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';

export default defineConfig({
  integrations: [
    starlight({
      title: 'Documentation',
      sidebar: [
        { label: 'Documentation', items: ['index', 'agent-workflow', 'cli', 'configuration', 'distribution', 'library'] },
      ],
    }),
  ],
});
