/** Version arrows are keyboard-readable view controls, not implicit edit/activation actions. */
import { cleanup, fireEvent, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { show } from './support';
import { PromptVersions } from '@/features/timeline/PromptVersions';

afterEach(cleanup);
it('shows the requested ordinal, keeps endpoint arrows unavailable, and calls only the selected view direction', () => {
  const previous = vi.fn(), next = vi.fn();
  const view = show(<PromptVersions current={1} total={2} onPrevious={previous} onNext={next} />);
  expect(screen.getByText('1/2')).toBeVisible();
  expect(screen.getByRole('status')).toHaveAttribute('aria-live', 'polite');
  expect(screen.getByRole('button', { name: 'Previous version' })).toBeDisabled();
  fireEvent.click(screen.getByRole('button', { name: 'Next version' }));
  expect(next).toHaveBeenCalledTimes(1); expect(previous).not.toHaveBeenCalled();
  view.rerender(<PromptVersions current={2} total={2} onPrevious={previous} onNext={next} />);
  expect(screen.getByText('2/2')).toBeVisible(); expect(screen.getByRole('button', { name: 'Next version' })).toBeDisabled();
  const back = screen.getByRole('button', { name: 'Previous version' }); back.focus(); expect(back).toHaveFocus();
  fireEvent.click(back); expect(previous).toHaveBeenCalledTimes(1);
});
it('holds both directions during a read and omits controls on an unedited prompt', () => {
  const next = vi.fn(), previous = vi.fn();
  const view = show(<PromptVersions current={2} total={3} pending onPrevious={previous} onNext={next} />);
  expect(screen.getByRole('group', { name: 'Message versions' })).toHaveAttribute('aria-busy', 'true');
  for (const button of screen.getAllByRole('button')) { expect(button).toBeDisabled(); fireEvent.click(button); }
  expect(next).not.toHaveBeenCalled(); expect(previous).not.toHaveBeenCalled();
  view.rerender(<PromptVersions current={1} total={1} onPrevious={previous} onNext={next} />);
  expect(screen.queryByRole('group')).toBeNull();
});
