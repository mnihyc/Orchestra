/** Timed group membership names its calendar deadline even when it ends within the next day. */
import { cleanup, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';

import { GroupMenu } from '@/features/chat/GroupMenu';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { chat } from '../doubles/chat-rows';
import { me, userRef } from '../doubles/fixtures';
import { show } from './support';

vi.mock('@/features/export/ChatMenu', () => ({ useChatExportEntries: () => [] }));
const appearance = useAppearance.getState();
const HOUR_MS = 60 * 60 * 1000;

afterEach(async () => { cleanup(); useAppearance.setState(appearance); await setLocale('en'); });

it.each(['en', 'zh-CN'] as const)('shows the membership date for a near expiry and no deadline for an ordinary member (%s)', async (locale) => {
  await setLocale(locale); useAppearance.setState({ locale, timeZone: 'UTC' });
  const ada = userRef(me(1, 'ada', 'Ada Lovelace')), vera = userRef(me(2, 'vera', 'Vera Rubin'));
  const end = new Date(Date.now() + HOUR_MS).toISOString();
  const row = chat({ kind: 'group', node: null, members: [ada, vera], created_by: ada, membership_expiries: { [vera.id]: end } });
  show(<GroupMenu chat={row} me={ada.id} onLeft={() => undefined} />);
  await userEvent.click(screen.getByRole('button', { name: i18n.t('chat.group.menu') }));
  await userEvent.click(screen.getByRole('menuitem', { name: i18n.t('chat.group.people') }));
  const member = screen.getByRole('button', { name: 'Vera Rubin' }).closest('li');
  if (member === null) throw new Error('The timed member must have a roster row.');
  const time = member.querySelector('time');
  expect(time).toHaveAttribute('datetime', end);
  expect(time).toHaveTextContent(new Intl.DateTimeFormat(locale, { month: 'short', timeZone: 'UTC' }).format(new Date(end)));
  expect(within(member).getByText(i18n.t('chat.people.until'))).toBeVisible();
  expect(screen.getByRole('button', { name: 'Ada Lovelace' }).closest('li')?.querySelector('time')).toBeNull();
});
