/** The build of the web application, its development server and its unit tests.
 *
 * The development server sends `/api` (WebSocket upgrades included) and `/health` to the API named
 * by `ORCH_DEV_API`, so that the page and the API share an origin as they do behind the proxy of a
 * deployment: the session cookie is same-site, and no cross-origin rule is needed anywhere.
 * An optional ORCH_DEV_GATEWAY carries /link/ through the same origin in development and preview,
 * verifying its HTTPS certificate against ORCH_DEV_GATEWAY_CA, with an optional TLS server name.
 */
import { fileURLToPath } from 'node:url';

import tailwindcss from '@tailwindcss/vite';
import { tanstackRouter } from '@tanstack/router-plugin/vite';
import react from '@vitejs/plugin-react';
import { defineConfig } from 'vitest/config';
import { helpContentPlugin } from './build/help-content';
import { releaseNoticesPlugin } from './build/release-notices';
import { servingProxyConfig } from './build/serving-proxy';

const upstreams = servingProxyConfig(process.env);

export default defineConfig({
  plugins: [
    helpContentPlugin(),
    releaseNoticesPlugin(),
    // The route tree is generated from the files under `src/routes`; every route is its own chunk.
    tanstackRouter({
      target: 'react',
      autoCodeSplitting: true,
      routesDirectory: 'src/routes',
      generatedRouteTree: 'src/routeTree.gen.ts',
      // The unknown-route screen lives beside the routes and is no route itself.
      routeFileIgnorePattern: '^not-found\\.tsx$',
      quoteStyle: 'single',
      semicolons: true,
    }),
    react(),
    tailwindcss(),
  ],
  resolve: {
    alias: { '@': fileURLToPath(new URL('./src', import.meta.url)) },
  },
  // Libraries that only a screen loaded late uses are prepared when the server starts: found while a
  // page is open, they make the server prepare them then and reload the page under its reader.
  optimizeDeps: {
    // This package's worker and wasm URLs are relative to its module; Vite must
    // transform them before prebundling could move that module into .vite/deps.
    exclude: ['@vedivad/typst-web-service'],
    include: [
      'marked',
      'pdfjs-dist',
      '@tanstack/react-virtual',
      // The charts of Management and of usage draw with uPlot, fetched with the screen that has them.
      'uplot',
      // The code view and the terminal are fetched when first used; prepared with the rest, they do not reload the page under its reader.
      '@codemirror/state',
      '@codemirror/view',
      '@codemirror/commands',
      '@codemirror/language',
      '@codemirror/search',
      '@lezer/highlight',
      '@xterm/xterm',
      '@xterm/addon-fit',
    ],
  },
  server: {
    host: '127.0.0.1',
    // Vite checks both the file and the queried module id for raw/URL imports. Keep the three
    // single-file Help assets and their actual import ids allowed without exposing their siblings.
    fs: { allow: ['.', '../docs/user', '../docs/development/extensions/providers',
      '../docs/development/extensions/adapters.md', '../docs/development/extensions/adapters.md?raw',
      '../crates/contracts/agent/README.md', '../crates/contracts/agent/README.md?url&no-inline',
      '../crates/contracts/agent-programs/README.md', '../crates/contracts/agent-programs/README.md?url&no-inline', '../shared'] },
    ...upstreams.server,
  },
  preview: {
    host: '127.0.0.1',
    ...upstreams.preview,
  },
  build: {
    // Everything but the first page is named by its content, under `/assets/`, and kept by the browser.
    assetsDir: 'assets',
    sourcemap: false,
    rolldownOptions: {
      output: {
        codeSplitting: {
          groups: [
            // What the first screen needs is fetched as one file, not in hundreds: every module the entry reaches by static
            // imports joins one group (left alone, the many lazily loaded screens cut it into hundreds of shared pieces,
            // most under a kilobyte, each a request). It is not cut into parts: parts of one group import each other, and
            // a part that ran before the one it imports from broke the page. (The page's own entry is then an empty file,
            // which Vite leaves out: the page loads the group directly.)
            { name: 'initial', tags: ['$initial'] },
            // The workbench's frame, which every signed-in screen stands in, likewise comes as one file with what it needs beyond
            // the first (its parts would otherwise come in a hundred small files). The screens before sign-in share none of it.
            { name: 'shell', test: /[\\/]src[\\/]shell[\\/]Shell\.tsx$/ },
          ],
        },
      },
    },
  },
  test: {
    environment: 'jsdom',
    // Full editor and overlay trees need bounded CPU and memory while independent files run.
    // Four isolated workers keep that work parallel; callers can choose another worker count.
    maxWorkers: 4,
    include: ['tests/unit/**/*.test.{ts,tsx}', 'tests/component/**/*.test.tsx'],
    setupFiles: ['tests/setup.ts'],
    restoreMocks: true,
    unstubGlobals: true,
  },
});
