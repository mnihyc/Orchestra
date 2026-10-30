/** One View as mechanism for every permission-page kind, preserving native provenance and recoverable failures. */
import { cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ReactNode } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';

import type * as AccessApi from '@/api/access';
import type { AccessKind, AccessTarget } from '@/api/access';
import { ApiError } from '@/api/client';
import { AccessDialog } from '@/features/access/AccessDialog';
import { ViewAs } from '@/features/access/ViewAs';
import type { Candidates } from '@/features/access/candidates';
import type { EffectiveAccess } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { apiErrorSentence } from '@/i18n/errors';

import { BEA, CATALOGUE, TOPIC, pageFor } from '../unit/general-access-fixtures';
import { show } from './support';

const calls = vi.hoisted(() => ({ effective: vi.fn<(target: AccessTarget, user: string) => Promise<EffectiveAccess>>() }));
vi.mock('@/api/access', async (load) => ({ ...await load<typeof AccessApi>(),
  pageQuery: (target: AccessTarget) => ({ queryKey: ['view-as-page', target.kind, target.id], retry: false,
    queryFn: () => Promise.resolve(pageFor(target.kind)) }),
  permissionsQuery: { queryKey: ['view-as-permissions'], queryFn: () => Promise.resolve(CATALOGUE.reply) },
  effectiveQuery: (target: AccessTarget, user: string) => ({ queryKey: ['page-effective', target.kind, target.id, user], retry: false,
    queryFn: () => calls.effective(target, user) }),
}));
vi.mock('@/features/access/candidates', () => ({ useTargetOrg: () => null,
  useCandidates: (): Candidates => ({ groups: [{ id: 'people', label: 'People', options: [{ value: BEA.id, label: BEA.display_name, text: BEA.display_name }] }],
    byValue: new Map([[BEA.id, { input: { kind: 'user', id: BEA.id }, holder: { kind: 'user', user: BEA }, user: BEA, name: BEA.display_name, outside: false }]]),
    loading: false, placePicker: null }),
}));
vi.mock('@tanstack/react-router', () => ({ Link: ({ children }: { children: ReactNode }) => <a href="#place">{children}</a> }));
vi.mock('@/features/help/open', () => ({ useHelpOpener: () => vi.fn() }));
vi.mock('@/features/help/Hint', () => ({ Hint: () => null }));
vi.mock('@/features/access/ByArea', () => ({ ByArea: () => <p>Full permissions</p> }));
vi.mock('@/features/navigation/placeLabels', () => ({ useNodeLabel: (_id: string, name: string) => name,
  usePlaceLabel: () => (place: { name: string | null } | null) => place?.name ?? null, useKnownPlaces: () => new Map() }));

const kinds: Exclude<AccessKind, 'chat'>[] = ['org', 'project', 'topic', 'thread', 'document', 'provider', 'agent', 'environment', 'host', 'machine', 'template', 'mcp_server', 'mount'];
function result(kind: AccessKind): EffectiveAccess {
  // This recorded catalogue predates the machine page; its native view permission is machine.view.
  const permission = kind === 'machine' ? 'machine.view' : CATALOGUE.of(kind).find((entry) => entry.name.endsWith('.view'))?.name;
  if (permission === undefined) throw new Error(`The fixture lacks the native view permission of ${kind}`);
  return { user: BEA, role: null, role_from: null, administers: false, admin_only: [], permissions: [{ permission, through: [{ kind: 'owner' }] }] };
}
async function choose() {
  await userEvent.type(screen.getByRole('combobox', { name: i18n.t('access.viewAs.person') }), BEA.display_name);
  await userEvent.click(await screen.findByRole('option', { name: BEA.display_name }));
}
beforeEach(() => { calls.effective.mockReset(); calls.effective.mockImplementation((target) => Promise.resolve(result(target.kind))); });
afterEach(async () => { cleanup(); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  it.each(kinds)(`reads %s after choosing a person, through the same request (${locale}, ${theme})`, async (kind) => {
    await setLocale(locale);
    const target: AccessTarget = { kind, id: `selected-${kind}`, name: 'Shared work', orgId: 'org' };
    show(<div data-theme={theme}><ViewAs target={target} open onClose={vi.fn()} /></div>);
    expect(calls.effective).not.toHaveBeenCalled();
    await choose();
    await screen.findByRole('list', { name: i18n.t('access.viewAs.list') });
    expect(calls.effective).toHaveBeenCalledTimes(1);
    expect(calls.effective).toHaveBeenCalledWith(target, BEA.id);
    expect(screen.getByText(i18n.t('access.viewAs.owner'), { exact: true })).toBeVisible();
    expect(screen.getByRole('combobox', { name: i18n.t('access.viewAs.person') })).toHaveValue(BEA.display_name);
    expect(screen.queryByText(BEA.display_name, { exact: true })).not.toBeInTheDocument();
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  });

  it(`shows the returned person's name without overwriting a new search (${locale}, ${theme})`, async () => {
    await setLocale(locale);
    const target: AccessTarget = { kind: 'provider', id: 'provider', name: 'Supply' };
    const access = result(target.kind);
    access.user = { ...BEA, display_name: 'Beatrice' };
    calls.effective.mockResolvedValue(access);
    show(<div data-theme={theme}><ViewAs target={target} open onClose={vi.fn()} /></div>);
    await choose();
    await screen.findByRole('list', { name: i18n.t('access.viewAs.list') });
    const person = screen.getByRole('combobox', { name: i18n.t('access.viewAs.person') });
    expect(person).toHaveValue('Beatrice');
    expect(screen.queryByText('Beatrice', { exact: true })).not.toBeInTheDocument();
    await userEvent.clear(person);
    await userEvent.type(person, 'Ad');
    expect(person).toHaveValue('Ad');
    expect(screen.queryByRole('list', { name: i18n.t('access.viewAs.list') })).not.toBeInTheDocument();
    expect(calls.effective.mock.calls).toEqual([[target, BEA.id]]);
  });

  it(`keeps a resource's real error and retries the selected person (${locale}, ${theme})`, async () => {
    await setLocale(locale);
    const target: AccessTarget = { kind: 'provider', id: 'provider', name: 'Supply' };
    calls.effective.mockRejectedValueOnce(new ApiError({ type: 'orch:error:not_found', code: 'not_found', status: 404, title: '', detail: 'The person was removed.', request_id: 'rights' }, null));
    show(<div data-theme={theme}><ViewAs target={target} open onClose={vi.fn()} /></div>);
    await choose();
    expect(await screen.findByRole('alert')).toHaveTextContent(apiErrorSentence('not_found', 404, i18n.t, locale));
    expect(screen.queryByText(i18n.t('access.viewAs.nothing'), { exact: true })).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: i18n.t('common.tryAgain') }));
    await screen.findByRole('list', { name: i18n.t('access.viewAs.list') });
    expect(calls.effective.mock.calls).toEqual([[target, BEA.id], [target, BEA.id]]);
    await userEvent.clear(screen.getByRole('combobox', { name: i18n.t('access.viewAs.person') }));
    expect(screen.queryByRole('list', { name: i18n.t('access.viewAs.list') })).not.toBeInTheDocument();
    expect(calls.effective).toHaveBeenCalledTimes(2);
  });

  it(`renders source-owner provenance without a target owner or admin claim (${locale}, ${theme})`, async () => {
    await setLocale(locale);
    const access = result('environment');
    access.permissions = [{ permission: 'environment.manage', through: [{ kind: 'source_owner' }] }];
    calls.effective.mockResolvedValue(access);
    show(<div data-theme={theme}><ViewAs target={{ kind: 'environment', id: 'environment', name: 'Shared workspace' }} open onClose={vi.fn()} /></div>);
    await choose();
    const list = await screen.findByRole('list', { name: i18n.t('access.viewAs.list') });
    expect(list).toHaveTextContent(locale === 'en' ? 'Owns the machine this environment runs on' : '拥有此环境所在的机器');
    expect(list).not.toHaveTextContent('access.sourceOwner');
    expect(screen.queryByText(i18n.t('access.viewAs.owner'), { exact: true })).not.toBeInTheDocument();
    expect(screen.queryByText(i18n.t('access.viewAs.administers'), { exact: true })).not.toBeInTheDocument();
    expect(calls.effective).toHaveBeenCalledExactlyOnceWith({ kind: 'environment', id: 'environment', name: 'Shared workspace' }, BEA.id);
  });

  it(`renders admin provenance returned by the same endpoint (${locale}, ${theme})`, async () => {
    await setLocale(locale);
    const access = result('provider');
    access.permissions[0]?.through.push({ kind: 'admin', from: TOPIC }, { kind: 'system_admin' });
    calls.effective.mockResolvedValue(access);
    show(<div data-theme={theme}><ViewAs target={{ kind: 'provider', id: 'provider', name: 'Supply' }} open onClose={vi.fn()} /></div>);
    await choose();
    const list = await screen.findByRole('list', { name: i18n.t('access.viewAs.list') });
    expect(list).toHaveTextContent(i18n.t('access.viewAs.adminFrom', { place: TOPIC.name }));
    expect(list).toHaveTextContent(i18n.t('access.viewAs.systemAdmin'));
  });

  it(`opens the inspector once from the full resource permission page (${locale}, ${theme})`, async () => {
    await setLocale(locale);
    show(<div data-theme={theme}><AccessDialog target={{ kind: 'provider', id: 'provider', name: 'Supply' }} open initialPermission="provider.view" onClose={vi.fn()} /></div>);
    const entry = await screen.findByRole('button', { name: i18n.t('access.footer.viewAs') });
    expect(screen.getAllByRole('button', { name: i18n.t('access.footer.viewAs') })).toHaveLength(1);
    await userEvent.click(entry);
    await waitFor(() => expect(screen.getByRole('combobox', { name: i18n.t('access.viewAs.person') })).toBeVisible());
    expect(calls.effective).not.toHaveBeenCalled();
  });
}
