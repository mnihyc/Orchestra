/** Independently enabled split menus keep a distinct divider and their button variant's outline. */
import { screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';

import { SplitButton } from '@/ui/SplitButton';
import { show } from './support';

it.each([['primary', 'Send'], ['secondary', 'Stop']] as const)('keeps the enabled %s menu legible beside a disabled main action', async (variant, label) => {
  const schedule = vi.fn();
  show(<SplitButton variant={variant} disabled menuDisabled={false} menuLabel="More ways to send"
    onClick={() => undefined} entries={[{ id: 'schedule', label: 'Send later', onSelect: schedule }]}>{label}</SplitButton>);
  expect(screen.getByRole('button', { name: label })).toBeDisabled();
  const menu = screen.getByRole('button', { name: 'More ways to send' });
  expect(menu).toBeEnabled();
  expect(menu).toHaveClass('border-l-current');
  if (variant === 'secondary') expect(menu).toHaveClass('border-control-border');
  await userEvent.click(menu);
  await userEvent.click(screen.getByRole('menuitem', { name: 'Send later' }));
  expect(schedule).toHaveBeenCalledExactlyOnceWith();
});
