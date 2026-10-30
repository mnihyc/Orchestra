/** A paste remains editable and recoverable while its private transfer is unfinished or failed. */
import { act, fireEvent, screen, waitFor } from '@testing-library/react';
import { useImperativeHandle, useRef, useState } from 'react';
import { beforeEach, expect, it, vi } from 'vitest';
import type * as Uploads from '@/lib/uploads';
import type * as PastedText from '@/lib/pastedText';
import { UploadChips } from '@/features/composer/Attachments';
import { usePastedText } from '@/features/composer/usePastedText';
import type { PastedTextPart } from '@/lib/pastedText';
import type { CodeViewHandle, CodeViewProps } from '@/ui/CodeView';
import { show } from './support';

const calls = vi.hoisted(() => ({ jobs: [] as { file: File; signal: AbortSignal; finish: () => void; fail: () => void }[], cancel: vi.fn(), attached: vi.fn(), published: vi.fn() }));
vi.mock('@/ui/CodeView', () => ({ CodeView: (props: CodeViewProps) => {
  const words = useRef(props.text);
  useImperativeHandle(props.ref, () => ({ getText: () => words.current } as CodeViewHandle));
  return <textarea aria-label={props.label} defaultValue={props.text} onChange={(event) => { words.current = event.target.value; props.onEdit?.(true); props.onSettled?.(words.current); }} />;
} }));
vi.mock('@/lib/uploads', async (load) => ({ ...await load<typeof Uploads>(), UploadTask: class {
  readonly uploadId = null;
  constructor(readonly file: File) {}
  run(signal: AbortSignal) {
    return new Promise((resolve, reject) => {
      calls.jobs.push({ file: this.file, signal, finish: () => resolve({ blob: 'd'.repeat(64), size: this.file.size, name: this.file.name, media_type: 'text/plain' }), fail: () => reject(new Error('Upload interrupted')) });
      signal.addEventListener('abort', () => reject(new DOMException('Upload cancelled', 'AbortError')), { once: true });
    });
  }
  cancel() { calls.cancel(); return Promise.resolve(); }
} }));
vi.mock('@/lib/pastedText', async (load) => ({ ...await load<typeof PastedText>(), pasteWouldScroll: () => true,
  attachPastedText: (_kind: string, _id: string, blob: { blob: string; size: number; name: string }, words: string) => {
    calls.attached(words);
    return Promise.resolve({ type: 'pasted_text', ...blob, path: `.orch/attachments/${blob.name}`, characters: words.length, ascii_characters: words.length });
  },
}));

function Probe() {
  const [text, setText] = useState('');
  const box = useRef<HTMLTextAreaElement>(null);
  const paste = usePastedText({ kind: 'thread', id: 'pending-paste-proof', box, maxLines: 12, text, setText, add: (part: PastedTextPart) => { calls.published(part); }, replace: () => undefined, remove: () => undefined });
  return <><textarea ref={box} aria-label="Message" value={text} onChange={(event) => setText(event.target.value)} /><button onClick={() => paste.paste('Original pending words')}>Paste long text</button><UploadChips queue={paste.queue} /></>;
}
beforeEach(() => { calls.jobs = []; calls.cancel.mockClear(); calls.attached.mockClear(); calls.published.mockClear(); });

it('stops the original transfer on editing and publishes only the replacement words', async () => {
  show(<Probe />); fireEvent.click(screen.getByRole('button', { name: 'Paste long text' }));
  fireEvent.click(await screen.findByRole('button', { name: 'Pasted text' }));
  const editor = await screen.findByRole('textbox', { name: 'Pasted text' });
  expect(editor).toHaveValue('Original pending words');
  fireEvent.change(editor, { target: { value: 'New words while uploading' } });
  await waitFor(() => expect(calls.jobs).toHaveLength(2));
  expect(calls.jobs[0]?.signal.aborted).toBe(true);
  await act(async () => { calls.jobs[0]?.finish(); await Promise.resolve(); });
  expect(calls.attached).not.toHaveBeenCalled(); expect(calls.published).not.toHaveBeenCalled();
  await act(async () => { calls.jobs[1]?.finish(); await Promise.resolve(); });
  await waitFor(() => expect(calls.attached).toHaveBeenCalledExactlyOnceWith('New words while uploading'));
  expect(calls.published).toHaveBeenCalledOnce();
});

it.each([false, true])('Paste as text recovers words and drops the private transfer after failure=%s', async (failed) => {
  show(<Probe />); fireEvent.click(screen.getByRole('button', { name: 'Paste long text' }));
  await waitFor(() => expect(calls.jobs).toHaveLength(1));
  if (failed) await act(async () => { calls.jobs[0]?.fail(); await Promise.resolve(); });
  fireEvent.click(screen.getByRole('button', { name: 'Paste as text' }));
  await waitFor(() => expect(screen.getByRole('textbox', { name: 'Message' })).toHaveValue('Original pending words'));
  expect(calls.cancel).toHaveBeenCalled();
  expect(screen.queryByRole('button', { name: 'Pasted text' })).not.toBeInTheDocument();
  expect(calls.attached).not.toHaveBeenCalled(); expect(calls.published).not.toHaveBeenCalled();
});
