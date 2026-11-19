/** Pending files can be retried or removed, and a model without native images receives them by path. */
import { fireEvent, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';

import { AttachmentChips, UploadChips } from '@/features/composer/Attachments';
import type { ContentPart } from '@/generated/api-types';
import { show } from './support';

describe('attachment chips', () => {
  it('names a failed file and lets the person retry it or remove it', () => {
    const retry = vi.fn();
    const remove = vi.fn();
    show(<UploadChips queue={{ rows: [{ id: 'file', file: new File(['data'], 'report.csv'), received: 2, phase: 'failed', error: new Error('failed') }], retry, remove }} />);
    expect(screen.getByText('report.csv')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Retry report.csv' }));
    expect(retry).toHaveBeenCalledWith('file');
    fireEvent.click(screen.getByRole('button', { name: 'Remove report.csv' }));
    expect(remove).toHaveBeenCalledWith('file');
  });

  it('says the model receives the image as a file and retains Change model and Remove', () => {
    const change = vi.fn();
    const remove = vi.fn();
    const parts: ContentPart[] = [{ type: 'image', blob: 'a'.repeat(64), name: 'screen.png', media_type: 'image/png' }];
    show(<AttachmentChips parts={parts} onRemove={remove} imageIncompatible onChangeModel={change} />);
    expect(screen.getByRole('status')).toHaveTextContent('This model receives the image as a file');
    fireEvent.click(screen.getByRole('button', { name: 'Change model' }));
    expect(change).toHaveBeenCalledOnce();
    fireEvent.click(screen.getByRole('button', { name: 'Remove screen.png' }));
    expect(remove).toHaveBeenCalledWith(0);
  });
});
