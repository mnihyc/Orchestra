/** Retained document panes must survive React deactivating and restoring their effects. */
import { act, cleanup, screen, waitFor } from '@testing-library/react';
import { Activity, useCallback, useState, useSyncExternalStore } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Documents from '@/api/documents';
import { BlockEditor, type EditorApi } from '@/features/documents/Editor';
import { DocumentMarksProvider, retireDocumentMarks } from '@/features/documents/marks';
import { clearViewOpenings, showViewOpening, ViewOpening } from '@/features/workbench/viewOpening';
import type { Document, DocumentBlock } from '@/generated/api-types';
import { documentWriter } from '@/realtime/stores/document';
import { World } from '../doubles/world';
import { show } from './support';

const calls = vi.hoisted(() => ({ send: vi.fn<typeof Documents.postOps>(), list: vi.fn<() => Promise<{ items: never[]; next_cursor: null }>>() }));
vi.mock('@/api/documents', async (load) => {
  const actual = await load<typeof Documents>();
  return { ...actual, postOps: calls.send,
    documentsOfNodeQuery: (...args: Parameters<typeof actual.documentsOfNodeQuery>) => ({
      ...actual.documentsOfNodeQuery(...args), queryFn: calls.list,
    }),
  };
});

let doc: Document, block: DocumentBlock;
const ready = vi.fn<(api: EditorApi | null) => void>();
function Pane({ mode, fixed = false, editable = true }: { mode: 'visible' | 'hidden'; fixed?: boolean; editable?: boolean }) {
  return <Activity mode={mode}><DocumentMarksProvider owner="lifecycle-pane" documentId={doc.id}>
    <BlockEditor document={doc} editable={editable} inspection={fixed} fixed={fixed ? [block] : undefined} onReady={ready} />
  </DocumentMarksProvider></Activity>;
}
function current(): EditorApi {
  const api = ready.mock.calls.at(-1)?.[0];
  if (api === null || api === undefined) throw new Error('The shown document must expose a live editor.');
  return api;
}
function WorkbenchPane() {
  const [api, setApi] = useState<EditorApi | null>(null);
  const capture = useCallback((next: EditorApi | null) => { ready(next); setApi(next); }, []);
  const held = useSyncExternalStore(
    (listener) => api?.sync?.save.subscribe(listener) ?? (() => undefined),
    () => api?.sync?.hasUnsent() ?? false,
  );
  return <ViewOpening view="editor-lifetime" route="editor-route" hold={held}>
    <DocumentMarksProvider owner="lifecycle-pane" documentId={doc.id}>
      <BlockEditor document={doc} editable onReady={capture} />
    </DocumentMarksProvider>
  </ViewOpening>;
}
async function settleHiddenEditor() {
  // The installed editor schedules destruction at 1ms after effect cleanup; cross that boundary.
  await act(async () => { await new Promise((resolve) => setTimeout(resolve, 2)); });
}
beforeEach(() => {
  ready.mockClear(); calls.send.mockReset(); calls.list.mockReset(); calls.list.mockResolvedValue({ items: [], next_cursor: null }); documentWriter.clear();
  const world = new World(), person = world.people.get('ada');
  const stored = [...world.documents.docs.values()].find((value) => value.title === 'Rollout plan');
  if (person === undefined || stored === undefined) throw new Error('The readable document fixture is required.');
  doc = world.documents.full(person, stored);
  const paragraph = world.documents.sync(person, stored, null).blocks.find((value) => value.type === 'paragraph');
  if (paragraph === undefined) throw new Error('The document fixture must include a paragraph.');
  block = { ...paragraph, parent_id: null, position: '1', content: [{ type: 'text', text: 'Saved passage' }] };
  documentWriter.reset(doc.id, { document: doc, blocks: [block], discussions: [] }, doc.seq);
});
afterEach(() => { cleanup(); retireDocumentMarks(); documentWriter.clear(); clearViewOpenings(); });

it.each([false, true])('restores the real editor after two hidden lifetimes (fixed snapshot: %s)', async (fixed) => {
  const drawn = show(<Pane mode="visible" fixed={fixed} editable={!fixed} />);
  await screen.findByRole('textbox', { name: 'Document text' });
  for (let cycle = 0; cycle < 2; cycle += 1) {
    expect(current().editor.state.doc.textContent).toBe('Saved passage');
    drawn.rerender(<Pane mode="hidden" fixed={fixed} editable={!fixed} />);
    await settleHiddenEditor();
    drawn.rerender(<Pane mode="visible" fixed={fixed} editable={!fixed} />);
    await waitFor(() => expect(current().editor.isDestroyed).toBe(false));
    expect(screen.getByRole('textbox', { name: 'Document text' })).toHaveTextContent('Saved passage');
    expect(current().editor.isEditable).toBe(!fixed);
    expect(current().sync === null).toBe(fixed);
  }
  expect(calls.send).not.toHaveBeenCalled();
  drawn.unmount();
  expect(ready.mock.calls.at(-1)?.[0]).toBeNull();
});

it('reopens from the latest confirmed blocks, including rows received while the pane was hidden', async () => {
  const drawn = show(<Pane mode="visible" />);
  await screen.findByRole('textbox', { name: 'Document text' });
  const saved = { ...block, seq_updated: doc.seq + 1, content: [{ type: 'text' as const, text: 'Confirmed before hiding' }] };
  act(() => documentWriter.apply(doc.id, { blocks: [saved] }, saved.seq_updated));
  expect(current().editor.state.doc.textContent).toBe('Confirmed before hiding');
  drawn.rerender(<Pane mode="hidden" />);
  await settleHiddenEditor();
  const newer = { ...saved, seq_updated: saved.seq_updated + 1, content: [{ type: 'text' as const, text: 'Confirmed while hidden' }] };
  act(() => documentWriter.apply(doc.id, { blocks: [newer] }, newer.seq_updated));
  drawn.rerender(<Pane mode="visible" />);
  await waitFor(() => expect(current().editor.state.doc.textContent).toBe('Confirmed while hidden'));
  expect(calls.send).not.toHaveBeenCalled();
});

it('restores a live reader without edit authority, then follows a new editing grant', async () => {
  const drawn = show(<Pane mode="visible" />);
  await screen.findByRole('textbox', { name: 'Document text' });
  drawn.rerender(<Pane mode="hidden" editable={false} />);
  await settleHiddenEditor();
  drawn.rerender(<Pane mode="visible" editable={false} />);
  await waitFor(() => expect(current().editor.isEditable).toBe(false));
  expect(screen.getByRole('textbox', { name: 'Document text' })).toHaveAttribute('contenteditable', 'false');
  drawn.rerender(<Pane mode="visible" />);
  await waitFor(() => expect(current().editor.isEditable).toBe(true));
  expect(current().editor.state.doc.textContent).toBe('Saved passage');
  expect(calls.send).not.toHaveBeenCalled();
});

it('retains a pending save through the workbench hold, then sleeps and restores the acknowledged content', async () => {
  let finish!: (reply: Awaited<ReturnType<typeof Documents.postOps>>) => void;
  calls.send.mockImplementation(() => new Promise((resolve) => { finish = resolve; }));
  let release = showViewOpening('editor-lifetime', 'editor-route');
  show(<WorkbenchPane />);
  await screen.findByRole('textbox', { name: 'Document text' });
  const first = current();
  act(() => { first.editor.commands.insertContentAt(2, 'Pending '); first.sync?.flush(); });
  expect(first.sync?.hasUnsent()).toBe(true);
  expect(calls.send).toHaveBeenCalledTimes(1);
  await act(async () => { release(); await Promise.resolve(); });
  await settleHiddenEditor();
  expect(first.editor.isDestroyed).toBe(false);
  expect(current()).toBe(first);
  expect(first.editor.state.doc.textContent).toBe('Pending Saved passage');
  const saved = { ...block, seq_updated: doc.seq + 1, content: [{ type: 'text' as const, text: 'Pending Saved passage' }] };
  await act(async () => { finish({ document: { ...doc, seq: saved.seq_updated }, blocks: [saved], seq: saved.seq_updated }); await Promise.resolve(); });
  await settleHiddenEditor();
  expect(first.sync?.hasUnsent()).toBe(false);
  expect(first.editor.isDestroyed).toBe(true);
  await act(async () => { release = showViewOpening('editor-lifetime', 'editor-route'); await Promise.resolve(); });
  await waitFor(() => expect(current().editor.state.doc.textContent).toBe('Pending Saved passage'));
  expect(current().sync?.hasUnsent()).toBe(false);
  expect(calls.send).toHaveBeenCalledTimes(1);
  await act(async () => { release(); await Promise.resolve(); });
});


it.each([false, true])('does not browse a shared document’s redacted parent for suggestions (editable: %s)', async (editable) => {
  doc = { ...doc, node: { ...doc.node, name: null, redacted: true } };
  show(<Pane mode="visible" editable={editable} />);
  await screen.findByRole('textbox', { name: 'Document text' });
  expect(calls.list).not.toHaveBeenCalled();
  expect(current().editor.isEditable).toBe(editable);
});

it('loads parent-document suggestions only for an editor who can browse that parent', async () => {
  const drawn = show(<Pane mode="visible" editable={false} />);
  await screen.findByRole('textbox', { name: 'Document text' });
  expect(calls.list).not.toHaveBeenCalled();
  drawn.rerender(<Pane mode="visible" />);
  await waitFor(() => expect(calls.list).toHaveBeenCalledTimes(1));
});
