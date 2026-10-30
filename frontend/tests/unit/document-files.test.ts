/** Files finished after more typing go beside the block where they were chosen, and a reader cannot insert them. */
import type { Editor } from '@tiptap/core';
import { EditorState, type Transaction } from '@tiptap/pm/state';
import { describe, expect, it } from 'vitest';

import { insertDocumentFile } from '@/features/documents/Media';
import { docToDrafts, orchSchema } from '@/features/documents/schema';
import type { DocumentFile } from '@/generated/api-types';

const file: DocumentFile = { id: 'file', name: 'diagram.png', media_type: 'image/png', size: 8, is_text: false, added_by: null, added_at: '2026-10-14T10:00:00.000Z' };

function page(editable = true) {
  const schema = orchSchema();
  let state = EditorState.create({ doc: schema.nodeFromJSON({ type: 'doc', content: [
    { type: 'block', attrs: { id: 'chosen' }, content: [{ type: 'paragraph', content: [{ type: 'text', text: 'The plan' }] }] },
    { type: 'block', attrs: { id: 'later' }, content: [{ type: 'paragraph', content: [{ type: 'text', text: 'Typed while the image uploads' }] }] },
  ] }) });
  const editor = { schema, get state() { return state; }, isEditable: editable, isDestroyed: false, view: { dispatch: (tr: Transaction) => { state = state.apply(tr); } } } as unknown as Editor;
  return { editor, blocks: () => docToDrafts(state.doc) };
}

describe('document file insertion', () => {
  it('keeps intervening text and places the image beside its original block with a durable file reference', () => {
    const doc = page();
    insertDocumentFile(doc.editor, file, 'chosen');
    const blocks = doc.blocks();
    expect(blocks.map((block) => block.type)).toEqual(['paragraph', 'image', 'paragraph']);
    expect(blocks[1]?.props.file_id).toBe(file.id);
    expect(blocks[2]?.content).toEqual([{ type: 'text', text: 'Typed while the image uploads' }]);
  });

  it('leaves the document alone when the reader lost editing access while the upload finished', () => {
    const doc = page(false);
    expect(insertDocumentFile(doc.editor, file, 'chosen')).toBeNull();
    expect(doc.blocks().map((block) => block.id)).toEqual(['chosen', 'later']);
  });
});
