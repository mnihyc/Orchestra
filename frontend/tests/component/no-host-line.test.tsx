/** Where a person may use no host, the placement's one line names what fixes it: an administrator the As admin switch,
 * or Use on a host's Access once it is on; anyone else the owner of a host they see, or whom to ask. A provider's container
 * runs as its owner, so a host its owner reaches only through As admin does not count for it. */
import { screen, within } from '@testing-library/react';
import { QueryClient } from '@tanstack/react-query';
import { beforeEach, expect, it, vi } from 'vitest';
import { adminModeKey } from '@/api/adminMode';
import { PlacementFields } from '@/features/start/NewEnvironment';
import type { AdminMode, Host, HostDriver } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { person } from '../doubles/chat-rows';
import { enrollmentHost } from '../doubles/enrollment-row';
import { show, skipOverflowFallback } from './support';

skipOverflowFallback();

vi.mock('@/api/resources', async (original) => ({ ...await original<object>(),
  templatesQuery: () => ({ queryKey: ['no-host-templates'], queryFn: () => Promise.resolve([]) }),
}));

const docker: HostDriver = { name: 'docker', title: 'Docker', creates: ['environment', 'provider'], runtime_classes: ['runc'] };
function bundled(permissions: string[], adminOnly: string[] = []): Host {
  return { ...enrollmentHost('orch-node-id', true), name: 'orch-node', bundled_with_server: true, drivers: [docker],
    owner: person(7, 'pkgowner', 'Pkg Owner'),
    access: { permissions, administers: adminOnly.length > 0, through: [], admin_only: adminOnly, participating: false } };
}

function draw(kind: 'environment' | 'provider', hosts: Host[], mode?: AdminMode) {
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  if (mode !== undefined) cache.setQueryData(adminModeKey, mode);
  show(<PlacementFields kind={kind} hosts={hosts} value={undefined} onChange={vi.fn()} />, cache);
}

beforeEach(async () => { await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) it(`tells an administrator with As admin off the switch, with the switch beside it (${locale})`, async () => {
  await setLocale(locale);
  draw('provider', [], { available: true, as_admin: false });
  const line = await screen.findByText(i18n.t('hosts.noneFor.provider.admin'));
  expect(line).toBeVisible();
  expect(screen.getByRole('button', { name: i18n.t('adminReach.mode.enable') })).toBeVisible();
  expect(screen.queryByText(i18n.t('start.where.hostDefaultNoneProvider'))).toBeNull();
});

it("tells an administrator with As admin on to add themself to Use on the host's Access, and opens it", async () => {
  // As admin reaches the bundled host, but the provider's container would run as the administrator themself.
  draw('provider', [bundled(['host.view', 'host.use', 'host.manage'], ['host.view', 'host.use', 'host.manage'])], { available: true, as_admin: true });
  expect(await screen.findByText(i18n.t('hosts.noneFor.provider.adminOn'))).toBeVisible();
  const access = screen.getByRole('link', { name: i18n.t('hosts.noneFor.access', { host: 'orch-node' }) });
  expect(access).toHaveAttribute('href', expect.stringContaining('/resources/hosts/orch-node-id'));
  expect(access).toHaveAttribute('href', expect.stringContaining('tab=access'));
  expect(screen.queryByRole('button', { name: i18n.t('adminReach.mode.enable') })).toBeNull();
});

for (const locale of ['en', 'zh-CN'] as const) it(`names the owner of a host the person sees and may not use (${locale})`, async () => {
  await setLocale(locale);
  draw('environment', [bundled(['host.view'])]);
  const line = document.querySelector<HTMLElement>('[data-no-host="member"]');
  if (line === null) throw new Error('the line is drawn');
  expect(within(line).getByText('Pkg Owner')).toBeVisible();
  expect(line).toHaveTextContent('orch-node');
});

it('tells anyone else who sees no host whom to ask', () => {
  draw('provider', []);
  expect(screen.getByText(i18n.t('hosts.noneFor.provider.ask'))).toBeVisible();
  expect(screen.queryByRole('button', { name: i18n.t('adminReach.mode.enable') })).toBeNull();
});

it("keeps saying to choose the host when one the person may use is another person's", () => {
  // A provider's new container goes on another person's host only where it is named.
  draw('provider', [bundled(['host.view', 'host.use'])]);
  expect(screen.getByText(i18n.t('start.where.hostDefaultNoneProvider'))).toBeVisible();
  expect(document.querySelector('[data-no-host]')).toBeNull();
});
