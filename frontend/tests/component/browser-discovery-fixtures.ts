/** Native browser projections for cold-list, missing-program and retained Tools navigation proofs. */
import type { Browser, Environment, EnvironmentBrowsers, EnvironmentDisplays } from '@/generated/api-types';
import { detailEnvironment } from './resource-detail-fixtures';

export const browserEnvironment: Environment = { ...detailEnvironment, browser: true, vnc: true, access: { ...detailEnvironment.access,
  permissions: ['environment.view', 'environment.manage', 'browser.view', 'browser.manage', 'ports.view', 'vnc.view'] } };

export function browserSnapshot(): EnvironmentBrowsers {
  return {
    off: null,
    support: { programs: [{ name: 'chromium', path: '/usr/bin/chromium', version: '131' }], window: false, playwright_cli: null, sandbox: null },
    presets: [{ name: 'headless', available: true, reason: null }, { name: 'window', available: false, reason: 'No display' }],
    profiles: [], profiles_dir: null, browsers: [], limit: { open: 0, max: 4 },
  };
}

export function displaySnapshot(): EnvironmentDisplays {
  return { off: null, support: { servers: [], window_manager: null }, displays: [], limit: { open: 0, max: 4 } };
}

export function runningBrowser(): Browser {
  const environment = browserEnvironment;
  return { id: 'browser', environment: { id: environment.id, name: environment.name, address: environment.address, owner: environment.owner,
    placement: environment.placement, status: environment.status, ports_exposed: environment.ports_exposed },
    profile: 'work', origin: 'started', opened_by: environment.owner, thread: null, preset: 'headless', display: null,
    endpoint: 'http://127.0.0.1:41873', program: 'Chromium', tabs: 1, note: null, watching: 0, driving: [], state: 'open',
    close_reason: null, opened_at: '2026-10-16T00:00:00Z', last_used_at: '2026-10-16T00:00:00Z', closed_at: null };
}
