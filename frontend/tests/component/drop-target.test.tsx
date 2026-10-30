/** Only accepting places show a destination, and leaving or dropping clears it. */
import { fireEvent, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';

import { DropTarget } from '@/features/composer/DropTarget';
import { show } from './support';

describe('drop targets', () => {
  it.each(['Attach to your message', 'Attach to this chat', 'Add to this document'])('shows %s only while a file is over the place', (label) => {
    const drop = vi.fn();
    show(<DropTarget label={label} onDrop={drop}><textarea aria-label="Editor" /></DropTarget>);
    const editor = screen.getByRole('textbox');
    expect(screen.queryByRole('status')).not.toBeInTheDocument();
    const file = new File(['a'], 'a.txt'); const dataTransfer = { types: ['Files'], files: [file] };
    fireEvent.dragOver(editor, { dataTransfer });
    expect(screen.getByRole('status')).toHaveTextContent(label);
    fireEvent.drop(editor, { dataTransfer, clientX: 10, clientY: 20 });
    expect(drop).toHaveBeenCalledOnce();
    expect(screen.queryByRole('status')).not.toBeInTheDocument();
  });

  it('shows no accepting outline for ordinary text or a place that cannot take files', () => {
    const drop = vi.fn();
    const view = show(<DropTarget label="Attach to this chat" onDrop={drop}><textarea aria-label="Editor" /></DropTarget>);
    fireEvent.dragOver(screen.getByRole('textbox'), { dataTransfer: { types: ['text/plain'] } });
    expect(screen.queryByRole('status')).not.toBeInTheDocument();
    view.rerender(<DropTarget disabled label="Attach to this chat" onDrop={drop}><textarea aria-label="Editor" /></DropTarget>);
    fireEvent.drop(screen.getByRole('textbox'), { dataTransfer: { types: ['Files'] } });
    expect(drop).not.toHaveBeenCalled();
    expect(screen.queryByRole('status')).not.toBeInTheDocument();
  });
});
