/** Mark all read explains an absent or not-yet-read counter. */
import type * as Live from '@/realtime/live';
import type * as AwayLive from '@/features/settings/personal/away/live';
import { act, cleanup, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { Inbox } from '@/features/inbox/Inbox';
import { i18n, setLocale } from '@/i18n';
import { useUserStore } from '@/realtime/stores/user';
import { show } from './support';

const mark = vi.hoisted(() => vi.fn());
vi.mock('@/features/inbox/read', () => ({ markNotificationsRead: mark }));
vi.mock('@/realtime/live', async (load) => ({ ...await load<typeof Live>(), useUserFeed: () => undefined }));
vi.mock('@/features/settings/personal/away/live', async (load) => ({ ...await load<typeof AwayLive>(), useAwayLive: () => undefined }));
vi.mock('@/features/settings/personal/away/Pause', () => ({ PauseControls: () => null }));
vi.mock('@/features/inbox/away/Grouping', () => ({ useInboxGrouping: () => ({ value: 'day', loading: false, saving: false, choose: () => undefined, error: null }), GroupingControl: () => null }));
vi.mock('@/shell/screen', async (loadOriginal) => ({ ...await loadOriginal<object>(), useScreenHeading: () => undefined }));
vi.mock('@/features/help/Hint', () => ({ Hint: () => null }));
const previous = useUserStore.getState();
afterEach(async () => { cleanup(); useUserStore.setState(previous); mark.mockClear(); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) it.each([true, false])('explains the unavailable read action in '+locale+' (loaded: %s)', async (loaded) => {
  await setLocale(locale);
  useUserStore.setState({ loaded, rows: new Map(), reachedEnd: true, nextCursor: null, counters: { ...previous.counters, unread: 0 } });
  show(<Inbox filter="all" onFilter={() => undefined} />);
  const control = screen.getByRole('button', { name: i18n.t('inbox.markAllRead') }), target = control.parentElement;
  if (target === null) throw new Error('The disabled read action must have a keyboard target');
  expect(control).toBeDisabled(); act(() => target.focus());
  expect(await screen.findByRole('tooltip')).toHaveTextContent(i18n.t(loaded ? 'ui.unavailable.noUnread' : 'common.loading'));
  expect(mark).not.toHaveBeenCalled();
});
