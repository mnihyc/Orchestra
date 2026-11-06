/** CI serves the single verified production artifact; all ordinary test policies are inherited. */
import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { defineConfig } from '@playwright/test';
import base from '../playwright.config.ts';

const frontend = fileURLToPath(new URL('../', import.meta.url));
const manifest = readFileSync(new URL('../../.tmp/ci-web/dist-manifest.json', import.meta.url));
const proof = JSON.parse(manifest.toString()) as { source: { head: string } };
if (process.env.ORCH_E2E_DEV === '1'
    || proof.source.head !== process.env.CI_WEB_SOURCE
    || createHash('sha256').update(manifest).digest('hex') !== process.env.CI_WEB_VERIFIED_ARTIFACT) {
  throw new Error('Run CI browser groups through ci_web.py with the matching verified artifact.');
}
if (base.workers !== 1 || base.fullyParallel !== false) throw new Error('CI requires one mutable test world per runner.');
const servers = Array.isArray(base.webServer) ? base.webServer : [base.webServer];
let builds = 0;
export default defineConfig({
  ...base,
  testDir: frontend,
  webServer: servers.filter((server) => server !== undefined).map((server) => {
    const prefix = 'npm run build && ';
    if (!server.command.startsWith(prefix)) return { ...server, cwd: frontend, reuseExistingServer: false };
    builds += 1;
    return { ...server, command: server.command.slice(prefix.length), cwd: frontend, reuseExistingServer: false };
  }),
});
if (builds !== 1) throw new Error('The production preview entrypoint changed; review its artifact boundary.');
