/** Disabled candidates retain native causes and recovery; choosing never probes with a write. */
import { cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Destinations from '@/api/copyDestinations';
import type * as Nodes from '@/api/nodes';
import { CopyPlacePicker } from '@/features/duplicate/CopyPlacePicker';
import type { PickedPlace } from '@/features/settings/PlacePicker';
import { i18n, setLocale } from '@/i18n';
import { NetworkError } from '@/lib/requests';
import { org, me, userRef } from '../doubles/fixtures';
import { treeNode } from '../doubles/tree-node';
import { show } from './support';
const calls = vi.hoisted(() => ({ judge: vi.fn<typeof Destinations.judgeCopyDestinations>(), access: vi.fn<(...args: unknown[]) => void>() }));
vi.mock('@/features/access/AccessDialog', async (loadOriginal) => ({ ...await loadOriginal<object>(), openAccess: calls.access }));
vi.mock('@/api/copyDestinations', async (original) => ({ ...await original<typeof Destinations>(),
  copyDestinationsQuery: (request: Destinations.CopyDestinationsRequest, reader: readonly unknown[]) => ({ queryKey: ['copy-destinations', reader, request], queryFn: ({ signal }: { signal: AbortSignal }) => calls.judge(request, signal), retry: false, gcTime: 0 }),
}));
vi.mock('@/api/nodes', async (original) => ({ ...await original<typeof Nodes>(),
  orgsQuery: { queryKey: ['judged-orgs'], queryFn: () => Promise.resolve([org('root', 'team', 'Team', userRef(me(1, 'reader', 'Reader')))]) },
  fetchTree: () => Promise.resolve({ seq: 1, since: null, complete: true, hidden: [], nodes: [treeNode({ id: 'root', kind: 'org', parent_id: null, name: 'Team' }), treeNode({ id: 'choose', kind: 'project', parent_id: 'root', name: 'Choose' }), treeNode({ id: 'blocked', kind: 'project', parent_id: 'root', name: 'Blocked' })] }),
}));
function Picker() {
  const [value, setValue] = useState<PickedPlace | null>(null);
  return <CopyPlacePicker label={i18n.t('duplicate.destination')} subject={{ kind: 'provider', id: 'source' }} intent="duplicate" value={value} onChange={setValue} onStanding={() => undefined} />;
}
beforeEach(() => {
  vi.clearAllMocks();
  calls.judge.mockImplementation((request) => Promise.resolve({ subject: request.subject, intent: request.intent, source_owned: false,
    destinations: request.destinations.map((destination) => ({ destination, cross_root: true, outcome: destination.kind === 'node' && destination.id === 'blocked'
      ? { state: 'blocked' as const, cause: 'owners_only' as const, problem: { type: 'orch:error:authz.denied', code: 'authz.denied' as const, status: 403, title: 'Denied', detail: '', request_id: 'judge' } }
      : destination.kind === 'node' && destination.id === 'choose' ? { state: 'needs_choice' as const, choice: 'mcp_machine' as const } : { state: 'allowed' as const } })) }));
});
afterEach(async () => { cleanup(); await setLocale('en'); });
for (const locale of ['en', 'zh-CN'] as const) {
  it(`retains a blocked place with its native reason and recovery in ${locale}`, async () => {
    await setLocale(locale); const user = userEvent.setup(); show(<Picker />); await user.click(screen.getByRole('combobox', { name: i18n.t('duplicate.destination') }));
    const blocked = await screen.findByRole('option', { name: 'Team / Blocked' });
    await waitFor(() => expect(blocked).toHaveTextContent(i18n.t('duplicate.causes.owners_only'))); expect(blocked).toHaveAttribute('aria-disabled', 'true');
    await user.click(screen.getByRole('button', { name: i18n.t('duplicate.reviewDestination', { name: 'Team / Blocked' }) }));
    const dialog = await screen.findByRole('dialog', { name: 'Team / Blocked' }); expect(dialog).toHaveTextContent(i18n.t('duplicate.causes.owners_only'));
    await user.click(screen.getByRole('button', { name: i18n.t('duplicate.reviewAccess') })); expect(calls.access).toHaveBeenCalledWith({ kind: 'provider', id: 'source', name: i18n.t('access.missing.thisItem') }, { share: false });
  });
  it(`allows a needs-choice place without presenting it as a denial in ${locale}`, async () => {
    await setLocale(locale); const user = userEvent.setup(); show(<Picker />); await user.click(screen.getByRole('combobox', { name: i18n.t('duplicate.destination') }));
    const choose = await screen.findByRole('option', { name: 'Team / Choose' }); await waitFor(() => expect(choose).toHaveTextContent(i18n.t('duplicate.choices.mcp_machine')));
    expect(choose).not.toHaveAttribute('aria-disabled', 'true'); await user.click(choose); expect(screen.getByText('Team / Choose', { exact: true })).toBeInTheDocument();
  });
}
it('a failed native judgement leaves candidates disabled and offers a read retry', async () => {
  calls.judge.mockRejectedValue(new NetworkError('failed')); const user = userEvent.setup(); show(<Picker />); await user.click(screen.getByRole('combobox', { name: i18n.t('duplicate.destination') }));
  const place = await screen.findByRole('option', { name: 'Team' }); await waitFor(() => expect(place).toHaveTextContent(i18n.t('duplicate.judgeFailed')));
  expect(place).toHaveAttribute('aria-disabled', 'true'); await screen.findByRole('alert'); expect(calls.access).not.toHaveBeenCalled();
});
