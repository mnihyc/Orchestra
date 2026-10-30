/** Ordinary resource facts have names in both languages, including the machine's port policy. */
import { screen, waitFor, within } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import userEvent from '@testing-library/user-event';
import { ResourceAccess, resourcePermission } from '@/features/resources/ResourceAccess';
import { MachineRoleChips } from '@/features/resources/machines/EnrollmentRoles';
import { MachinePolicyTab, policyCommand } from '@/features/resources/machines/Policy';
import { PortPolicyValue } from '@/features/resources/machines/PortPolicyValue';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

const fixture = vi.hoisted(() => ({
  policy: { allowed: ['place', 'host', 'link'], publish: { incoming_address: 'build.example.test', listen: 'all', ranges: [{ from: 41000, to: 41015 }] }, limits: null },
}));
vi.mock('@/features/access/AccessDialog', () => ({ openAccess: vi.fn() }));
vi.mock('@/features/help/open', () => ({ useHelpOpener: () => vi.fn() }));
vi.mock('@/api/access', () => ({ permissionsQuery: {
  queryKey: ['screen-names-permissions'],
  queryFn: () => Promise.resolve({ permissions: [{ name: 'future.use', label: 'Use report viewer' }] }),
} }));
vi.mock('@/api/resources', () => ({
  machinePolicyQuery: () => ({ queryKey: ['screen-names-policy'], queryFn: () => Promise.resolve({ machine: { id: 'machine', name: 'Build machine', online: true, platform_os: 'linux', placement: 'connected' }, command: 'orch-node policy', reported_at: null, keys: [{
    key: 'ports', value: fixture.policy, bounds: 'machine', writable: false, delegated: false,
    definition: { key: 'ports', title: 'Ports', meaning: 'The machine port ceiling.', sections: ['node'], delegable: true,
      managed: fixture.policy, cautious: { allowed: [], publish: null, limits: null } },
  }] }) }),
  updateMachinePolicy: vi.fn(),
}));

afterEach(async () => { await setLocale('en'); });

it.each(['en', 'zh-CN'] as const)('names mount, tool-server and machine permissions in %s', async (locale) => {
  await setLocale(locale);
  const permissions = ['mount.view', 'mount.use', 'mount.manage', 'mcp.view', 'mcp.use', 'mcp.manage', 'machine.view', 'machine.use', 'machine.manage'];
  show(<ResourceAccess target={{ kind: 'mount', id: 'mount', name: 'Notes' }}
    access={{ permissions, participating: true, admin_only: [], administers: false, through: [] }} />);
  const held = screen.getByRole('list', { name: i18n.t('resources.access.hold') });
  for (const permission of permissions) {
    const label = resourcePermission(i18n.t, permission);
    expect(label).not.toBeNull();
    expect(within(held).getByText(label ?? '')).toBeVisible();
    expect(held).not.toHaveTextContent(permission);
  }
});

it('uses a future permission’s supplied catalogue name and leaves an unreported name unknown', async () => {
  expect(resourcePermission(i18n.t, 'future.missing')).toBeNull();
  show(<ResourceAccess target={{ kind: 'mount', id: 'mount', name: 'Notes' }}
    access={{ permissions: ['future.use', 'future.missing'], participating: true, admin_only: [], administers: false, through: [] }} />);
  const held = screen.getByRole('list', { name: i18n.t('resources.access.hold') });
  await waitFor(() => expect(within(held).getByText('Use report viewer')).toBeVisible());
  expect(within(held).getByText(i18n.t('resourceAccessWords.unavailable'))).toBeVisible();
  expect(held).not.toHaveTextContent('future.use');
  expect(held).not.toHaveTextContent('future.missing');
});

it.each(['en', 'zh-CN'] as const)('uses the enrollment role names on chips in %s', async (locale) => {
  await setLocale(locale);
  const roles = ['environment', 'node', 'agent', 'provider'] as const;
  const view = show(<MachineRoleChips roles={roles} />);
  for (const role of roles) {
    expect(screen.getByText(i18n.t(`machines.add.roleChoices.${role}.name`))).toBeVisible();
    expect(within(view.container).queryByText(role)).not.toBeInTheDocument();
  }
});

it.each(['en', 'zh-CN'] as const)('reads port audiences and listener facts as words in %s while retaining the exact command', async (locale) => {
  await setLocale(locale);
  const view = show(<MachinePolicyTab machine={{ id: 'machine' }} />);
  const section = await screen.findByRole('region', { name: i18n.t('machines.policy.sections.node.name') });
  const portName = i18n.t('machines.policy.keys.ports.title');
  await userEvent.click(within(section).getByRole('button', { name: i18n.t('machines.policy.viewTitle', { key: portName }) }));
  const values = view.container.querySelectorAll<HTMLElement>('[data-policy-port-value]');
  expect(values).toHaveLength(1);
  const current = values.item(0);
  expect(current).toBeInstanceOf(HTMLElement);
  for (const kind of ['place', 'host', 'link'] as const) {
    expect(within(current).getByText(i18n.t(`choiceCatalog.portAudience.${kind}.name`))).toBeVisible();
    expect(current).not.toHaveTextContent(`"${kind}"`);
  }
  expect(current).toHaveTextContent('build.example.test');
  expect(current).toHaveTextContent('41000–41015');
  expect(current).toHaveTextContent(i18n.t('choiceCatalog.hostBinding.all.name'));
  expect(current).not.toHaveTextContent('"allowed"');
  expect(current).not.toHaveTextContent('"incoming_address"');
  await userEvent.click(within(section).getByRole('button', { name: i18n.t('help.hint.label', { term: portName }) }));
  const help = await screen.findByRole('dialog', { name: portName });
  expect(help.querySelectorAll('[data-policy-port-value]')).toHaveLength(2);
  const exact = `orch-node policy set ports '${JSON.stringify(fixture.policy)}'`;
  expect(policyCommand('ports', fixture.policy, 'orch-node')).toBe(exact);
  expect(screen.getByText(exact)).toBeVisible();
});

it.each(['en', 'zh-CN'] as const)('keeps a declared local listener limit readable in %s', async (locale) => {
  await setLocale(locale);
  const view = show(<PortPolicyValue value={{ allowed: ['host'], publish: null,
    limits: { incoming_address: 'gateway.example.test', listen: '127.0.0.1', ranges: [{ from: 41010, to: 41010 }], revision: 3 } }} />);
  expect(screen.getByText(i18n.t('choiceCatalog.portAudience.host.name'))).toBeVisible();
  expect(screen.getByText('gateway.example.test')).toBeVisible();
  expect(screen.getByText('127.0.0.1')).toBeVisible();
  expect(screen.getByText('41010')).toBeVisible();
  expect(view.container).not.toHaveTextContent('"ranges"');
  expect(view.container).not.toHaveTextContent(i18n.t('machinePortValue.noLimits'));
});

it('does not invent an unrestricted policy from a missing or unrecognized value', () => {
  const view = show(<PortPolicyValue value={{ allowed: ['future-audience'] }} />);
  expect(screen.getByText(i18n.t('machinePortValue.unavailable'))).toBeVisible();
  expect(view.container).not.toHaveTextContent('future-audience');
  expect(view.container).not.toHaveTextContent(i18n.t('machinePortValue.noLimits'));
  view.rerender(<PortPolicyValue value={null} />);
  expect(screen.getByText(i18n.t('machinePortValue.unavailable'))).toBeVisible();
});


it.each([
  ['en', 'Owns the machine this environment runs on'],
  ['zh-CN', '拥有此环境所在的机器'],
] as const)('explains physical machine ownership without claiming environment ownership in %s', async (locale, explanation) => {
  await setLocale(locale);
  const view = show(<ResourceAccess target={{ kind: 'environment', id: 'environment', name: 'Shared workspace' }}
    access={{ permissions: ['environment.manage'], participating: false, admin_only: [], administers: false,
      through: [{ kind: 'source_owner' }] }} />);
  expect(screen.getByText(explanation, { exact: true })).toBeVisible();
  expect(view.container).not.toHaveTextContent('access.sourceOwner');
  expect(screen.queryByText(i18n.t('resources.access.through.owner'), { exact: true })).not.toBeInTheDocument();
  expect(screen.queryByText(i18n.t('resources.access.administers'), { exact: true })).not.toBeInTheDocument();
});
