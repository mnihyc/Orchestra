/** A button that may be cut by a narrow cell keeps the wrapper that carries its reason inside that cell. */
import { act, cleanup, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { Button } from '@/ui/Button';
import { show } from './support';

afterEach(cleanup);

it('bounds the wrapper of a reason only when the button asks for it', () => {
  show(<><Button reason="Locked for now">Plain</Button><Button reason="Locked for now" bounded>Cut</Button></>);
  const plain = screen.getByRole('button', { name: 'Plain' }).parentElement;
  const cut = screen.getByRole('button', { name: 'Cut' }).parentElement;
  expect(plain).not.toHaveClass('min-w-0');
  expect(cut).toHaveClass('min-w-0', 'max-w-full');
});

it('protects a pending act immediately and shows its spinner only after the reading delay', async () => {
  vi.useFakeTimers();
  try {
    const view = show(<Button pending>Try again</Button>);
    const button = screen.getByRole('button', { name: 'Try again' });
    expect(button).toHaveAttribute('aria-disabled', 'true');
    expect(button.querySelector('.animate-spin')).toBeNull();
    await act(async () => { await vi.advanceTimersByTimeAsync(199); });
    expect(button.querySelector('.animate-spin')).toBeNull();
    await act(async () => { await vi.advanceTimersByTimeAsync(1); });
    expect(button.querySelector('.animate-spin')).not.toBeNull();
    view.rerender(<Button>Try again</Button>);
    expect(button.querySelector('.animate-spin')).toBeNull();
  } finally { vi.useRealTimers(); }
});
