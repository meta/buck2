/**
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

import { themes } from 'prism-react-renderer';
import { fbContent, isInternal } from 'docusaurus-plugin-internaldocs-fb/internal';
import type { ThemeConfig as ClassicPresetConfig, Options as ClassicPresetOptions } from '@docusaurus/preset-classic';
import type { DocusaurusConfig } from '@docusaurus/types';

import { postProcessItems } from './sidebars.js';
import { redirects } from './redirects';

const lightCodeTheme = themes.github;
const darkCodeTheme = themes.dracula;

const presetOptions: ClassicPresetOptions = ({
  docs: {
    path: '../docs',
    exclude: ['developers/**'],
    sidebarPath: require.resolve('./sidebars_generated.ts'),
    async sidebarItemsGenerator({ defaultSidebarItemsGenerator, ...args }) {
      const items = await defaultSidebarItemsGenerator({
        ...args
      });
      return postProcessItems(items);
    },
  },
  theme: {
    customCss: require.resolve('./src/css/custom.css'),
  },
  internSearch: true,
  staticDocsProject: 'buck2',
});

const themeConfig: ClassicPresetConfig = ({
  docs: {
    sidebar: {
      hideable: true,
    },
  },
  navbar: {
    title: 'Buck2',
    logo: {
      alt: 'Buck2 Logo',
      src: 'img/logo.svg',
    },
    items: [
      {
        type: 'doc',
        docId: 'index',
        position: 'left',
        label: 'Docs',
      },
      {
        to: '/docs/api',
        position: 'left',
        label: 'API',
        activeBaseRegex: '/docs/api',
      },
      {
        to: '/docs/prelude/rules',
        position: 'left',
        label: 'Rules',
        activeBasePath: '/docs/prelude',
      },
      {
        href: fbContent({
          internal: 'https://www.internalfb.com/code/buck2',
          external: 'https://github.com/facebook/buck2',
        }),
        // @ts-ignore : The type signature for `fbContent` incorrectly claims it might return a `[]`
        label: fbContent({
          internal: 'CodeHub',
          external: 'GitHub',
        }),
        position: 'right',
      },
    ],
  },
  footer: {
    style: 'dark',
    links: [
      {
        title: 'Docs',
        items: [
          {
            label: 'User guide',
            to: '/docs',
          },
        ],
      },
      {
        title: 'Community',
        items: isInternal() ? [
          {
            label: 'User group',
            href: 'https://fb.workplace.com/groups/buck2users',
          },
          {
            label: 'Announcement group',
            href: 'https://fb.workplace.com/groups/buck2prototyping',
          },
        ] : [
          {
            label: 'GitHub issues',
            href: 'https://github.com/facebook/buck2/issues',
          },
          {
            label: 'Community Discord server',
            href: 'https://discord.gg/N6NtcYQXU8',
          },
        ],
      },
      {
        title: 'More',
        items: [
          {
            label: 'Code',
            href: fbContent({
              internal: 'https://www.internalfb.com/code/fbsource/fbcode/buck2/',
              external: 'https://github.com/facebook/buck2',
            }),
          },
          {
            label: 'Terms of Use',
            href: 'https://opensource.fb.com/legal/terms',
          },
          {
            label: 'Privacy Policy',
            href: 'https://opensource.fb.com/legal/privacy',
          },
        ],
      },
    ],
    copyright: `Copyright © ${new Date().getFullYear()} Meta Platforms, Inc. Built with Docusaurus.`,
  },
  prism: {
    additionalLanguages: ['bash', 'powershell', 'cpp', 'ini', 'mermaid'],
    theme: lightCodeTheme,
    darkTheme: darkCodeTheme,
  },
  algolia: fbContent({
    internal: undefined,
    external: {
      appId: '9RT0EWXQO8',
      apiKey: 'cf8a08e681e1e1d8a73a08d3f13948c7',
      indexName: 'buck2',
    }
  }),
});

const config: DocusaurusConfig = ({
  title: 'Buck2',
  url: 'https://buck2.build',
  baseUrl: '/',
  onBrokenLinks: 'throw',
  trailingSlash: true,
  onBrokenMarkdownLinks: 'warn',
  favicon: 'img/logo.png',
  organizationName: 'facebook',
  projectName: 'buck2',

  presets: [
    [
      require.resolve('docusaurus-plugin-internaldocs-fb/docusaurus-preset'),
      presetOptions,
    ],
  ],

  plugins: [
    [
      '@docusaurus/plugin-google-gtag',
      {
        trackingID: 'G-GEGGHE39PE',
        anonymizeIP: true,
      },
    ],
    [
      '@docusaurus/plugin-client-redirects',
      {
        redirects: redirects,
      },
    ],
  ],

  themeConfig,

  // @ts-ignore : Fields of this are not declared as optional, but they are
  markdown: ({
    // Use mdx for `.mdx` files and commonmark for `.md` files
    format: 'mdx',
    mermaid: true,
  }),
  themes: ['@docusaurus/theme-mermaid'],
});

module.exports = {
  config: config,
};
