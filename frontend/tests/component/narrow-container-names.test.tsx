/** Medium names and the phone's single row of icons keep every choice directly reachable. */
import { DndContext } from '@dnd-kit/core';
import { act, cleanup, fireEvent, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { useState } from 'react';
import { MessagesSquare } from 'lucide-react';
import { NarrowChoices } from '@/features/workbench/NarrowChoices';
import { ContainerSwitcher } from '@/features/workbench/ContainerSwitcher';
import { containerChoice } from '@/features/workbench/choiceWords';
import { makeContainer } from '@/features/workbench/layout';
import { i18n, setLocale } from '@/i18n';
import { OVERFLOW_REST_MS } from '@/ui/overflowTooltips';
import { show } from './support';

vi.mock('@/features/navigation/area', () => ({ useAreaNavigation: () => null }));
const names = ['conversation', 'team', 'code', 'environment', 'agent'] as const;
function Controls() {
  const [current, choose] = useState<string>('conversation');
  return <NarrowChoices choices={names.map((name) => ({ id: name, name: i18n.t(`workbench.containers.${name}.title`),
    description: i18n.t(`workbench.containers.${name}.about`), icon: <MessagesSquare aria-hidden />, active: current === name,
    onSelect: () => choose(current === name ? '' : name) }))} />;
}
const phoneNames = ['navigation', 'resource-navigation', ...names, 'terminal'] as const;
function phoneWords(name: typeof phoneNames[number]) {
  return name === 'terminal' ? { name: i18n.t('views.terminal.title'), description: i18n.t('views.terminal.about') } : containerChoice(name, i18n.t);
}
function PhoneControls({ selected = () => undefined }: { selected?: (name: string) => void }) {
  const [current, choose] = useState<string>('conversation');
  return <NarrowChoices presentation="icons" choices={phoneNames.map((name) => ({ id: name, ...phoneWords(name),
    icon: <MessagesSquare aria-hidden />, active: current === name,
    onSelect: () => { choose(current === name ? '' : name); selected(name); } }))} />;
}
afterEach(async () => { cleanup(); vi.useRealTimers(); document.documentElement.removeAttribute('data-theme'); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  it(`shows every medium-width choice and activates it with arrows and Enter at ${locale}/${theme}`, async () => {
    await setLocale(locale); document.documentElement.setAttribute('data-theme', theme);
    const user = userEvent.setup(); show(<Controls />);
    const toolbar = screen.getByRole('toolbar', { name: i18n.t('workbench.containers.label') });
    for (const name of names) expect(within(toolbar).getByText(i18n.t(`workbench.containers.${name}.title`), { exact: true })).toBeVisible();
    expect(within(toolbar).queryByRole('button', { name: /^(More|Details|更多|详情)$/ })).not.toBeInTheDocument();
    const first = within(toolbar).getByRole('button', { name: i18n.t('workbench.containers.conversation.title') });
    first.focus(); await user.keyboard('{ArrowRight}{Enter}');
    const team = within(toolbar).getByRole('button', { name: i18n.t('workbench.containers.team.title') });
    expect(team).toHaveFocus(); expect(team).toHaveAttribute('aria-pressed', 'true');
    await user.keyboard('{End}{Enter}');
    expect(within(toolbar).getByRole('button', { name: i18n.t('workbench.containers.agent.title') })).toHaveAttribute('aria-pressed', 'true');
  });

  it(`names every phone icon on focus and keeps one shown view at ${locale}/${theme}`, async () => {
    await setLocale(locale); document.documentElement.dataset.theme = theme;
    const user = userEvent.setup(), selected = vi.fn(); show(<PhoneControls selected={selected} />);
    const toolbar = screen.getByRole('toolbar', { name: i18n.t('workbench.containers.label') });
    expect(within(toolbar).getAllByRole('button')).toHaveLength(8);
    expect(within(toolbar).queryByRole('button', { name: /^(More|更多)$/ })).not.toBeInTheDocument();
    for (const name of phoneNames) {
      const words = phoneWords(name);
      const button = within(toolbar).getByRole('button', { name: words.name });
      expect(button).toHaveAccessibleDescription(words.description);
      act(() => { button.focus(); });
      expect(await screen.findByRole('tooltip')).toHaveTextContent(words.name);
    }
    await user.keyboard('{Home}{ArrowRight}{Enter}');
    const resources = within(toolbar).getByRole('button', { name: phoneWords('resource-navigation').name });
    expect(resources).toHaveFocus(); expect(resources).toHaveAttribute('aria-pressed', 'true');
    expect(toolbar.querySelectorAll('[aria-pressed="true"]')).toHaveLength(1);
    expect(selected).toHaveBeenCalledExactlyOnceWith('resource-navigation');
    await user.keyboard('{Enter}');
    expect(toolbar.querySelectorAll('[aria-pressed="true"]')).toHaveLength(0);
  });

  it(`reveals a phone name on hold without opening it, and cancels a scrolling gesture at ${locale}/${theme}`, async () => {
    await setLocale(locale); document.documentElement.dataset.theme = theme;
    vi.useFakeTimers();
    const selected = vi.fn(); show(<PhoneControls selected={selected} />);
    const button = screen.getByRole('button', { name: phoneWords('team').name });
    const touch = { pointerType: 'touch', pointerId: 1, clientX: 20, clientY: 20 };
    fireEvent.pointerDown(button, touch);
    expect(screen.queryByRole('tooltip')).not.toBeInTheDocument();
    act(() => { vi.advanceTimersByTime(OVERFLOW_REST_MS); });
    expect(screen.getByRole('tooltip')).toHaveTextContent(phoneWords('team').name);
    expect(fireEvent.contextMenu(button)).toBe(false);
    fireEvent.pointerUp(button, touch); fireEvent.click(button, { detail: 1 });
    expect(selected).not.toHaveBeenCalled();
    expect(button).toHaveAttribute('aria-pressed', 'false');
    fireEvent.pointerDown(button, touch);
    fireEvent.pointerMove(button, { ...touch, clientY: 50 });
    act(() => { vi.advanceTimersByTime(OVERFLOW_REST_MS); });
    expect(screen.queryByRole('tooltip')).not.toBeInTheDocument();
    fireEvent.pointerCancel(button, touch);
    fireEvent.pointerDown(button, touch); fireEvent.pointerUp(button, touch); fireEvent.click(button, { detail: 1 });
    expect(selected).toHaveBeenCalledExactlyOnceWith('team');
    expect(button).toHaveAttribute('aria-pressed', 'true');
    fireEvent.pointerDown(button, touch); act(() => { vi.advanceTimersByTime(OVERFLOW_REST_MS); });
    fireEvent.pointerUp(button, touch);
    // Keyboard activation has no pointer-click count and must still work after reading a name.
    fireEvent.click(button, { detail: 0 });
    expect(selected).toHaveBeenCalledTimes(2); expect(selected).toHaveBeenLastCalledWith('team');
    expect(button).toHaveAttribute('aria-pressed', 'false');
    fireEvent.pointerDown(button, touch);
    cleanup(); act(() => { vi.advanceTimersByTime(OVERFLOW_REST_MS); });
    expect(screen.queryByRole('tooltip')).not.toBeInTheDocument();
  });

  it(`keeps every right-container label in the constrained switcher at ${locale}/${theme}`, async () => {
    await setLocale(locale); document.documentElement.setAttribute('data-theme', theme);
    const choose = vi.fn(); const user = userEvent.setup();
    show(<DndContext><div style={{ width: 180 }}><ContainerSwitcher containers={names.map((name) => makeContainer(name))} active="conversation" onSelect={choose} /></div></DndContext>);
    const strip = screen.getByRole('tablist', { name: i18n.t('workbench.containers.label') });
    for (const name of names) {
      const title = i18n.t(`workbench.containers.${name}.title`);
      expect(within(strip).getByRole('tab', { name: title })).toHaveTextContent(title);
      expect(within(strip).getByText(title, { exact: true })).toBeVisible();
    }
    within(strip).getByRole('tab', { name: i18n.t('workbench.containers.conversation.title') }).focus();
    await user.keyboard('{End}'); expect(choose).toHaveBeenCalledWith('agent');
    expect(screen.queryByRole('button', { name: /^(More|更多)$/ })).not.toBeInTheDocument();
  });
}
