/** The real defaults grid reads, previews and writes the system endpoint with its reviewed revision. */
import { cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Access from '@/api/access';
import type * as Auth from '@/api/auth';
import type * as Nodes from '@/api/nodes';
import { ApiError } from '@/api/client';
import { AccessDefaults } from '@/features/admin/AccessDefaults';
import type { HolderSearchReply, Node, PermissionPage } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { choiceOption } from '@/i18n/choices';
import { permissionNameOf } from '@/features/access/names';
import { CATALOGUE, pageFor } from '../unit/general-access-fixtures';
import { me } from '../doubles/fixtures';
import { show } from './support';

const calls = vi.hoisted(() => ({ read: vi.fn<typeof Access.readSystemDefaults>(), preview: vi.fn<typeof Access.previewSystemDefaults>(),
  change: vi.fn<typeof Access.changeSystemDefaults>(), node: vi.fn<() => Promise<Node>>(), candidates: vi.fn<() => Promise<HolderSearchReply>>() }));
vi.mock('@/api/access', async (load) => ({ ...await load<typeof Access>(), readSystemDefaults: calls.read,
  previewSystemDefaults: calls.preview, changeSystemDefaults: calls.change,
  permissionsQuery: { queryKey: ['permissions'], queryFn: () => Promise.resolve(CATALOGUE.reply) },
  holderSearchQuery: () => ({ queryKey: ['system-candidates'], queryFn: calls.candidates }),
}));
vi.mock('@/api/auth', async (load) => ({ ...await load<typeof Auth>(), meQuery: { queryKey: ['me'], queryFn: () => Promise.resolve(me(1, 'ada', 'Ada')) } }));
vi.mock('@/api/nodes', async (load) => ({ ...await load<typeof Nodes>(), nodeQuery: (id: string) => ({ queryKey: ['node', id], queryFn: calls.node }) }));
vi.mock('@/features/navigation/placeLabels', () => ({ usePlaceLabel: () => (value: { name?: string | null } | null) => value?.name ?? null }));

function systemPage(): PermissionPage {
  const page = pageFor('org');
  return { ...page, object: { id: page.object.id, kind: 'system', name: 'System' }, page: 'defaults', owner: null, owners: [],
    admins: { count: 1, of: ['system'] }, followers: 23,
    rows: page.rows.map((row) => ({ ...row, inherit: false, inherited: null, entries: [] })) };
}
beforeEach(() => {
  vi.clearAllMocks(); calls.read.mockReset().mockResolvedValue(systemPage());
  calls.candidates.mockResolvedValue({ items: [] });
  calls.preview.mockReset().mockResolvedValue({ pages: 23, orgs: 3, gaining: [], losing: [], more: 0 });
  calls.change.mockReset().mockResolvedValue({ ...systemPage(), revision: 8 });
});
afterEach(async () => { cleanup(); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) {
  it(`adds an implicit role in following places without inventing a system node or membership in ${locale}`, async () => {
    await setLocale(locale); const user = userEvent.setup(); show(<AccessDefaults />);
    await screen.findByRole('button', { name: i18n.t('adminDefaults.reset') });
    const def = CATALOGUE.of('org').find((entry) => entry.name === 'org.view');
    const role = CATALOGUE.reply.roles[0];
    if (def === undefined || role === undefined) throw new Error('The catalogue must declare an org view permission and a role.');
    await user.click(screen.getByRole('button', { name: i18n.t('access.cell.addTo', { permission: permissionNameOf(def) }) }));
    await user.click(await screen.findByRole('combobox', { name: i18n.t('adminDefaults.role') }));
    await user.click(screen.getByRole('option', { name: choiceOption(i18n.t, `accessRole.${role.key}`).name }));
    await waitFor(() => expect(calls.change).toHaveBeenCalledExactlyOnceWith({ expected_revision: 7,
      rows: [{ permission: 'org.view', add: [{ holder: { kind: 'role', role: role.key } }] }] }));
    expect(calls.node).not.toHaveBeenCalled();
    expect(calls.preview).not.toHaveBeenCalled();
  });

  it(`previews and confirms the native system reset with the exact revision, without a fake node request in ${locale}`, async () => {
    await setLocale(locale); const user = userEvent.setup();
    show(<AccessDefaults />);
    const reset = await screen.findByRole('button', { name: i18n.t('adminDefaults.reset') });
    expect(calls.read).toHaveBeenCalledOnce();
    expect(calls.node).not.toHaveBeenCalled(); expect(calls.candidates).not.toHaveBeenCalled();
    await user.click(reset);
    const review = screen.getByRole('alertdialog', { name: i18n.t('adminDefaults.resetTitle') });
    expect(review).toHaveTextContent(i18n.t('adminDefaults.resetBody'));
    await waitFor(() => expect(calls.preview).toHaveBeenCalledExactlyOnceWith({ expected_revision: 7, whole: 'reset' }));
    expect(calls.change).not.toHaveBeenCalled();
    await within(review).findByText(i18n.t('access.preview.orgs', { count: 3 }), { exact: false });
    await user.click(within(review).getByRole('button', { name: i18n.t('adminDefaults.reset') }));
    await waitFor(() => expect(calls.change).toHaveBeenCalledExactlyOnceWith({ expected_revision: 7, whole: 'reset' }));
    expect(calls.node).not.toHaveBeenCalled(); expect(calls.candidates).not.toHaveBeenCalled();
  });

  it(`keeps a failed system read as an error with Retry and no editing controls in ${locale}`, async () => {
    await setLocale(locale);
    calls.read.mockRejectedValueOnce(new ApiError({ type: 'orch:error:internal', code: 'internal', title: 'Read failed', detail: 'The synthetic defaults read failed.', status: 500, request_id: 'system-defaults' }, null));
    const user = userEvent.setup(); show(<AccessDefaults />);
    expect(await screen.findByRole('alert')).toHaveTextContent(i18n.t('errors.internal'));
    expect(screen.queryByRole('button', { name: i18n.t('adminDefaults.reset') })).not.toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: i18n.t('common.tryAgain') }));
    await screen.findByRole('button', { name: i18n.t('adminDefaults.reset') });
    expect(calls.read).toHaveBeenCalledTimes(2); expect(calls.change).not.toHaveBeenCalled();
  });
}
