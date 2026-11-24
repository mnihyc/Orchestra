/** A value/state description stays exact when the icon's name tooltip opens and closes. */
import { cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import type * as Radix from 'radix-ui';
import * as tooltips from '@/ui/Tooltip';
import { i18n, setLocale } from '@/i18n';
import { IconButton } from '@/ui/IconButton';
import { show } from './support';

const primitives = vi.hoisted(() => ({ tooltip: vi.fn() }));
vi.mock('radix-ui', async (load) => {
  const lib = await load<typeof Radix>();
  return { ...lib, Tooltip: { ...lib.Tooltip, Root: (props: React.ComponentProps<typeof lib.Tooltip.Root>) => {
    primitives.tooltip();
    return <lib.Tooltip.Root {...props} />;
  } } };
});

afterEach(async () => { cleanup(); await setLocale('en'); });

it('opens one tooltip over the actual focused control without mounting a tooltip tree for every button', async () => {
  primitives.tooltip.mockClear();
  show(<>{Array.from({ length: 40 }, (_, i) => <IconButton key={i} label={`Thread action ${i}`} shortcut="Escape" />)}</>);
  expect(primitives.tooltip.mock.calls.length).toBeLessThanOrEqual(1);
  const user = userEvent.setup();
  const first = screen.getByRole('button', { name: 'Thread action 0' });
  await user.tab();
  const tip = await screen.findByRole('tooltip');
  expect(first).toHaveFocus();
  expect(first.getAttribute('aria-describedby')?.split(' ')).toContain(tip.id);
  expect(tip).toHaveTextContent('Thread action 0');
  expect(tip.querySelector('kbd')).not.toBeNull();
  await user.tab();
  const second = screen.getByRole('button', { name: 'Thread action 1' });
  expect(await screen.findByRole('tooltip')).toHaveTextContent('Thread action 1');
  expect(second).toHaveFocus();
  expect(first).not.toHaveAttribute('aria-describedby');
  await user.keyboard('{Escape}');
  await waitFor(() => expect(screen.queryByRole('tooltip')).not.toBeInTheDocument());
  expect(second).toHaveFocus();
});

it('keeps a focused button’s label current across a ref handoff and dismisses it when the button leaves', async () => {
  const receive = vi.fn<(node: HTMLButtonElement | null) => void>();
  const button = (label: string) => <IconButton label={label} ref={(node) => { receive(node); }} />;
  const view = show(button('Old thread name'));
  const source = screen.getByRole('button', { name: 'Old thread name' });
  await userEvent.tab();
  expect(await screen.findByRole('tooltip')).toHaveTextContent('Old thread name');
  view.rerender(button('New thread name'));
  expect(screen.getByRole('button', { name: 'New thread name' })).toBe(source);
  await waitFor(() => expect(screen.getByRole('tooltip')).toHaveTextContent('New thread name'));
  expect(source).toHaveFocus();
  view.rerender(<span>Another tab</span>);
  await waitFor(() => expect(screen.queryByRole('tooltip')).not.toBeInTheDocument());
});

it('keeps the twenty unchanged action controls when the thread supplies new click handlers', async () => {
  const invoke = vi.fn<(thread: string, index: number) => void>();
  function Controls({ thread }: { thread: string }) {
    return Array.from({ length: 20 }, (_, i) => <IconButton key={i} label={`Edit ${i}`} action="edit" icon={<span>edit</span>}
      onClick={() => invoke(thread, i)} />);
  }
  const tooltip = vi.spyOn(tooltips, 'ControlLabel');
  const view = show(<Controls thread="first" />);
  try {
    const button = screen.getByRole('button', { name: 'Edit 7' });
    expect(tooltip).toHaveBeenCalled();
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
