/** Browsing a resource is inert; only a chosen override reaches the setting writer. */
import { cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { afterEach, expect, it, vi } from 'vitest';
import type * as Tools from '@/api/resource-tools';
import { resourceRows } from '@/api/listed';
import { McpSettingChoice } from '@/features/settings/McpChoice';
import { i18n, setLocale } from '@/i18n';
import { detailHome, detailMachineRef, detailMcp } from './resource-detail-fixtures';
import { show } from './support';

const reads = vi.hoisted(() => ({ catalogue: vi.fn<(environment: string | null) => Promise<ReturnType<typeof resourceRows<typeof detailMcp>>>>() }));
vi.mock('@/api/resource-tools', async (load) => ({ ...await load<typeof Tools>(),
  mcpChoicesQuery: (environment: string | null) => ({ queryKey: ['mcp-choices-test', environment], queryFn: () => reads.catalogue(environment), staleTime: Infinity }),
}));
vi.mock('@/features/navigation/places', () => ({ PlaceLink: ({ children }: { children: React.ReactNode }) => <span>{children}</span> }));
vi.mock('@/features/resources/machines/MachineLink', () => ({ MachineLink: ({ machine }: { machine: { name: string } }) => <span>{machine.name}</span> }));
vi.mock('@/features/access/AccessDialog', () => ({ openAccess: vi.fn() }));
vi.mock('@tanstack/react-router', async (load) => ({ ...await load<object>(), useLocation: () => ({ pathname: '/environment', searchStr: '' }), useNavigate: () => vi.fn() }));
afterEach(async () => { cleanup(); vi.clearAllMocks(); localStorage.clear(); await setLocale('en'); });

function Field({ changed, use = true, initial = {} }: { changed: (value: unknown) => void; use?: boolean; initial?: Record<string, boolean> }) {
  const [value, setValue] = useState(initial);
  reads.catalogue.mockResolvedValue(resourceRows([{ ...detailMcp, origin_machine: detailMachineRef,
    availability: [{ machine_id: 'executor', available: false, reason: 'its imported definition belongs to another machine' }],
    access: { ...detailMcp.access, permissions: use ? ['mcp.view', 'mcp.use'] : ['mcp.view'] } }]));
  return <McpSettingChoice label="MCP servers" shape={{ type: 'map', value: { type: 'bool' }, byId: true }} environment="executor-environment"
    value={value} effective={{}} onChange={(next) => { changed(next); setValue(next as Record<string, boolean>); }} />;
}

for (const locale of ['en', 'zh-CN'] as const) it(`keeps unavailable MCP servers named and browsing inert in ${locale}`, async () => {
  await setLocale(locale); const changed = vi.fn(); const user = userEvent.setup(); show(<Field changed={changed} />);
  expect(reads.catalogue).not.toHaveBeenCalled();
  await user.click(screen.getByRole('combobox', { name: i18n.t('resourceGroups.mcpAdd') }));
  const row = await screen.findByRole('option', { name: detailMcp.name });
  expect(row).not.toHaveAttribute('aria-disabled', 'true');
  expect(within(row).getByText(detailMachineRef.name)).toBeVisible();
  expect(within(row).getByText(i18n.t('resourceGroups.reasons.otherMachine'))).toBeVisible();
  expect(reads.catalogue).toHaveBeenCalledExactlyOnceWith('executor-environment');
  expect(changed).not.toHaveBeenCalled();
  await user.click(screen.getByRole('button', { name: i18n.t('resourceGroups.collapse', { name: detailHome.name }) }));
  expect(changed).not.toHaveBeenCalled();
  await user.click(screen.getByRole('button', { name: i18n.t('resourceGroups.expand', { name: detailHome.name }) }));
  await user.click(screen.getByRole('option', { name: detailMcp.name }));
  expect(changed).toHaveBeenCalledExactlyOnceWith({ [detailMcp.id]: false });
  const enabled = screen.getByRole('switch', { name: i18n.t('resourceGroups.mcpEnabled', { name: detailMcp.name }) });
  expect(enabled).toBeEnabled(); await user.click(enabled);
  expect(changed).toHaveBeenLastCalledWith({ [detailMcp.id]: true });
});

it('keeps a readable choice when Use is absent and lets an old enabled override be removed', async () => {
  const user = userEvent.setup(); const changed = vi.fn(); show(<Field changed={changed} use={false} initial={{ [detailMcp.id]: true }} />);
  await waitFor(() => expect(screen.getByRole('switch', { name: i18n.t('resourceGroups.mcpEnabled', { name: detailMcp.name }) })).toBeEnabled());
  const toggle = screen.getByRole('switch', { name: i18n.t('resourceGroups.mcpEnabled', { name: detailMcp.name }) });
  await user.click(toggle); expect(changed).toHaveBeenLastCalledWith({ [detailMcp.id]: false });
  expect(toggle).toBeDisabled();
  await user.click(screen.getByRole('button', { name: i18n.t('resourceGroups.mcpRemove', { name: detailMcp.name }) }));
  expect(changed).toHaveBeenLastCalledWith({});
});
