/** Actual editors own their link field and block pointer; a neighbor cannot redirect a document action. */
import { Editor } from '@tiptap/core';
import { expect, it, vi } from 'vitest';
import { createStore } from 'zustand';
import { bindEditorMarks, scrollToDocumentBlock, useMarks } from '@/features/documents/marks';
import { askDocumentFiles } from '@/features/documents/Media';
import { schemaExtensions } from '@/features/documents/schema';
import { linkFieldOf } from '@/features/documents/TextBar';
import { blockPointerOf, blockEntries } from '@/features/documents/Handle';
import { i18n } from '@/i18n';

it('keeps link opening and block deletion attached to the addressed editor', () => {
  const one = new Editor({ extensions: schemaExtensions(), content: { type: 'doc', content: [{ type: 'block', attrs: { id: 'one' }, content: [{ type: 'paragraph', content: [{ type: 'text', text: 'First document' }] }] }] } });
  const two = new Editor({ extensions: schemaExtensions(), content: { type: 'doc', content: [{ type: 'block', attrs: { id: 'two' }, content: [{ type: 'paragraph', content: [{ type: 'text', text: 'Second document' }] }] }] } });
  try {
    linkFieldOf(one).setState({ open: true }); expect(linkFieldOf(two).getState().open).toBe(false);
    const target = { node: one.state.doc.child(0), pos: 0 };
    blockPointerOf(one).setState({ target }); expect(blockPointerOf(two).getState().target).toBeNull();
    const original = two.getJSON();
    const remove = blockEntries(one, target, i18n.t, 'document-one').find((entry) => entry.id === 'delete');
    if (remove === undefined || !('onSelect' in remove)) throw new Error('The actual block menu must offer delete');
    remove.onSelect(); expect(two.getJSON()).toEqual(original); expect(one.getText()).not.toContain('First document');
  } finally { one.destroy(); two.destroy(); }
});

it('a media action uses its actual editor’s picker when the same document is open twice', () => {
  const make = () => new Editor({ extensions: schemaExtensions(), content: { type: 'doc', content: [{ type: 'block', attrs: { id: 'shared-block' }, content: [{ type: 'paragraph', content: [{ type: 'text', text: 'A shared paragraph' }] }] }] } });
  const first = make(), second = make();
  const mount = (editor: Editor) => {
    const root = document.createElement('div'); root.dataset.document = 'same-document';
    const input = document.createElement('input'); input.type = 'file'; input.dataset.documentFiles = '';
    (editor.storage as { orchDoc?: { id: string } }).orchDoc = { id: 'same-document' };
    root.append(input, editor.view.dom); document.body.append(root); return { root, input };
  };
  const a = mount(first), b = mount(second);
  try {
    const one = vi.spyOn(a.input, 'click'), two = vi.spyOn(b.input, 'click');
    first.commands.setTextSelection(3); second.commands.setTextSelection(9);
    askDocumentFiles(second, true);
    expect(one).not.toHaveBeenCalled(); expect(two).toHaveBeenCalledExactlyOnceWith();
    expect(b.input.accept).toBe('image/*'); expect(a.input.accept).toBe('');
    expect(first.state.selection.from).toBe(3); expect(second.state.selection.from).toBe(9);
    b.root.remove(); askDocumentFiles(second);
    expect(one).not.toHaveBeenCalled(); expect(two).toHaveBeenCalledTimes(1);
  } finally { first.destroy(); second.destroy(); a.root.remove(); b.root.remove(); }
});

it('a comment anchor scrolls only its owning editor when a document is open twice', () => {
  const make = () => new Editor({ extensions: schemaExtensions(), content: { type: 'doc', content: [{ type: 'block', attrs: { id: 'same-block' }, content: [{ type: 'paragraph', content: [{ type: 'text', text: 'Repeated document' }] }] }] } });
  const first = make(), second = make();
  document.body.append(first.view.dom, second.view.dom);
  const firstMarks = createStore(() => useMarks.getInitialState()), secondMarks = createStore(() => useMarks.getInitialState());
  const freeFirst = bindEditorMarks(first, firstMarks), freeSecond = bindEditorMarks(second, secondMarks);
  try {
    const a = first.view.dom.querySelector('[data-id="same-block"]'), b = second.view.dom.querySelector('[data-id="same-block"]');
    if (a === null || b === null) throw new Error('Both actual editors must contain the shared block');
    const left = vi.spyOn(a, 'scrollIntoView'), right = vi.spyOn(b, 'scrollIntoView');
    scrollToDocumentBlock('same-block', secondMarks);
    expect(left).not.toHaveBeenCalled(); expect(right).toHaveBeenCalledExactlyOnceWith({ block: 'center', behavior: 'smooth' });
    freeSecond(); scrollToDocumentBlock('same-block', secondMarks);
    expect(left).not.toHaveBeenCalled(); expect(right).toHaveBeenCalledTimes(1);
  } finally { freeFirst(); freeSecond(); first.view.dom.remove(); second.view.dom.remove(); first.destroy(); second.destroy(); }
});
