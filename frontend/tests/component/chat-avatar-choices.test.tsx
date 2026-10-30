/** The actual chat scope and avatar palette keep a visible choice without covering their content. */
import { cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { useRef, useState } from 'react';
import { afterEach, expect, it } from 'vitest';

import { ChatScope } from '@/features/chat/ChatPanel';
import type { Segment } from '@/features/chat/panel';
import { i18n, setLocale } from '@/i18n';
import { Avatar } from '@/ui/Avatar';
import { SwatchGroup } from '@/ui/SwatchGroup';
import { show } from './support';

const scopes: Segment[] = [
  { id: 'scope-topic', chatId: 'topic-chat', kind: 'topic', name: 'Fix login', seq: 0 },
  { id: 'scope-project', chatId: 'project-chat', kind: 'project', name: 'Web', seq: 0 },
];
const choiceStyles = readFileSync(resolve('src/styles/choices.css'), 'utf8');
const styles: HTMLStyleElement[] = [];

function stylesheet(dark: boolean) {
  const colours: Record<string, string> = {
    surface: dark ? '#111111' : '#ffffff',
    'control-border': dark ? '#bbbbbb' : '#444444',
    'control-selected': dark ? '#eeeeee' : '#111111',
    'control-on-selected': dark ? '#111111' : '#ffffff',
    'bg-muted': dark ? '#333333' : '#dddddd',
    'surface-raised': dark ? '#222222' : '#ffffff', 'segment-selected': dark ? '#444444' : '#ffffff', text: dark ? '#eeeeee' : '#111111',
    'text-muted': dark ? '#cccccc' : '#555555', 'shadow-sm': '0 1px 2px #0002',
    'text-on-accent': '#ffffff', focus: '#777777',
  };
  const style = document.createElement('style');
  style.textContent = choiceStyles.replace(/var\(--([\w-]+)\)/g, (original: string, token: string) => colours[token] ?? original);
  document.head.append(style); styles.push(style);
  expect(style.sheet?.cssRules.length).toBeGreaterThan(0);
}

function TopicScope() {
  const [value, setValue] = useState('topic-chat');
  const restore = useRef(false);
  // The live chat owns its header: changing chats remounts the scope, as it does in PanelChat.
  return <ChatScope key={value} segments={scopes} value={value} restoreFocusRef={restore}
    onValueChange={(next) => { restore.current = true; setValue(next); }} />;
}

function AvatarPalette() {
  const [value, setValue] = useState('0');
  return <SwatchGroup label={i18n.t('profile.avatar.label')} value={value} onValueChange={setValue}
    options={[0, 1, 2].map((colour) => ({ value: String(colour), name: i18n.t('profile.avatar.color', { number: colour + 1 }),
      sample: <Avatar text="A" color={colour} size={24} /> }))} />;
}

afterEach(async () => { cleanup(); for (const style of styles.splice(0)) style.remove(); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) for (const dark of [false, true]) {
  it(`shows the shared Topic/Project selection through tooltips and chat remounts in ${locale}/${dark ? 'dark' : 'light'}`, async () => {
    await setLocale(locale); stylesheet(dark);
    const user = userEvent.setup();
    show(<TopicScope />);
    const group = () => screen.getByRole('radiogroup', { name: i18n.t('chat.segments') });
    const topic = () => within(group()).getByRole('radio', { name: i18n.t('chat.segment.topic') });
    const project = () => within(group()).getByRole('radio', { name: i18n.t('chat.segment.project') });
    const selectedGround = dark ? 'rgb(68, 68, 68)' : 'rgb(255, 255, 255)';
    const selectedWords = dark ? 'rgb(238, 238, 238)' : 'rgb(17, 17, 17)';
    expect(group()).toHaveAttribute('data-segmented');
    expect(within(group()).getAllByRole('radio')).toHaveLength(2);
    expect(topic()).toBeChecked();
    expect(topic()).toHaveAttribute('data-state', 'checked');
    expect(getComputedStyle(topic()).backgroundColor).toBe(selectedGround);
    expect(getComputedStyle(project()).backgroundColor).toBe('rgba(0, 0, 0, 0)');
    expect(getComputedStyle(within(topic()).getByText(i18n.t('chat.segment.topic'))).color).toBe(selectedWords);
    await user.hover(topic());
    expect(await screen.findByRole('tooltip')).toHaveTextContent('Fix login');
    expect(topic()).toHaveAttribute('data-state', 'checked');
    expect(getComputedStyle(topic()).backgroundColor).toBe(selectedGround);
    await user.click(project());
    await waitFor(() => { expect(project()).toBeChecked(); expect(project()).toHaveFocus(); });
    expect(getComputedStyle(project()).backgroundColor).toBe(selectedGround);
    expect(topic()).not.toBeChecked();
    await user.keyboard('{ArrowLeft>}');
    try {
      await waitFor(() => { expect(topic()).toBeChecked(); expect(topic()).toHaveFocus(); });
    } finally { await user.keyboard('{/ArrowLeft}'); }
    expect(topic()).toHaveAttribute('data-state', 'checked');
    expect(getComputedStyle(topic()).backgroundColor).toBe(selectedGround);
    expect(project()).not.toBeChecked();
  });

  it(`rings the avatar colour outside its intact sample and keeps arrow selection in ${locale}/${dark ? 'dark' : 'light'}`, async () => {
    await setLocale(locale); stylesheet(dark);
    const user = userEvent.setup();
    show(<AvatarPalette />);
    const label = i18n.t('profile.avatar.label');
    const group = screen.getByRole('radiogroup', { name: label });
    expect(screen.getAllByText(label, { exact: true })).toHaveLength(1);
    const options = within(group).getAllByRole('radio');
    const first = within(group).getByRole('radio', { name: i18n.t('profile.avatar.color', { number: 1 }) });
    const next = within(group).getByRole('radio', { name: i18n.t('profile.avatar.color', { number: 2 }) });
    const ringColour = dark ? 'rgb(238, 238, 238)' : 'rgb(17, 17, 17)';
    expect(first).toBeChecked();
    expect(getComputedStyle(first).borderTopColor).toBe(ringColour);
    expect(getComputedStyle(first).borderTopWidth).toBe('2px');
    expect(getComputedStyle(first).backgroundColor).toBe('rgba(0, 0, 0, 0)');
    for (const option of options) {
      expect(option).toHaveClass('size-8', 'rounded-full', 'p-0.5');
      expect(option.querySelectorAll('.avatar')).toHaveLength(1);
      expect(option.querySelector('svg, .absolute')).toBeNull();
    }
    await user.click(first);
    await user.keyboard('{ArrowRight>}');
    try {
      await waitFor(() => { expect(next).toBeChecked(); expect(next).toHaveFocus(); });
    } finally { await user.keyboard('{/ArrowRight}'); }
    expect(getComputedStyle(next).borderTopColor).toBe(ringColour);
    expect(getComputedStyle(first).borderTopColor).toBe('rgba(0, 0, 0, 0)');
    expect(first).not.toBeChecked();
    expect(next.querySelector('svg, .absolute')).toBeNull();
  });
}
