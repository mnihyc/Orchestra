/** A route may reveal a group once; native metadata refresh never reverses the reader's dismissal. */
import { useQueryClient } from '@tanstack/react-query';
import { createMemoryHistory, createRootRoute, createRouter, RouterProvider } from '@tanstack/react-router';
import { cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import type * as Directory from '@/api/directory';
import { groupsQuery } from '@/api/directory';
import type { Candidate } from '@/features/access/candidates';
import { Groups } from '@/features/settings/org/Groups';
import type * as PageContext from '@/features/workbench/pageContext';
import type { Group } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { ADA } from '../doubles/chat-rows';
import { ACME, id, org as organization } from '../doubles/fixtures';
import { show } from './support';

const crew: Group = { id: id(19, 1), org_id: ACME.org, name: 'Review crew', description: '', revision: 1, member_count: 0 };
const state = vi.hoisted(() => ({ group: '', read: vi.fn<() => Promise<Group[]>>() }));
vi.mock('@/features/workbench/pageContext', async (load) => ({ ...await load<typeof PageContext>(), usePageContext: () => ({ target: { search: { group: state.group } } }) }));
vi.mock('@/features/access/candidates', () => ({ useCandidates: () => ({ groups: [], byValue: new Map<string, Candidate>(), loading: false, placePicker: null }) }));
vi.mock('@/api/directory', async (load) => {
  const actual = await load<typeof Directory>();
  return { ...actual, groupsQuery: (org: string, under: boolean) => ({ ...actual.groupsQuery(org, under), queryFn: state.read }),
    groupHoldersQuery: (group: string) => ({ queryKey: ['group-holders-test', group], queryFn: () => Promise.resolve([]) }) };
});
const org = organization(ACME.org, 'acme', 'Acme', ADA);
function View() {
  const cache = useQueryClient();
  return <><button onClick={() => {
    const rows = [{ ...crew, description: 'Fresh native metadata', revision: 2 }];
    state.read.mockResolvedValue(rows);
    cache.setQueryData(groupsQuery(org.id, false).queryKey, rows);
  }}>Publish native group metadata</button><Groups org={org} /></>;
}
afterEach(async () => { cleanup(); vi.clearAllMocks(); await setLocale('en'); });

it.each(['en', 'zh-CN'] as const)('keeps the dismissed route inspection closed on refresh and lets the reader reopen it in %s', async (locale) => {
  await setLocale(locale); state.group = crew.id; state.read.mockResolvedValue([crew]);
  const user = userEvent.setup();
  const root = createRootRoute({ component: View });
  const router = createRouter({ routeTree: root, history: createMemoryHistory({ initialEntries: ['/'] }) });
  show(<RouterProvider router={router} />);
  const dialog = await screen.findByRole('dialog', { name: i18n.t('members.groups.membersTitle', { name: crew.name }) });
  expect(dialog).toBeVisible();
  await user.keyboard('{Escape}');
  await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());
  await user.click(screen.getByRole('button', { name: 'Publish native group metadata' }));
  expect(await screen.findByText('Fresh native metadata')).toBeVisible();
  expect(screen.queryByRole('dialog')).toBeNull();
  await user.click(screen.getByRole('button', { name: i18n.t('members.groups.openMembers', { name: crew.name }) }));
  expect(await screen.findByRole('dialog', { name: i18n.t('members.groups.membersTitle', { name: crew.name }) })).toBeVisible();
});
