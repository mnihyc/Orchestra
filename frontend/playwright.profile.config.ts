/** Local source maps let the hosted profiler report which application functions blocked a frame. */
import { defineConfig } from '@playwright/test';
import base from './playwright.config';

const servers = Array.isArray(base.webServer) ? base.webServer : [];
export default defineConfig({
  ...base,
  webServer: servers.map((server) => ({
    ...server,
    command: server.command.replace('npm run build &&', 'npm run build -- --sourcemap hidden &&'),
  })),
});
