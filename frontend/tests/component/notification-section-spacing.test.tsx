/** The actual Notifications body inherits one section spacing scale from the personal settings frame. */
import { cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type * as Router from '@tanstack/react-router';
import type { AnchorHTMLAttributes, ReactNode } from 'react';
import { afterEach, expect, it, vi } from 'vitest';
import { Notifications } from '@/features/settings/personal/Notifications';
import type { NotificationPrefs } from '@/generated/api-types';
import type * as Screen from '@/shell/screen';
import { i18n, setLocale } from '@/i18n';
import { Section } from '@/shell/Page';
import { show } from './support';

const openHelp = vi.hoisted(() => vi.fn());
vi.mock('@/features/help/open', () => ({ useHelpOpener: () => openHelp }));
const prefs: NotificationPrefs = { approvals_always: false, chat_direct_level: 'mentions', chat_node_level: 'mentions', quiet_hours: null, pause_until: null, revision: 1 };
vi.mock('@/api/notifications', () => ({ notificationPrefsKey: ['spacing-notification-prefs'], setNotificationPrefs: vi.fn(),
  notificationPrefsQuery: { queryKey: ['spacing-notification-prefs'], queryFn: () => Promise.resolve(prefs) },
}));
vi.mock('@/shell/screen', async (load) => ({ ...await load<typeof Screen>(), useScreenHeading: () => undefined }));
vi.mock('@/features/settings/personal/NotificationExceptions', () => ({ NotificationExceptions: () => <Section id="spacing-exceptions" title={i18n.t('notificationExceptions.title')}><p>No exceptions</p></Section> }));
vi.mock('@/features/settings/personal/Routes', () => ({ OutsideOrch: () => <p>Outward routes</p> }));
vi.mock('@/features/settings/personal/away/Pause', () => ({ PauseControls: () => <p>Pause choices</p> }));
vi.mock('@tanstack/react-router', async (load) => ({ ...await load<typeof Router>(), Link: ({ to, children, activeOptions: _activeOptions, ...props }: AnchorHTMLAttributes<HTMLAnchorElement> & { to: string; activeOptions?: unknown; children?: ReactNode }) => <a {...props} href={to}>{children}</a> }));
afterEach(async () => { cleanup(); openHelp.mockClear(); document.documentElement.removeAttribute('data-theme'); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark']) it(`separates all six sections without wrappers or repeated headings in ${locale}/${theme}`, async () => {
  await setLocale(locale); document.documentElement.dataset.theme = theme;
  const view = show(<Notifications />);
  await screen.findByRole('region', { name: i18n.t('notifications.approvals.title') });
  const content = view.container.querySelector('[data-settings-content]');
  expect(content).toHaveClass('flex', 'flex-col', 'gap-6');
  const titles = ['notificationExceptions.title', 'notifications.kinds', 'notifications.approvals.title', 'notifications.chats.title', 'notifications.quiet.title', 'notifications.outside.title'] as const;
  const regions = titles.map((title) => screen.getByRole('region', { name: i18n.t(title) }));
  expect(regions.every((region) => region.parentElement === content)).toBe(true);
  expect(content?.querySelectorAll(':scope > section')).toHaveLength(6);
  for (const title of titles) expect(screen.getAllByRole('heading', { name: i18n.t(title) })).toHaveLength(1);
  expect(screen.getAllByRole('heading', { level: 1 })).toHaveLength(1);
  const question = screen.getByRole('button', { name: i18n.t('help.hint.label', { term: i18n.t('help.hints.loss-notify.title') }) });
  const heading = 'tell-the-affected-people';
  await waitFor(() => expect(question).toHaveAttribute('data-help-heading', heading));
  await userEvent.click(question);
  await userEvent.click(await screen.findByRole('button', { name: i18n.t('help.hint.more') }));
  expect(openHelp).toHaveBeenCalledOnce();
  expect(openHelp).toHaveBeenCalledWith('recover-lost-work', heading);
});
