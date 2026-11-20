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
import { firstWavePlugin, threadShellWorkerPlugin } from './build/first-wave';
import { staticChunkGroup } from './build/frame-chunks';
import { assetFileName, inlineAsset } from './build/fonts';
import { vocabularyPlugin } from './build/vocabulary';
import { helpContentPlugin } from './build/help-content';
import { releaseNoticesPlugin } from './build/release-notices';
import { servingProxyConfig } from './build/serving-proxy';

const upstreams = servingProxyConfig(process.env);

export default defineConfig({
  plugins: [
    vocabularyPlugin(),
    helpContentPlugin(),
    releaseNoticesPlugin(),
    // The frame is known before the verified reader selects the first vocabulary.
    firstWavePlugin(),
    threadShellWorkerPlugin(),
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
    assetsInlineLimit: inlineAsset,
    sourcemap: false,
    rolldownOptions: {
      preserveEntrySignatures: false,
      output: {
        assetFileNames: assetFileName,
        format: 'es',
        strictExecutionOrder: true,
        codeSplitting: {
          // Shared factories register once, preserving stores and export identities while each screen
          // carries the code it actually reaches. Styles remain files so Vite emits each sheet once.
          experimentalInlineCommonChunks: { maxSize: Infinity, exclude: /\.(css|scss|sass|less|styl|stylus|pcss|postcss|sss)(\?|$)/ },
          groups: [
            // What the first screen needs is fetched as one file, not in hundreds: every module the entry reaches by static
            // imports joins one group (left alone, the many lazily loaded screens cut it into hundreds of shared pieces,
            // most under a kilobyte, each a request). It is not cut into parts: parts of one group import each other, and
            // a part that ran before the one it imports from broke the page. (The page's own entry is then an empty file,
            // which Vite leaves out: the page loads the group directly.)
            { name: 'initial', tags: ['$initial'], priority: 3 },
            staticChunkGroup('application', fileURLToPath(new URL('./src/application.tsx', import.meta.url)), 2),
            // Match every static frame dependency directly, even one also used by another lazy entry.
            // The earlier phase owns shared modules once; optional viewers remain outside this closure.
            staticChunkGroup('frame', fileURLToPath(new URL('./src/shell/Shell.tsx', import.meta.url)), 1),
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
