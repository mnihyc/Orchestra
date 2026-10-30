/** A place page's header icons keep a named counterpart in its menu, which opens the same contents in a dialog. */
import { screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';

import { type PlaceHeaderPanel, usePlaceHeaderPanels } from '@/features/navigation/PlaceHeaderPanels';
import type { Org, Topic } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { IconButton } from '@/ui/IconButton';
import { Menu } from '@/ui/Menu';
import { me, memberAccess, org, userRef } from '../doubles/fixtures';
import { show } from './support';

vi.mock('@/features/announcements/Composer', () => ({ PlaceAnnouncementContents: () => <div data-testid="announcement-composer" /> }));
vi.mock('@/features/thread/SubscriptionBell', () => ({ SubscriptionBell: () => <div data-testid="notification-options" /> }));
vi.mock('@/features/navigation/PlaceDetails', () => ({ PlaceDetailsContents: () => <div data-testid="place-details" /> }));

const acme = org('acme-id', 'acme', 'Acme', userRef(me(1, 'ada', 'Ada')), { access: { ...memberAccess, permissions: ['org.view'] } });
const login = { id: 'topic-id', kind: 'topic', name: 'Fix login', access: { permissions: ['topic.view'] } } as unknown as Topic;

function Header({ node, panels, bell }: { node: Org | Topic; panels: readonly PlaceHeaderPanel[]; bell: string }) {
  const made = usePlaceHeaderPanels(node, panels, bell);
  return <>
    <Menu label="Place menu" entries={made.entries} trigger={<IconButton ref={made.opener} label="More of this place" icon={<span aria-hidden>…</span>} />} />
    {made.dialogs}
  </>;
}

afterEach(async () => { await setLocale('en'); });

it.each(['en', 'zh-CN'] as const)('names each header icon in the menu and opens its contents from there (%s)', async (locale) => {
  await setLocale(locale);
  const user = userEvent.setup();
  show(<Header node={acme} panels={['details', 'announcements', 'notifications']} bell={i18n.t('notificationBell.place')} />);
  await user.click(screen.getByRole('button', { name: 'More of this place' }));
  const menu = screen.getByRole('menu');
  for (const name of [i18n.t('places.details.title'), i18n.t('placeAnnouncements.title'), i18n.t('notificationBell.place')]) expect(within(menu).getByRole('menuitem', { name })).toBeVisible();
  await user.click(within(menu).getByRole('menuitem', { name: i18n.t('placeAnnouncements.title') }));
  const dialog = await screen.findByRole('dialog', { name: i18n.t('placeAnnouncements.at', { name: 'Acme' }) });
  expect(within(dialog).getByTestId('announcement-composer')).toBeInTheDocument();
});

it('offers a topic only what its header shows: no description panel of its own', async () => {
  const user = userEvent.setup();
  show(<Header node={login} panels={['details', 'announcements', 'notifications']} bell={i18n.t('places.topic.bell')} />);
  await user.click(screen.getByRole('button', { name: 'More of this place' }));
  const menu = screen.getByRole('menu');
  expect(within(menu).queryByRole('menuitem', { name: i18n.t('places.details.title') })).toBeNull();
  await user.click(within(menu).getByRole('menuitem', { name: i18n.t('places.topic.bell') }));
  const dialog = await screen.findByRole('dialog', { name: i18n.t('places.topic.bell') });
  expect(within(dialog).getByTestId('notification-options')).toBeInTheDocument();
});
