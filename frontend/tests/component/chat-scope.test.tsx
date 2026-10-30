/** The scope row shows names only; place tooltips follow pointer and keyboard selection. */
import { cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { afterEach, describe, expect, it } from 'vitest';

import { ChatScope } from '@/features/chat/ChatPanel';
import type { Segment } from '@/features/chat/panel';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

const SCOPES: Segment[] = [
  { id: 'scope-thread', chatId: 'thread-chat', kind: 'thread', name: 'Main conversation', seq: 0 },
  { id: 'scope-topic', chatId: 'topic-chat', kind: 'topic', name: 'Fix login', seq: 0 },
  { id: 'scope-project', chatId: 'project-chat', kind: 'project', name: 'Web', seq: 0 },
];

function Scope() {
  const [value, setValue] = useState('thread-chat');
  return <ChatScope segments={SCOPES} value={value} onValueChange={setValue} />;
}

afterEach(async () => { cleanup(); await setLocale('en'); });

describe('compact chat scope labels', () => {
  it.each(['en', 'zh-CN'] as const)('names the place as the selection changes in %s and keeps keyboard access', async (locale) => {
    await setLocale(locale);
    const t = i18n.getFixedT(locale), user = userEvent.setup();
    show(<Scope />);
    const group = screen.getByRole('radiogroup', { name: t('chat.segments') });
    const current = within(group).getByRole('radio', { name: t('chat.segment.thread') });
    expect(current).toBeChecked();
    for (const option of within(group).getAllByRole('radio')) expect(option).not.toHaveAttribute('aria-describedby');
    expect(within(group).queryByText('Main conversation', { exact: true })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /Guide|指南/ })).not.toBeInTheDocument();
    await user.click(within(group).getByRole('radio', { name: t('chat.segment.topic') }));
    const topic = within(group).getByRole('radio', { name: t('chat.segment.topic') });
    expect(topic).toBeChecked();
    expect(within(group).getByRole('radio', { name: t('chat.segment.thread') })).not.toBeChecked();
    expect(within(group).queryByText('Fix login', { exact: true })).not.toBeInTheDocument();
    // Clicking dismisses a tooltip until the pointer leaves; re-enter to rest on this name.
    await user.unhover(topic);
    await user.hover(topic);
    expect(await screen.findByRole('tooltip')).toHaveTextContent('Fix login');
    expect(topic).toHaveAccessibleDescription('Fix login');
    await user.unhover(topic);
    // Radix moves focus in a later task, then selects while the arrow is still held.
    await user.keyboard('{ArrowRight>}');
    const project = within(group).getByRole('radio', { name: t('chat.segment.project') });
    try {
      await waitFor(() => {
        expect(project).toHaveFocus();
        expect(project).toBeChecked();
        expect(topic).not.toBeChecked();
      });
    } finally { await user.keyboard('{/ArrowRight}'); }
    // Selection clicks dismiss tooltips. Returning by Tab reads the selected scope's full name.
    await user.tab(); await user.tab({ shift: true });
    expect(project).toHaveFocus();
    expect(await screen.findByRole('tooltip')).toHaveTextContent('Web');
    expect(project).toHaveAccessibleDescription('Web');
  });
});
