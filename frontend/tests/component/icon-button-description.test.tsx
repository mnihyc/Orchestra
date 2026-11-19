/** A value/state description stays exact when the icon's name tooltip opens and closes. */
import { cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import * as tooltips from '@/ui/Tooltip';
import { i18n, setLocale } from '@/i18n';
import { IconButton } from '@/ui/IconButton';
import { show } from './support';

afterEach(async () => { cleanup(); await setLocale('en'); });

it('keeps the twenty unchanged action controls when the thread supplies new click handlers', async () => {
  const invoke = vi.fn<(thread: string, index: number) => void>();
  function Controls({ thread }: { thread: string }) {
    return Array.from({ length: 20 }, (_, i) => <IconButton key={i} label={`Edit ${i}`} action="edit" icon={<span>edit</span>}
      onClick={() => invoke(thread, i)} />);
  }
  const tooltip = vi.spyOn(tooltips, 'Tooltip');
  const view = show(<Controls thread="first" />);
  try {
    const button = screen.getByRole('button', { name: 'Edit 7' });
    tooltip.mockClear();
    view.rerender(<Controls thread="second" />);
    expect(tooltip.mock.calls.length).toBe(0);
    expect(screen.getByRole('button', { name: 'Edit 7' })).toBe(button);
    await userEvent.click(button);
    expect(invoke).toHaveBeenCalledExactlyOnceWith('second', 7);
  } finally { view.unmount(); tooltip.mockRestore(); }
});

for (const locale of ['en', 'zh-CN'] as const) {
  it(`keeps a distinct refusal reason alongside the caller's value in ${locale}`, async () => {
    await setLocale(locale); const user = userEvent.setup();
    const label = i18n.t('actions.edit.label');
    const value = locale === 'en' ? 'safe: On' : '安全：开';
    const reason = locale === 'en' ? 'The setting is locked.' : '此设置已锁定。';
    show(<><span id="locked-value">{value}</span><IconButton label={label} aria-describedby="locked-value" reason={reason} blocked /></>);
    const control = screen.getByRole('button', { name: label });
    await user.tab();
    expect(await screen.findByRole('tooltip')).toHaveTextContent(reason);
    expect(control).toHaveAccessibleDescription(`${value} ${reason}`);
    expect(control).toHaveAttribute('aria-disabled', 'true');
  });

  it(`preserves the caller's complete accessible description through tooltip composition in ${locale}`, async () => {
    await setLocale(locale); const user = userEvent.setup();
    const label = i18n.t('actions.edit.label');
    const value = locale === 'en' ? 'safe: On' : '安全：开';
    show(<><span id="current-value">{value}</span><IconButton label={label} aria-describedby="current-value" /><input aria-label="Other field" /></>);
    const control = screen.getByRole('button', { name: label });
    expect(control).toHaveAccessibleDescription(value);
    await user.tab();
    expect(control).toHaveFocus();
    expect(await screen.findByRole('tooltip')).toHaveTextContent(label);
    expect(control).toHaveAttribute('aria-describedby', 'current-value');
    expect(control).toHaveAccessibleDescription(value);
    await user.tab();
    await waitFor(() => expect(screen.queryByRole('tooltip')).not.toBeInTheDocument());
    expect(control).toHaveAccessibleDescription(value);
    await user.tab({ shift: true });
    expect(control).toHaveFocus();
    expect(await screen.findByRole('tooltip')).toHaveTextContent(label);
    expect(control).toHaveAccessibleDescription(value);
  });
}
