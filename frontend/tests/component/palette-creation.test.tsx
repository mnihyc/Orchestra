/** Creation asks its real place and permissions once; closing its chooser cancels a late answer. */
import { act, cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { CreationHost, closeCreation, openCreation } from '@/features/finder/Creation';
import type * as SearchApi from '@/api/search';
import type * as NodesApi from '@/api/nodes';
import type { NameSearch } from '@/api/search';
import type { NodeDialog } from '@/features/navigation/NodeDialogs';
import type { PlaceRef } from '@/features/navigation/places';
import type { NameHit, NameSearchPage, Node } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

const calls = vi.hoisted(() => ({ node: vi.fn<(id: string) => Promise<Node>>(), names: vi.fn<(search: NameSearch) => Promise<NameSearchPage>>(), dialog: vi.fn<(dialog: NodeDialog) => void>() }));
vi.mock('@/api/nodes', async (original) => ({ ...await original<typeof NodesApi>(),
  nodeQuery: (id: string) => ({ queryKey: ['creation-node', id], queryFn: () => calls.node(id), staleTime: 30_000 }),
}));
vi.mock('@/features/navigation/NodeDialogs', () => ({ openNodeDialog: calls.dialog }));
vi.mock('@/api/search', async (original) => {
  const actual = await original<typeof SearchApi>();
  return { ...actual, namesQuery: (search: NameSearch) => ({ ...actual.namesQuery(search), queryFn: () => calls.names(search) }) };
});

/** Only the fields the chooser reads: the native form is replaced at this component boundary. */
function node(permissions: string[]): Node {
  return { id: 'topic', kind: 'topic', name: 'Login', parent_id: 'project', archived: false, status: 'open', path: [],
    access: { permissions }, owner: { id: 'owner', kind: 'human', username: 'ada', display_name: 'Ada', avatar: { text: 'A', color: 1 } },
  } as unknown as Node;
}
const PARENT: PlaceRef = { id: 'topic', kind: 'topic', name: 'Login', parentId: 'project', archived: false, status: 'open', folder: false };
beforeEach(() => { calls.node.mockReset(); calls.dialog.mockClear(); calls.names.mockReset().mockResolvedValue({ items: [], partial: false }); });
afterEach(async () => { act(() => { closeCreation(); }); cleanup(); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) it(`opens the ordinary thread/document form only with the chosen place's actual permission (${locale})`, async () => {
  await setLocale(locale); calls.node.mockResolvedValue(node(['doc.create', 'topic.interact']));
  show(<CreationHost />);
  act(() => { openCreation('thread', 'topic'); });
  await waitFor(() => expect(calls.dialog).toHaveBeenCalledExactlyOnceWith({ type: 'new-thread', parent: PARENT }));
  expect(calls.node).toHaveBeenCalledExactlyOnceWith('topic');
  calls.dialog.mockClear(); calls.node.mockClear();
  act(() => { openCreation('document', 'topic'); });
  await waitFor(() => expect(calls.dialog).toHaveBeenCalledExactlyOnceWith({ type: 'new-document', parent: PARENT }));
  // The same authoritative node key is cached; this second opening makes no redundant read.
  expect(calls.node).not.toHaveBeenCalled();
});

it('says who may create in a refused place and leaves another-place choice available', async () => {
  await setLocale('en'); calls.node.mockResolvedValue(node([])); show(<CreationHost />);
  act(() => { openCreation('document', 'topic'); });
  await waitFor(() => expect(screen.getByRole('status')).toHaveTextContent('Ada'));
  expect(screen.getByRole('combobox', { name: 'Where' })).toBeEnabled();
  expect(calls.dialog).not.toHaveBeenCalled();
});

it('does not let a late permission read reopen a cancelled chooser', async () => {
  await setLocale('en'); let answer: ((value: Node) => void) | undefined;
  calls.node.mockImplementation(() => new Promise<Node>((resolve) => { answer = resolve; }));
  show(<CreationHost />); act(() => { openCreation('thread', 'topic'); });
  await waitFor(() => expect(calls.node).toHaveBeenCalledOnce());
  const user = userEvent.setup(); await user.click(screen.getByRole('button', { name: i18n.t('common.cancel') }));
  await act(async () => { answer?.(node(['topic.interact'])); await Promise.resolve(); });
  expect(calls.dialog).not.toHaveBeenCalled();
  expect(screen.queryByRole('dialog')).toBeNull();
});

for (const locale of ['en', 'zh-CN'] as const) it(`chooses a real place with the keyboard, no hidden lookup and an honest partial answer (${locale})`, async () => {
  await setLocale(locale); const t = i18n.getFixedT(locale);
  const hit: NameHit = { id: 'topic', kind: 'topic', name: 'Login', address: null, archived: false, avatar: null, color: null, custom_color: null,
    owner: null, recent: false, path: [], tag: null, thread_attention: null, username: null };
  calls.node.mockResolvedValue(node(['topic.interact'])); calls.names.mockResolvedValue({ items: [hit], partial: true });
  show(<CreationHost />); expect(calls.names).not.toHaveBeenCalled();
  act(() => { openCreation('thread'); });
  const search = screen.getByRole('combobox', { name: t('commands.create.where') });
  expect(search).toHaveFocus(); expect(calls.names).not.toHaveBeenCalled();
  const user = userEvent.setup(); await user.type(search, 'L');
  await waitFor(() => expect(screen.getByText(t('finder.namesMinimum'), { exact: true })).toBeVisible());
  expect(calls.names).not.toHaveBeenCalled();
  await user.type(search, 'ogin');
  expect(await screen.findByRole('option', { name: 'Login' })).toBeVisible();
  expect(screen.getByRole('status')).toHaveTextContent(t('finder.namesPartial'));
  expect(calls.names).toHaveBeenCalledExactlyOnceWith({ q: 'Login', kinds: ['project', 'topic'] });
  await user.keyboard('{Enter}');
  await waitFor(() => expect(calls.dialog).toHaveBeenCalledExactlyOnceWith({ type: 'new-thread', parent: PARENT }));
  expect(calls.node).toHaveBeenCalledExactlyOnceWith('topic');
});
