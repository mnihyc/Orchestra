/** Edit keeps the actual content parts and names the precise running-turn decision. */
import { fireEvent, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { PromptEditForm } from '@/features/timeline/PromptEditForm';
import type { ContentPart } from '@/generated/api-types';
import { show } from './support';

const parts: ContentPart[] = [{ type: 'text', text: 'Original message' },
  { type: 'file', blob: 'retained-blob', name: 'report.pdf', size: 12, path: 'input/report.pdf' }];

it('submits edited words with the existing attachment and no running-turn policy when idle', async () => {
  const user = userEvent.setup(), submit = vi.fn();
  show(<PromptEditForm parts={parts} activeTurnId={null} onSubmit={submit} onCancel={vi.fn()} />);
  const input = screen.getByRole('textbox', { name: 'Edit message' });
  await user.clear(input); await user.type(input, 'Revised message');
  await user.click(screen.getByRole('button', { name: 'Send edit' }));
  expect(submit).toHaveBeenCalledExactlyOnceWith([{ type: 'text', text: 'Revised message' }, parts[1]], undefined);
  expect(parts[0]).toEqual({ type: 'text', text: 'Original message' });
});

it('waits or interrupts only the exact displayed running turn', async () => {
  const user = userEvent.setup(), submit = vi.fn();
  show(<PromptEditForm parts={parts} activeTurnId="current-turn" onSubmit={submit} onCancel={vi.fn()} />);
  await user.click(screen.getByRole('button', { name: 'Wait and send edit' }));
  expect(submit).toHaveBeenLastCalledWith(parts, { turn_id: 'current-turn', action: 'wait' });
  await user.click(screen.getByRole('button', { name: 'Interrupt and send edit' }));
  expect(submit).toHaveBeenLastCalledWith(parts, { turn_id: 'current-turn', action: 'interrupt' });
  expect(submit).toHaveBeenCalledTimes(2);
});

it('keeps IME input local and makes an unavailable edit readable without accepting it', () => {
  const submit = vi.fn(), cancel = vi.fn();
  show(<PromptEditForm parts={parts} activeTurnId={null} unavailable="Your access changed." onSubmit={submit} onCancel={cancel} />);
  const input = screen.getByRole('textbox', { name: 'Edit message' });
  fireEvent.keyDown(input, { key: 'Enter', ctrlKey: true, isComposing: true });
  expect(submit).not.toHaveBeenCalled();
  expect(screen.getByRole('button', { name: 'Send edit' })).toBeDisabled();
  expect(screen.getByText('Your access changed.')).toBeVisible();
  expect(input).toHaveValue('Original message');
  fireEvent.keyDown(input, { key: 'Escape' }); expect(cancel).toHaveBeenCalledTimes(1);
});
