// @ts-check
import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';
import starlightThemeRapide from 'starlight-theme-rapide';

// Project site lives at https://anistark.github.io/orion-core/
// https://astro.build/config
export default defineConfig({
  site: 'https://anistark.github.io',
  base: '/orion-core/',
  integrations: [
    starlight({
      title: 'Orion',
      description:
        'Agent harness for local LLM inference. Backend-agnostic — bring your own model runtime (llama.cpp, MLX, cloud APIs, anything).',
      plugins: [starlightThemeRapide()],
      logo: {
        // Split light/dark: white-on-dark mark for dark theme, outline mark for light.
        dark: './src/assets/orion-logo.png',
        light: './src/assets/orion-icon.png',
        alt: 'Orion',
      },
      favicon: '/favicon-64.png',
      customCss: ['./src/styles/global.css'],
      social: [
        { icon: 'github', label: 'GitHub', href: 'https://github.com/anistark/orion-core' },
      ],
      editLink: {
        baseUrl: 'https://github.com/anistark/orion-core/edit/main/docs/',
      },
      lastUpdated: true,
      head: [
        {
          tag: 'link',
          attrs: { rel: 'apple-touch-icon', href: '/orion-core/apple-touch-icon.png' },
        },
        {
          tag: 'link',
          attrs: { rel: 'preconnect', href: 'https://fonts.googleapis.com' },
        },
        {
          tag: 'link',
          attrs: { rel: 'preconnect', href: 'https://fonts.gstatic.com', crossorigin: true },
        },
        {
          tag: 'link',
          attrs: {
            rel: 'stylesheet',
            href: 'https://fonts.googleapis.com/css2?family=Hanken+Grotesk:wght@400;500;600;700&family=Space+Grotesk:wght@500;600;700&family=JetBrains+Mono:wght@400;500;600&display=swap',
          },
        },
      ],
      sidebar: [
        {
          label: 'Start here',
          items: [
            { label: 'Overview', link: '/' },
            { label: 'Getting started', slug: 'start/getting-started' },
          ],
        },
        {
          label: 'Concepts',
          items: [
            { label: 'Architecture', slug: 'concepts/architecture' },
            { label: 'Agent', slug: 'concepts/agent' },
            { label: 'Backend', slug: 'concepts/backend' },
            { label: 'Messages', slug: 'concepts/messages' },
            { label: 'Events', slug: 'concepts/events' },
            { label: 'Context & budgets', slug: 'concepts/context' },
            { label: 'Templates', slug: 'concepts/templates' },
            { label: 'Tools', slug: 'concepts/tools' },
            { label: 'Errors', slug: 'concepts/errors' },
          ],
        },
        {
          label: 'Reference',
          items: [
            { label: 'Examples', slug: 'reference/examples' },
            {
              label: 'API docs (docs.rs) ↗',
              link: 'https://docs.rs/orion-core',
              attrs: { target: '_blank', rel: 'noopener' },
            },
            {
              label: 'Crate (crates.io) ↗',
              link: 'https://crates.io/crates/orion-core',
              attrs: { target: '_blank', rel: 'noopener' },
            },
          ],
        },
      ],
    }),
  ],
});
