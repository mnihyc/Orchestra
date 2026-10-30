/** The existing attachment chip opens pasted text editable in a draft and read-only in history. */
import { fireEvent, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { useImperativeHandle, useRef } from 'react';
import type { CodeViewHandle, CodeViewProps } from '@/ui/CodeView';
import { AttachmentChips } from '@/features/composer/Attachments';
import { rememberPaste, type PastedTextPart } from '@/lib/pastedText';
import { show } from './support';

vi.mock('@/ui/CodeView', () => ({ CodeView: (props: CodeViewProps) => {
  const words = useRef(props.text);
  useImperativeHandle(props.ref, () => ({ getText: () => words.current } as CodeViewHandle));
  return <textarea aria-label={props.label} defaultValue={props.text} readOnly={props.readOnly} onChange={(event) => { words.current = event.target.value; props.onEdit?.(true); props.onSettled?.(words.current); }} />;
} }));
const paste: PastedTextPart = { type: 'pasted_text', blob: 'b'.repeat(64), name: 'pasted.txt', path: '.orch/attachments/pasted.txt', size: 12000, characters: 12000, ascii_characters: 12000 };

describe('pasted text chip', () => {
  it('says where and how it will be sent, opens editable and can put its words back inline', async () => {
    rememberPaste(paste.blob, 'original text');
    const edit = vi.fn(), asText = vi.fn(), dirty = vi.fn();
    show(<AttachmentChips parts={[paste]} destination={(part) => 'path' in part ? `→ ${part.path} · when sent` : null} pasteMode={() => false}
      onPasteAsText={asText} onEditPaste={edit} onPasteDirty={dirty} onRemove={vi.fn()} />);
    expect(screen.getByText('12,000 characters')).toBeInTheDocument();
    expect(screen.getByText('Sent as a file')).toBeInTheDocument();
    expect(screen.getByText('→ .orch/attachments/pasted.txt · when sent')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Pasted text' }));
    const editor = await screen.findByRole('textbox', { name: 'Pasted text' });
    expect(editor).not.toHaveAttribute('readonly');
    fireEvent.change(editor, { target: { value: 'edited text' } });
    expect(edit).toHaveBeenCalledWith(0, 'edited text');
    expect(dirty).toHaveBeenCalledWith(paste, true);
    fireEvent.click(screen.getByRole('button', { name: 'Paste as text' }));
    expect(asText).toHaveBeenCalledWith(0, 'edited text');
  });
  it('a history chip expands read-only and has no draft action', async () => {
    rememberPaste(paste.blob, 'sent text');
    show(<AttachmentChips parts={[paste]} />);
    expect(screen.queryByRole('button', { name: 'Paste as text' })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Remove Pasted text' })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Pasted text' }));
    expect(await screen.findByRole('textbox', { name: 'Pasted text' })).toHaveAttribute('readonly');
  });
});
