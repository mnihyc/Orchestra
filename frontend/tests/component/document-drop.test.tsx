/** A delayed file drop keeps the pointer's document block even when the caret moves before upload finishes. */
import { act, fireEvent, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import type * as Documents from '@/api/documents';
import { BlockEditor, type EditorApi } from '@/features/documents/Editor';
import { DocumentMarksProvider, retireDocumentMarks } from '@/features/documents/marks';
import { documentWriter } from '@/realtime/stores/document';
import type { DocumentFile } from '@/generated/api-types';
import { World } from '../doubles/world';
import { show } from './support';

const calls = vi.hoisted(() => ({ add: vi.fn(), ready: vi.fn<(api: EditorApi | null) => void>() }));
vi.mock('@/lib/useUploads', () => ({ useUploads: () => ({ rows: [], blocked: false, add: calls.add, remove: vi.fn(), retry: vi.fn() }) }));
vi.mock('@/features/documents/sync', () => ({ createSync: () => ({ destroy: () => undefined }), syncOf: () => null }));
vi.mock('@/api/documents', async (load) => {
  const actual = await load<typeof Documents>();
  return { ...actual, documentsOfNodeQuery: (...args: Parameters<typeof actual.documentsOfNodeQuery>) => ({ ...actual.documentsOfNodeQuery(...args), queryFn: () => Promise.resolve({ items: [], next_cursor: null }) }) };
});
afterEach(() => { retireDocumentMarks(); documentWriter.clear(); calls.add.mockReset(); calls.ready.mockReset(); });

it('inserts after the hit block rather than the current cursor and never folds document text', async () => {
  const world = new World(), person = world.people.get('ada');
  const stored = [...world.documents.docs.values()].find((value) => value.title === 'Rollout plan');
  if (person === undefined || stored === undefined) throw new Error('The document fixture is required');
  const doc = world.documents.full(person, stored);
  const first = world.documents.sync(person, stored, null).blocks.find((value) => value.type === 'paragraph');
  if (first === undefined) throw new Error('The paragraph fixture is required');
  const target = { ...first, id: crypto.randomUUID(), position: '2', parent_id: null, content: [{ type: 'text' as const, text: 'Pointer target' }] };
  const initial = { ...first, position: '1', parent_id: null, content: [{ type: 'text' as const, text: 'Caret starts here' }] };
  documentWriter.reset(doc.id, { document: doc, blocks: [initial, target], discussions: [] }, doc.seq);
  show(<DocumentMarksProvider owner="drop-point-proof" documentId={doc.id}><BlockEditor document={doc} editable onReady={calls.ready} /></DocumentMarksProvider>);
  const text = await screen.findByRole('textbox', { name: 'Document text' });
  await waitFor(() => expect(calls.ready.mock.calls.at(-1)?.[0]?.editor).toBeDefined());
  const api = calls.ready.mock.calls.at(-1)?.[0]; if (api === undefined || api === null) throw new Error('The editor must be ready');
  let hit = 0; api.editor.state.doc.descendants((node, pos) => { if (node.attrs.id === target.id) { hit = pos + 2; return false; } return true; });
  const point = vi.spyOn(api.editor.view, 'posAtCoords').mockReturnValue({ pos: hit, inside: hit });
  fireEvent.drop(text, { dataTransfer: { types: ['Files'], files: [new File(['file'], 'note.txt')], items: [], getData: () => '' }, clientX: 40, clientY: 90 });
  await waitFor(() => expect(calls.add).toHaveBeenCalledTimes(1));
  expect(point).toHaveBeenCalled();
  const publish = calls.add.mock.calls[0]?.[1] as ((file: DocumentFile) => void) | undefined;
  if (publish === undefined) throw new Error('The upload must capture its insertion point');
  const file: DocumentFile = { id: crypto.randomUUID(), name: 'note.txt', size: 4, media_type: 'text/plain', is_text: true, added_by: null, added_at: new Date().toISOString() };
  act(() => { api.editor.commands.setTextSelection(2); publish(file); });
  const blocks = api.editor.getJSON().content;
  expect(blocks[0]?.attrs?.id).toBe(initial.id); expect(blocks[1]?.attrs?.id).toBe(target.id);
  expect(JSON.stringify(blocks[2])).toContain(file.id);
  expect(screen.queryByRole('button', { name: 'Pasted text' })).not.toBeInTheDocument();
});
