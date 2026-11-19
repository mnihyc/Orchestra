/** References an agent writes as `id:<id>`, with a part as `#n`: in a link or in backquotes they are the chip of what they
 * name, with its kind's glyph and its current name, and the card of a part opens at it; a reader who may not read the
 * thing keeps the words as they were written and is told no name; a shared-tree path is still a chip. */
import { cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Documents from '@/api/documents';
import type * as Nodes from '@/api/nodes';
import type * as References from '@/api/references';
import type * as Share from '@/api/share';
import { Body } from '@/features/chat/Body';
import { AgentReferences } from '@/features/people/AgentReferences';
import { idReferenceOf, type TypedReference } from '@/features/people/references';
import type { ReferencePreview } from '@/generated/api-types';
import { i18n } from '@/i18n';
import { Markdown } from '@/ui/Markdown';
import { show } from './support';

const THREAD = '0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5e10';
const DOC = '0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5e11';
const SECRET = '0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5e12';
const AGENT_THREAD = 'agent-thread';
const person = { id: 'ada', kind: 'human', username: 'ada', display_name: 'Ada', avatar: { text: 'A', color: 3 } } as const;

function preview(reference: TypedReference): ReferencePreview | null {
  const base = { place: null, description: '', updated_at: null, updated_by: null };
  if (reference.kind === 'node' && reference.id === THREAD) {
    return { ...base, kind: 'node', id: THREAD, node_kind: 'thread', title: 'Main thread',
      part: reference.turn === undefined ? null : { part: { turn: reference.turn }, number: reference.turn, anchor: null, text: 'Fix the redirect.', by: person } };
  }
  if (reference.kind === 'document' && reference.id === DOC) return { ...base, kind: 'document', id: DOC, node_kind: null, title: 'Rollout plan', part: null };
  // Anything else, and anything its reader may not read, is not found.
  return null;
}

const calls = vi.hoisted(() => ({
  preview: vi.fn<(reference: TypedReference) => Promise<unknown>>(),
  node: vi.fn<(id: string) => Promise<unknown>>(),
  document: vi.fn<(id: string) => Promise<unknown>>(),
  tree: vi.fn<(threadId: string, path: string) => Promise<unknown>>(),
}));
vi.mock('@/api/references', async (original) => {
  const actual = await original<typeof References>();
  return { ...actual, referencePreviewQuery: (reference: TypedReference) => ({ ...actual.referencePreviewQuery(reference), queryFn: () => calls.preview(reference) }) };
});
vi.mock('@/api/nodes', async (original) => {
  const actual = await original<typeof Nodes>();
  return { ...actual, nodeQuery: (id: string) => ({ ...actual.nodeQuery(id), queryFn: () => calls.node(id) }) };
});
vi.mock('@/api/documents', async (original) => {
  const actual = await original<typeof Documents>();
  return { ...actual, documentQuery: (id: string) => ({ ...actual.documentQuery(id), queryFn: () => calls.document(id) }) };
});
vi.mock('@/api/share', async (original) => {
  const actual = await original<typeof Share>();
  return { ...actual, sharedTreeQuery: (threadId: string, path = '/') => ({ ...actual.sharedTreeQuery(threadId, path), queryFn: () => calls.tree(threadId, path) }) };
});

beforeEach(() => {
  vi.clearAllMocks();
  calls.preview.mockImplementation((reference) => {
    const found = preview(reference);
    return found === null ? Promise.reject(new Error('not found')) : Promise.resolve(found);
  });
  calls.node.mockImplementation((id) => id === THREAD ? Promise.resolve({ id: THREAD, kind: 'thread', name: 'Main thread' }) : Promise.reject(new Error('not found')));
  calls.document.mockImplementation((id) => id === DOC ? Promise.resolve({ id: DOC, title: 'Rollout plan' }) : Promise.reject(new Error('not found')));
  calls.tree.mockResolvedValue({ entries: [{ path: 'notes/plan.md', name: 'Plan', kind: 'document', id: DOC, readable: true, redacted: false }] });
});
afterEach(() => cleanup());

const answer = (text: string) => show(<AgentReferences threadId={AGENT_THREAD}><Markdown text={text} /></AgentReferences>);

it('reads `id:<id>` with an optional part, and nothing else', () => {
  expect(idReferenceOf(`id:${THREAD}`)).toEqual({ id: THREAD });
  expect(idReferenceOf(`id:${THREAD}#3`)).toEqual({ id: THREAD, number: 3 });
  for (const other of [`id:${THREAD}#0`, `id:${THREAD}#x`, `id:not-an-id`, `/nodes/${THREAD}`, 'notes/plan.md', `ID ${THREAD}`]) expect(idReferenceOf(other)).toBeNull();
});

it('draws a readable `id:` link in an answer as the chip of what it names, with its kind\'s glyph and its current name', async () => {
  answer(`See [the old name](id:${THREAD}) and \`id:${DOC}\`.`);
  const thread = await screen.findByRole('link', { name: 'Main thread' });
  expect(thread).toHaveAttribute('href', `/thread/${THREAD}`);
  expect(thread.querySelector('svg')).not.toBeNull();
  expect(screen.queryByText('the old name')).not.toBeInTheDocument();
  const document = await screen.findByRole('link', { name: 'Rollout plan' });
  expect(document).toHaveAttribute('href', `/doc/${DOC}`);
  expect(document.querySelector('svg')).not.toBeNull();
  // A document is asked as a place first, which it is not, then as a document.
  expect(calls.preview).toHaveBeenCalledWith({ kind: 'node', id: DOC });
  expect(calls.preview).toHaveBeenCalledWith({ kind: 'document', id: DOC });
});

it('opens a chip written with `#n` at that part: its card names the turn and asks for it', async () => {
  const user = userEvent.setup();
  answer(`Look at [the fix](id:${THREAD}#3).`);
  const chip = await screen.findByRole('link', { name: 'Main thread' });
  await user.hover(chip);
  await waitFor(() => expect(document.querySelector('[data-reference-card]')).not.toBeNull(), { timeout: 2_000 });
  const card = document.querySelector<HTMLElement>('[data-reference-card]');
  if (card === null) throw new Error('The chip has no card');
  await waitFor(() => expect(card).toHaveTextContent(i18n.t('people.reference.turn', { number: 3 })));
  expect(within(card).getByText('Fix the redirect.')).toBeInTheDocument();
  expect(calls.preview).toHaveBeenCalledWith({ kind: 'node', id: THREAD, turn: 3, label:'the fix' });
});

it('keeps an unreadable `id:` reference as the words written, in a link and in backquotes, and asks no name for it', async () => {
  answer(`Here is [my own words](id:${SECRET}) and \`id:${SECRET}#2\`.`);
  await waitFor(() => expect(calls.preview).toHaveBeenCalledWith({ kind: 'chat', id: SECRET }));
  expect(calls.preview).toHaveBeenCalledWith({ kind: 'node', id: SECRET });
  expect(calls.preview).toHaveBeenCalledWith({ kind: 'document', id: SECRET });
  expect(screen.getByText('my own words')).toBeInTheDocument();
  expect(screen.getByRole('button',{name:`id:${SECRET}#2`})).toHaveAttribute('data-reference-state','no-access');
  expect(screen.getByRole('button',{name:'my own words'})).toHaveAttribute('data-reference-state','no-access');
  expect(screen.queryByRole('link')).not.toBeInTheDocument();
  expect(calls.node).not.toHaveBeenCalled();
  expect(calls.document).not.toHaveBeenCalled();
});

it('still draws a shared-tree path as the chip its thread resolves', async () => {
  answer('The plan is in `notes/plan.md`.');
  // The chip says the document's current title, not the name the tree listed.
  const chip = await screen.findByRole('link', { name: 'Rollout plan' });
  expect(chip).toHaveAttribute('href', `/doc/${DOC}`);
  expect(calls.tree).toHaveBeenCalledWith(AGENT_THREAD, 'notes/');
});

it('draws an `id:` reference in a chat message as the same chip', async () => {
  show(<Body text={`An agent's note: [the thread](id:${THREAD}#3), and [nothing to see](id:${SECRET}).`} originThread={AGENT_THREAD} />);
  expect(await screen.findByRole('link', { name: 'Main thread' })).toHaveAttribute('href', `/thread/${THREAD}`);
  await waitFor(() => expect(calls.preview).toHaveBeenCalledWith({ kind: 'chat', id: SECRET }));
  expect(screen.getByText('nothing to see')).toBeInTheDocument();
  expect(screen.getAllByRole('link')).toHaveLength(1);
});
