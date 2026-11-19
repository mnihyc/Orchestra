/** Caret reference reads belong to the current visible editor and reader. */
import { beforeEach, expect, it, vi } from 'vitest';
import { referenceChoices, type ReferenceContext } from '@/features/documents/referenceChoices';

const wire = vi.hoisted(() => ({ reader: 1, read: vi.fn<() => Promise<{ pages: { items: { id: string; title: string }[] }[] }>>() }));
vi.mock('@/api/client', () => ({ client: { session: { getState: () => ({ readerVersion: wire.reader }) } } }));
vi.mock('@/api/query', () => ({ queryClient: { infiniteQuery: wire.read } }));
vi.mock('@/api/documents', () => ({ documentsOfNodeQuery: (id: string) => ({ queryKey: ['documents', id] }) }));
vi.mock('@/realtime/stores/tree', () => ({ useTreeStore: { getState: () => ({ byOrg: new Map([
  ['root', { nodes: new Map([['topic', { id: 'topic', kind: 'topic', name: 'Visible topic', deleted: false }]]) }],
]) }) } }));

const document: ReferenceContext['document'] = { id: 'current', node: { id: 'topic', kind: 'topic', name: 'Topic', redacted: false },
  path: [{ id: 'root', kind: 'org', name: 'Root', redacted: false }] };
beforeEach(() => { wire.reader = 1; wire.read.mockReset().mockResolvedValue({ pages: [{ items: [{ id: 'other', title: 'Visible notes' }] }] }); });

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
  let finish: (value: { pages: { items: { id: string; title: string }[] }[] }) => void = () => { throw new Error('The reference read did not start'); };
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
  finish({ pages: [{ items: [{ id: 'old', title: 'Old native choice' }] }] });
  expect(await pending).toEqual([]);
});
