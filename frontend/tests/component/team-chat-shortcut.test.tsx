/** A team-chat shortcut leaves the page in place and focuses the live composer once it is ready. */
import { act, fireEvent, screen, waitFor } from '@testing-library/react';
import { useEffect, useRef } from 'react';
import { afterEach, expect, it, vi } from 'vitest';
import { i18n, setLocale } from '@/i18n';
import { useComposerFocus, requestChatFocus } from '@/features/chat/focus';
import { TeamChatShortcut } from '@/features/navigation/TeamChat';
import { LeaveDrawer } from '@/features/navigation/OrgSwitcher';
import { ViewRouteContext } from '@/features/workbench/viewOpening';
import { show } from './support';

const world = vi.hoisted(() => ({ reveal: vi.fn(), takeQuotes: vi.fn() }));
vi.mock('@/features/workbench/openView', () => ({ revealView: world.reveal }));
vi.mock('@/realtime/stores/chat', () => ({ useChatUnread: () => ({ unread: false, mentions: 0 }) }));

function Composer({ ready }: { ready: boolean }) {
  const input = useRef<HTMLInputElement>(null);
  const onReady = useComposerFocus('team-chat', world.takeQuotes);
  useEffect(() => {
    if (ready) onReady({ quote: () => undefined, focus: () => input.current?.focus() });
  }, [onReady, ready]);
  return ready ? <input ref={input} aria-label="Chat draft" /> : <p>Loading chat</p>;
}
afterEach(async () => { vi.clearAllMocks(); requestChatFocus(''); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) it(`reveals the single team chat without a link or page navigation in ${locale}`, async () => {
  await setLocale(locale);
  const leaveDrawer = vi.fn();
  const page = window.location.href;
  const view = show(<LeaveDrawer.Provider value={leaveDrawer}><TeamChatShortcut nodeId="team" chatId="team-chat" /><Composer ready={false} /><input aria-label="Page draft" defaultValue="Keep this" /></LeaveDrawer.Provider>);
  fireEvent.click(screen.getByRole('button', { name: i18n.t('navigation.teamChat') }));
  expect(world.reveal).toHaveBeenCalledWith('chat', { node: 'team' }, { focus: false });
  expect(leaveDrawer).toHaveBeenCalledTimes(1);
  expect(window.location.href).toBe(page);
  expect(screen.queryByRole('link', { name: i18n.t('navigation.teamChat') })).not.toBeInTheDocument();
  view.rerender(<LeaveDrawer.Provider value={leaveDrawer}><TeamChatShortcut nodeId="team" chatId="team-chat" /><Composer ready /><input aria-label="Page draft" defaultValue="Keep this" /></LeaveDrawer.Provider>);
  await waitFor(() => expect(screen.getByRole('textbox', { name: 'Chat draft' })).toHaveFocus());
  expect(screen.getByRole('textbox', { name: 'Page draft' })).toHaveValue('Keep this');
  screen.getByRole('button', { name: i18n.t('navigation.teamChat') }).focus();
  act(() => requestChatFocus('team-chat'));
  expect(screen.getByRole('textbox', { name: 'Chat draft' })).toHaveFocus();
});

it('does not steal focus on another screen when an old chat becomes ready later', () => {
  requestChatFocus('team-chat', 'old-opening');
  const view = show(<ViewRouteContext.Provider value="new-opening"><Composer ready={false} /><input aria-label="Current page" /></ViewRouteContext.Provider>);
  screen.getByRole('textbox', { name: 'Current page' }).focus();
  view.rerender(<ViewRouteContext.Provider value="new-opening"><Composer ready /><input aria-label="Current page" /></ViewRouteContext.Provider>);
  expect(screen.getByRole('textbox', { name: 'Current page' })).toHaveFocus();
  expect(screen.getByRole('textbox', { name: 'Chat draft' })).not.toHaveFocus();
});
