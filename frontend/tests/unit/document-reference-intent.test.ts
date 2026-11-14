/** Caret reference reads belong to the current visible editor and reader. */
import { beforeEach, expect, it, vi } from 'vitest';
import type * as ClientModule from '@/api/client';
import type * as QueryModule from '@/api/query';
import type * as TreeModule from '@/realtime/stores/tree';
import { referenceChoices, type ReferenceContext } from '@/features/documents/referenceChoices';

interface Page { items: { id: string; title: string }[]; next_cursor: string | null }
const wire = vi.hoisted(() => ({ reader: 1, read: vi.fn<() => Promise<Page>>() }));
vi.mock('@/api/client', async (loadOriginal) => {
  const actual = await loadOriginal<typeof ClientModule>();
  return { ...actual, client: { ...actual.client, session: { ...actual.client.session,
    getState: () => ({ ...actual.client.session.getState(), readerVersion: wire.reader }),
  } } };
});
vi.mock('@/api/query', async (loadOriginal) => {
  const actual = await loadOriginal<typeof QueryModule>();
  return { ...actual, queryClient: Object.assign(actual.queryClient, { query: wire.read }) };
});
vi.mock('@/api/documents', () => ({ documentReferencePageQuery: (id: string, query: string, cursor: string | null) => ({ queryKey: ['documents', id, query, cursor] }) }));
vi.mock('@/realtime/stores/tree', async (loadOriginal) => {
  const actual = await loadOriginal<typeof TreeModule>();
  const state = actual.useTreeStore.getState;
  return { ...actual, useTreeStore: Object.assign(actual.useTreeStore, { getState: () => ({ ...state(), byOrg: new Map([
    ['root', { nodes: new Map([['topic', { id: 'topic', kind: 'topic', name: 'Visible topic', deleted: false }]]) }],
  ]) }) }) };
});

const document: ReferenceContext['document'] = { id: 'current', node: { id: 'topic', kind: 'topic', name: 'Topic', redacted: false },
  path: [{ id: 'root', kind: 'org', name: 'Root', redacted: false }] };
beforeEach(() => { wire.reader = 1; wire.read.mockReset().mockResolvedValue({ items: [{ id: 'other', title: 'Visible notes' }], next_cursor: null }); });

it('makes no read before a live editor has committed its reference context', async () => {
  expect(await referenceChoices(() => undefined, '')).toEqual([]);
  expect(wire.read).not.toHaveBeenCalled();
});

it('discards a pending answer when that exact editor is retired', async () => {
  let finish: (value: Page) => void = () => { throw new Error('The reference read did not start'); };
  wire.read.mockImplementationOnce(() => new Promise((resolve) => { finish = resolve; }));
  let context: ReferenceContext | undefined = { document, enabled: true };
  const pending = referenceChoices(() => context, '');
  context = undefined;
  finish({ items: [{ id: 'old', title: 'Retired editor choice' }], next_cursor: null });
  expect(await pending).toEqual([]);
});

it('does not read an inactive editor and loads native choices on its actual reference intent', async () => {
  const context: ReferenceContext = { document, enabled: false };
  expect(await referenceChoices(() => context, '')).toEqual([]);
  expect(wire.read).not.toHaveBeenCalled();
  context.enabled = true;
  expect(await referenceChoices(() => context, 'visible')).toEqual([
    { id: 'other', kind: 'document', name: 'Visible notes', hint: 'document' },
    { id: 'topic', kind: 'node', name: 'Visible topic', hint: 'topic' },
  ]);
  expect(wire.read).toHaveBeenCalledOnce();
});

it.each(['reader', 'document', 'move', 'root', 'ancestry', 'redaction', 'hidden'] as const)('discards an old native answer after the %s boundary changes', async (boundary) => {
  let finish: (value: Page) => void = () => { throw new Error('The reference read did not start'); };
  wire.read.mockImplementationOnce(() => new Promise((resolve) => { finish = resolve; }));
  const context: ReferenceContext = { document, enabled: true };
  const pending = referenceChoices(() => context, '');
  if (boundary === 'reader') wire.reader += 1;
  if (boundary === 'document') context.document = { ...document, id: 'another-document' };
  if (boundary === 'move') context.document = { ...document, node: { ...document.node, id: 'elsewhere' } };
  if (boundary === 'root') context.document = { ...document, path: document.path.map((one) => ({ ...one, id: 'another-root' })) };
  if (boundary === 'ancestry') context.document = { ...document, path: [...document.path, { ...document.node, id: 'new-parent' }] };
  if (boundary === 'redaction') context.document = { ...document, node: { ...document.node, redacted: true } };
  if (boundary === 'hidden') context.enabled = false;
  finish({ items: [{ id: 'old', title: 'Old native choice' }], next_cursor: null });
  expect(await pending).toEqual([]);
});

it('offers every matching choice from the loaded page instead of dropping those after the eighth', async () => {
  wire.read.mockResolvedValueOnce({ items: Array.from({ length: 16 }, (_, index) => ({ id: `note-${index}`, title: `Note ${index}` })), next_cursor: null });
  const choices = await referenceChoices(() => ({ document, enabled: true }), 'Note');
  expect(choices).toHaveLength(16);
  expect(choices.at(-1)).toMatchObject({ kind: 'document', id: 'note-15' });
});

it('continues a filtered document search past an empty first page without loading the whole collection', async () => {
  const context = { document, enabled: true };
  wire.read.mockResolvedValueOnce({ items: [{ id: 'unrelated', title: 'Other notes' }], next_cursor: 'next-page' });
  const first = await referenceChoices(() => context, 'older');
  expect(wire.read).toHaveBeenCalledTimes(1);
  const more = first.find((one) => one.kind === 'more');
  if (more?.kind !== 'more') throw new Error('An incomplete reference page must offer continuation.');
  wire.read.mockResolvedValueOnce({ items: [{ id: 'older', title: 'Older handbook' }], next_cursor: null });
  expect(await more.load()).toEqual([{ id: 'older', kind: 'document', name: 'Older handbook', hint: 'document' }]);
  expect(wire.read).toHaveBeenLastCalledWith({ queryKey: ['documents', 'topic', 'older', 'next-page'] });
  context.enabled = false;
  expect(await more.load()).toEqual([]);
  expect(wire.read).toHaveBeenCalledTimes(2);
});
