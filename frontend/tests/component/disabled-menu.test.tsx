/** A disabled native trigger never opens its menu, and regains the same action after becoming available. */
import { fireEvent, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MoreHorizontal } from 'lucide-react';
import { describe, expect, it, vi } from 'vitest';
import { Button } from '@/ui/Button';
import { IconButton } from '@/ui/IconButton';
import { Menu } from '@/ui/Menu';
import { show } from './support';

describe.each(['text', 'icon'] as const)('a %s menu trigger', (kind) => {
  it('blocks pointer and keyboard opening while disabled, and preserves selection when enabled', async () => {
    const selected = vi.fn();
    const render = (disabled: boolean) => <Menu label="Delivery" entries={[{ id: 'queue', label: 'Queue', onSelect: selected }]}
      trigger={kind === 'text' ? <Button disabled={disabled}>Delivery</Button> : <IconButton disabled={disabled} label="Delivery" icon={<MoreHorizontal aria-hidden />} />} />;
    const view = show(render(true));
    const trigger = screen.getByRole('button', { name: 'Delivery' });
    expect(trigger).toBeDisabled();
    await userEvent.click(trigger);
    fireEvent.keyDown(trigger, { key: 'ArrowDown' });
    expect(screen.queryByRole('menu')).not.toBeInTheDocument();
    expect(selected).not.toHaveBeenCalled();
    view.rerender(render(false));
    expect(screen.getByRole('button', { name: 'Delivery' })).toBeEnabled();
    await userEvent.click(screen.getByRole('button', { name: 'Delivery' }));
    await userEvent.click(screen.getByRole('menuitem', { name: 'Queue' }));
    expect(selected).toHaveBeenCalledExactlyOnceWith();
  });
});
