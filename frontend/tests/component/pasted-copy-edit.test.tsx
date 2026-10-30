/** Editing one pasted copy changes its private destination only and leaves the immutable blob cache alone. */
import { fireEvent, screen, waitFor } from '@testing-library/react';
import { useImperativeHandle, useRef, useState } from 'react';
import { expect, it, vi } from 'vitest';
import type * as Uploads from '@/lib/uploads';
import type * as PastedText from '@/lib/pastedText';
import { AttachmentChips, UploadChips } from '@/features/composer/Attachments';
import { usePastedText } from '@/features/composer/usePastedText';
import { pasteKey, rememberedPaste, rememberPaste, type PastedTextPart } from '@/lib/pastedText';
import type { CodeViewHandle, CodeViewProps } from '@/ui/CodeView';
import { show } from './support';

vi.mock('@/ui/CodeView', () => ({ CodeView: (props: CodeViewProps) => {
  const words = useRef(props.text);
  useImperativeHandle(props.ref, () => ({ getText: () => words.current } as CodeViewHandle));
  return <textarea aria-label={props.label} defaultValue={props.text} readOnly={props.readOnly} onChange={(event) => { words.current = event.target.value; props.onEdit?.(true); props.onSettled?.(words.current); }} />;
} }));
vi.mock('@/lib/uploads', async (load) => ({ ...await load<typeof Uploads>(), releaseUpload: () => Promise.resolve(), UploadTask: class {
  readonly uploadId = null;
  constructor(readonly file: File) {}
  run(_signal: AbortSignal, progress: (received: number) => void) { progress(this.file.size); return Promise.resolve({ blob: 'c'.repeat(64), size: this.file.size, name: this.file.name, media_type: 'text/plain' }); }
  cancel() { return Promise.resolve(); }
} }));
vi.mock('@/lib/pastedText', async (load) => {
  const actual = await load<typeof PastedText>();
  return { ...actual, attachPastedText: (_kind: string, id: string, blob: { blob: string; size: number; name: string }, words: string, _signal: AbortSignal, path?: string) => {
    actual.rememberPaste(blob.blob, words);
    return Promise.resolve({ type: 'pasted_text', ...blob, ...(id === 'sent-pasted-copy' ? { name: 'first (2).txt', path: '.orch/attachments/first (2).txt' } : { path: path ?? '' }), characters: words.length, ascii_characters: words.length });
  } };
});

const original: PastedTextPart = { type: 'pasted_text', blob: 'b'.repeat(64), name: 'first.txt', path: '.orch/attachments/first.txt', size: 8, characters: 8, ascii_characters: 8 };
let held: PastedTextPart[] = [];
function Probe({ id = 'separate-pasted-copies' }: { id?: string }) {
  const [parts, setParts] = useState<PastedTextPart[]>([original, { ...original, name: 'second.txt', path: '.orch/attachments/second.txt' }]);
  held = parts;
  const box = useRef<HTMLTextAreaElement>(null);
  const remove = (index: number) => { const part = parts[index]; if (part !== undefined) paste.cancel(part); setParts((before) => before.filter((_, at) => at !== index)); };
  const paste = usePastedText({ kind: 'thread', id, box, maxLines: 12, text: '', setText: () => undefined,
    add: (part) => setParts((before) => [...before, part]), replace: (before, part) => setParts((parts) => parts.map((current) => pasteKey(current) === pasteKey(before) ? part : current)), remove });
  return <><UploadChips queue={paste.queue} /><AttachmentChips parts={parts} onRemove={remove} readPaste={paste.read} pastedIdentity={paste.identity} onPasteDirty={paste.markDirty} editText={paste.editText} pasteQueue={paste.queue} onEditPaste={(index, words) => { const part = parts[index]; if (part !== undefined) paste.edit(part, words); }} /></>;
}

it('does not overwrite another copy that has exactly the same original bytes', async () => {
  rememberPaste(original.blob, 'original'); show(<Probe />);
  fireEvent.click(screen.getAllByRole('button', { name: 'Pasted text' })[0]!);
  fireEvent.change(await screen.findByRole('textbox', { name: 'Pasted text' }), { target: { value: 'changed first copy' } });
  await waitFor(() => expect(held[0]?.blob).toBe('c'.repeat(64)));
  expect(held[1]?.blob).toBe(original.blob);
  expect(rememberedPaste(original.blob)).toBe('original');
  expect(screen.queryByRole('list', { name: 'Files being attached' })).not.toBeInTheDocument();
  fireEvent.click(screen.getAllByRole('button', { name: 'Pasted text' })[1]!);
  const editors = await screen.findAllByRole('textbox', { name: 'Pasted text' });
  expect(editors[1]).toHaveValue('original');
});

it('removing an edited copy never moves its open editor onto the next identical blob', async () => {
  rememberPaste(original.blob, 'original'); show(<Probe />);
  fireEvent.click(screen.getAllByRole('button', { name: 'Pasted text' })[0]!);
  const editor = await screen.findByRole('textbox', { name: 'Pasted text' });
  fireEvent.change(editor, { target: { value: 'Private words that belong only to the removed copy' } });
  fireEvent.click(screen.getAllByRole('button', { name: 'Remove Pasted text' })[0]!);
  expect(screen.queryByRole('textbox', { name: 'Pasted text' })).not.toBeInTheDocument();
  expect(screen.getAllByRole('button', { name: 'Pasted text' })).toHaveLength(1);
  fireEvent.click(screen.getByRole('button', { name: 'Pasted text' }));
  expect(await screen.findByRole('textbox', { name: 'Pasted text' })).toHaveValue('original');
  expect(held).toEqual([{ ...original, name: 'second.txt', path: '.orch/attachments/second.txt' }]);
});

it('keeps the sent-paste editor open when saving allocates its new private destination', async () => {
  rememberPaste(original.blob, 'original'); show(<Probe id="sent-pasted-copy" />);
  fireEvent.click(screen.getAllByRole('button', { name: 'Pasted text' })[0]!);
  const editor = await screen.findByRole('textbox', { name: 'Pasted text' });
  fireEvent.change(editor, { target: { value: 'Edited sent words' } });
  await waitFor(() => expect(held[0]?.path).toBe('.orch/attachments/first (2).txt'));
  expect(screen.getByRole('textbox', { name: 'Pasted text' })).toBe(editor);
  expect(editor).toHaveValue('Edited sent words');
  expect(screen.getAllByRole('button', { name: 'Pasted text' })[0]).toHaveAttribute('aria-expanded', 'true');
  expect(held[1]?.blob).toBe(original.blob);
});
