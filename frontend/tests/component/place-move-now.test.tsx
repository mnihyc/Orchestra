/** A place moves at once: inside one organization nothing is said; into another one line counts who loses access; the notice offers Undo. */
import { act, cleanup, screen, waitFor } from '@testing-library/react';
import { renderHook } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { type ReactNode, useState } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as NodeApi from '@/api/nodes';
import { ApiError } from '@/api/client';
import { crossesOrganization, MoveDialog, useMovePlace } from '@/features/org/Move';
import type { PlaceRef } from '@/features/navigation/places';
import type { Node, NodeMovePlan } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { useTreeStore } from '@/realtime/stores/tree';
import { Toaster } from '@/ui/Toast';
import { treeNode } from '../doubles/tree-node';
import { ADA, DORA } from '../doubles/chat-rows';
import { show } from './support';

const calls = vi.hoisted(() => ({ preview: vi.fn(), move: vi.fn(), read: vi.fn<(id: string) => Promise<Node>>() }));
vi.mock('@/api/nodes', async (original) => {
  const actual = await original<typeof NodeApi>();
  return { ...actual, previewMove: calls.preview, moveNode: calls.move, readNode: calls.read,
    nodeQuery: (id: string) => ({ queryKey: ['place-move-now', id], queryFn: () => calls.read(id) }) };
});
vi.mock('@/realtime/live', async (load) => ({ ...await load<object>(), useOrgTreeFeed: () => undefined }));
// The hook alone is drawn outside a router; an org's own move navigates, which these places do not.
vi.mock('@tanstack/react-router', async (load) => ({ ...await load<object>(), useNavigate: () => vi.fn() }));

const rows = [
  treeNode({ id: 'acme', kind: 'org', parent_id: null, name: 'Acme' }),
  treeNode({ id: 'web', kind: 'project', parent_id: 'acme', name: 'Web' }),
  treeNode({ id: 'docs', kind: 'project', parent_id: 'acme', name: 'Docs' }),
  treeNode({ id: 'labs', kind: 'org', parent_id: 'acme', name: 'Labs' }),
  treeNode({ id: 'landing', kind: 'topic', parent_id: 'web', name: 'Landing page' }),
];
const ref = (id: string, name: string, kind: Node['kind']) => ({ id, name, kind, redacted: false });
function node(id: string, parent: string, kind: 'project' | 'topic', name: string): Node {
  const path = kind === 'topic' ? [ref('acme', 'Acme', 'org'), ref(parent, 'Web', 'project'), ref(id, name, 'topic')] : [ref('acme', 'Acme', 'org'), ref(id, name, 'project')];
  return { id, kind, name, org_id: 'acme', parent_id: parent, revision: 3, path } as unknown as Node;
}
const topic: PlaceRef = { id: 'landing', kind: 'topic', name: 'Landing page', parentId: 'web', archived: false, status: 'open', folder: false };
const project: PlaceRef = { id: 'web', kind: 'project', name: 'Web', parentId: 'acme', archived: false, status: null, folder: false };
function plan(losing: number): NodeMovePlan {
  const person = (index: number) => ({ user: index === 0 ? ADA : DORA, permissions: ['topic.view'], pages: 1 });
  return { access: { pages: 1, orgs: null, gaining: [], losing: Array.from({ length: losing }, (_, index) => person(index)), more: 0 } } as unknown as NodeMovePlan;
}

/** The dialog closes as its host would, so that the notice outside it can be reached. */
function Host({ place, onClose }: { place: PlaceRef; onClose: () => void }) {
  const [open, setOpen] = useState(true);
  return <MoveDialog place={place} open={open} onClose={() => { setOpen(false); onClose(); }} />;
}

const savedTrees = useTreeStore.getState();
beforeEach(() => {
  useTreeStore.setState({ byOrg: new Map([['acme', { nodes: new Map(rows.map((row) => [row.id, row])), lastSeq: 1, loaded: true, error: null }]]) });
  calls.read.mockImplementation((id: string) => Promise.resolve(id === 'web' ? node('web', 'acme', 'project', 'Web') : node('landing', 'web', 'topic', 'Landing page')));
  calls.move.mockImplementation((_kind: string, id: string, to: string) => Promise.resolve({ ...node(id, to, id === 'web' ? 'project' : 'topic', id === 'web' ? 'Web' : 'Landing page'), revision: 4 }));
});
afterEach(async () => { cleanup(); vi.resetAllMocks(); useTreeStore.setState(savedTrees); await setLocale('en'); });

it('knows which moves leave the organization', () => {
  const nodes = new Map(rows.map((row) => [row.id, row]));
  expect(crossesOrganization(nodes, topic, 'docs')).toBe(false);
  expect(crossesOrganization(nodes, project, 'labs')).toBe(true);
  expect(crossesOrganization(nodes, { ...project, kind: 'org' }, 'labs')).toBe(true);
});

for (const locale of ['en', 'zh-CN'] as const) {
  it(`moves a topic inside its organization at once, says nothing about access and offers Undo (${locale})`, async () => {
    await setLocale(locale);
    const user = userEvent.setup();
    const close = vi.fn();
    show(<Host place={topic} onClose={close} />);
    await user.click(await screen.findByRole('radio', { name: 'Docs' }));
    const move = screen.getByRole('button', { name: i18n.t('places.move.confirm', { name: topic.name, where: 'Docs' }) });
    expect(move).toBeEnabled();
    expect(calls.preview).not.toHaveBeenCalled();
    expect(screen.queryByRole('status')).not.toBeInTheDocument();
    await user.click(move);
    await waitFor(() => expect(calls.move).toHaveBeenCalledExactlyOnceWith('topic', 'landing', 'docs', 3, undefined, expect.any(String)));
    await waitFor(() => expect(close).toHaveBeenCalledOnce());
    expect(calls.preview).not.toHaveBeenCalled();
    await user.click((await screen.findAllByRole('button', { name: i18n.t('places.toast.undo') }))[0]!);
    await waitFor(() => expect(calls.move).toHaveBeenLastCalledWith('topic', 'landing', 'web', 3, undefined, expect.any(String)));
  });
}

it('into another organization one line counts who loses access, the names on Show, and Move never waits for it', async () => {
  const user = userEvent.setup();
  let ready: ((value: NodeMovePlan) => void) | undefined;
  calls.preview.mockImplementation(() => new Promise<NodeMovePlan>((resolve) => { ready = resolve; }));
  const close = vi.fn();
  show(<Host place={project} onClose={close} />);
  await user.click(await screen.findByRole('radio', { name: 'Labs' }));
  const move = screen.getByRole('button', { name: i18n.t('places.move.confirm', { name: project.name, where: 'Labs' }) });
  expect(move).toBeEnabled();
  await waitFor(() => expect(calls.preview).toHaveBeenCalledExactlyOnceWith('project', 'web', 'labs'));
  act(() => ready?.(plan(2)));
  expect(await screen.findByText(i18n.t('places.move.loses', { count: 2 }))).toBeVisible();
  expect(screen.queryByText('Ada Lovelace')).not.toBeInTheDocument();
  await user.click(screen.getByRole('button', { name: i18n.t('actions.show.label') }));
  expect(screen.getByText('Ada Lovelace')).toBeVisible();
  expect(screen.getByText('Dora Developer')).toBeVisible();
  await user.click(move);
  await waitFor(() => expect(calls.move).toHaveBeenCalledExactlyOnceWith('project', 'web', 'labs', 3, undefined, expect.any(String)));
  await waitFor(() => expect(close).toHaveBeenCalledOnce());
  expect(await screen.findByText(i18n.t('places.move.lost', { count: 2 }))).toBeVisible();
});

it('a refusal of the tree itself stays in the dialog, and a failed access read says nothing and moves anyway', async () => {
  const user = userEvent.setup();
  calls.preview.mockRejectedValue(new Error('The access difference could not be read.'));
  calls.move.mockRejectedValueOnce(new ApiError({ type: 'orch:error:node.wrong_parent', code: 'node.wrong_parent', status: 409, title: 'Refused', detail: 'it cannot be moved into itself or below itself', request_id: 'move' }, null));
  const close = vi.fn();
  show(<Host place={project} onClose={close} />);
  await user.click(await screen.findByRole('radio', { name: 'Labs' }));
  await waitFor(() => expect(calls.preview).toHaveBeenCalled());
  expect(screen.queryByRole('status')).not.toBeInTheDocument();
  const move = screen.getByRole('button', { name: i18n.t('places.move.confirm', { name: project.name, where: 'Labs' }) });
  await user.click(move);
  expect(await screen.findByRole('alert')).toBeVisible();
  expect(close).not.toHaveBeenCalled();
  await user.click(move);
  await waitFor(() => expect(close).toHaveBeenCalledOnce());
});

it('a dropped row reads its current revision, then moves; into another organization it reads who loses access first', async () => {
  calls.preview.mockResolvedValue(plan(1));
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  const wrapper = ({ children }: { children: ReactNode }) => <QueryClientProvider client={client}><Toaster>{children}</Toaster></QueryClientProvider>;
  const { result } = renderHook(() => useMovePlace(), { wrapper });
  act(() => result.current.mutate({ place: topic, to: 'docs', where: 'Acme / Docs', across: false }));
  await waitFor(() => expect(calls.move).toHaveBeenCalledExactlyOnceWith('topic', 'landing', 'docs', 3, undefined, expect.any(String)));
  expect(calls.read).toHaveBeenCalledWith('landing');
  expect(calls.preview).not.toHaveBeenCalled();
  act(() => result.current.mutate({ place: project, to: 'labs', where: 'Acme / Labs', across: true }));
  await waitFor(() => expect(calls.move).toHaveBeenLastCalledWith('project', 'web', 'labs', 3, undefined, expect.any(String)));
  expect(calls.preview).toHaveBeenCalledExactlyOnceWith('project', 'web', 'labs');
  expect(await screen.findByText(i18n.t('places.move.lost', { count: 1 }))).toBeVisible();
});

it('a dropped row the tree refuses says so in a notice, as a drop has no dialog', async () => {
  calls.move.mockRejectedValueOnce(new ApiError({ type: 'orch:error:node.wrong_parent', code: 'node.wrong_parent', status: 409, title: 'Refused', detail: 'it cannot be moved into itself or below itself', request_id: 'drop' }, null));
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  const wrapper = ({ children }: { children: ReactNode }) => <QueryClientProvider client={client}><Toaster>{children}</Toaster></QueryClientProvider>;
  const { result } = renderHook(() => useMovePlace({ refusalNotice: true }), { wrapper });
  act(() => result.current.mutate({ place: topic, to: 'docs', where: 'Acme / Docs', across: false }));
  expect(await screen.findByText(i18n.t('places.toast.notMoved', { name: topic.name }))).toBeVisible();
});
