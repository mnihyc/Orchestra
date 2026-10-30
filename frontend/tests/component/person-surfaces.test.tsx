/** Inbox actors and missing-access approvers use the shared quick and kept person cards. */
import { screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { Activity } from 'react';
import type * as Router from '@tanstack/react-router';
import { expect, it, vi } from 'vitest';
import { MissingAccessList } from '@/ui/ErrorState';
import { NotificationRow } from '@/features/inbox/NotificationRow';
import type { MissingAccess, Notification } from '@/generated/api-types';
import { DORA, GRACE } from '../doubles/chat-rows';
import { show } from './support';
import { UserChip } from '@/ui/UserChip';
import { AvatarStack } from '@/ui/AvatarStack';

vi.mock('@tanstack/react-router', async (original) => ({ ...await original<typeof Router>(), useNavigate: () => vi.fn() }));
vi.mock('@/api/profile', () => ({ profileQuery: (username: string) => ({ queryKey: ['profile-surface', username], queryFn: () => Promise.resolve({ is_me: false, can_message: false }) }) }));
vi.mock('@/features/profile/CardDetails', () => ({ CardDetails: () => <div>Full profile</div> }));

const missing: MissingAccess = { object: { id: 'doc', kind: 'document', name: 'Guide', owner: null }, permissions: ['doc.view'], ask: [DORA], ask_more: 0, override: false, closed: null, options: null };

it('opens a single missing-access approver by keyboard without changing the refusal', async () => {
  const input = userEvent.setup(); show(<MissingAccessList missing={[missing]} />);
  expect(screen.getByText('Ask Dora Developer.')).toHaveClass('sr-only');
  await input.tab();
  expect(screen.getByRole('button', { name: DORA.display_name })).toHaveFocus();
  expect(await screen.findByText(`@${DORA.username}`)).toBeVisible();
  await input.keyboard('{Enter}');
  expect(within(await screen.findByRole('dialog', { name: DORA.display_name })).getByText('Full profile')).toBeVisible();
});

it('opens the common people list for several approvers and keeps the hidden remainder unnamed', async () => {
  const input = userEvent.setup(); show(<MissingAccessList missing={[{ ...missing, ask: [DORA, GRACE], ask_more: 3 }]} />);
  await input.click(screen.getByRole('button', { name: /People: Dora Developer, Grace Hopper/ }));
  const people = await screen.findByRole('dialog', { name: 'People' });
  const person = within(people).getByRole('button', { name: DORA.display_name });
  await input.click(person);
  expect(within(await screen.findByRole('dialog', { name: DORA.display_name })).getByText('Full profile')).toBeVisible();
});

it('keeps an older Inbox actor independently inspectable outside the notice link', async () => {
  const row: Notification = { id: 'notice', kind: 'doc.comment', actor: DORA, title: 'Stored notice', body: 'Please review', thread_id: null, thread: null, node: null, path: [], approval_id: null, chat_id: null,
    url: '/doc/doc', data: {}, details: { v: 1, action: 'doc.comment', parameters: { subject: 'Guide' } }, created_at: '2026-10-15T16:00:00Z', read_at: null, inbox_seq: 1, group: null, waiting_for_me: false };
  const input = userEvent.setup(); show(<ul><NotificationRow row={row} /></ul>);
  const person = screen.getByRole('button', { name: DORA.display_name });
  expect(screen.getByRole('link')).not.toContainElement(person);
  await input.hover(person);
  expect(await screen.findByText(`@${DORA.username}`)).toBeVisible();
  await input.click(person);
  expect(within(await screen.findByRole('dialog', { name: DORA.display_name })).getByText('Full profile')).toBeVisible();
});

it.each([DORA, { ...DORA, kind: 'service' as const, display_name: 'Build service' }])('keeps $display_name interactive when its tab returns', async (person) => {
  const input = userEvent.setup();
  const view = show(<Activity mode="visible"><UserChip user={person} /></Activity>);
  expect(screen.getByRole('button', { name: person.display_name })).toBeVisible();
  for (let count = 0; count < 2; count += 1) {
    view.rerender(<Activity mode="hidden"><UserChip user={person} /></Activity>);
    view.rerender(<Activity mode="visible"><UserChip user={person} /></Activity>);
    expect(screen.getByRole('button', { name: person.display_name })).toBeVisible();
  }
  await input.hover(screen.getByRole('button', { name: person.display_name }));
  expect(await screen.findByText(`@${person.username}`)).toBeVisible();
  await input.click(screen.getByRole('button', { name: person.display_name }));
  expect(await screen.findByRole('dialog', { name: person.display_name })).toBeVisible();
});

it('keeps a chip inside a choice noninteractive when its tab returns', () => {
  const content = <button type="button">Choose <UserChip user={DORA} /><AvatarStack users={[DORA, GRACE]} /></button>;
  const view = show(<Activity mode="visible">{content}</Activity>);
  view.rerender(<Activity mode="hidden">{content}</Activity>);
  view.rerender(<Activity mode="visible">{content}</Activity>);
  expect(screen.getAllByRole('button')).toHaveLength(1);
  expect(screen.getByRole('button', { name: /Choose Dora Developer/ })).toBeVisible();
});

it('keeps restored avatar groups interactive', async () => {
  const input = userEvent.setup();
  const people = <AvatarStack users={[DORA, GRACE]} />;
  const view = show(<Activity mode="visible">{people}</Activity>);
  for (let count = 0; count < 2; count += 1) {
    view.rerender(<Activity mode="hidden">{people}</Activity>);
    view.rerender(<Activity mode="visible">{people}</Activity>);
    expect(screen.getByRole('button', { name: /People: Dora Developer, Grace Hopper/ })).toBeVisible();
  }
  await input.click(screen.getByRole('button', { name: /People: Dora Developer, Grace Hopper/ }));
  const list = await screen.findByRole('dialog', { name: 'People' });
  await input.click(within(list).getByRole('button', { name: DORA.display_name }));
  const profile = await screen.findByRole('dialog', { name: DORA.display_name });
  expect(within(profile).getByText('Full profile')).toBeVisible();
  await input.keyboard('{Escape}');
  expect(screen.queryByRole('dialog', { name: DORA.display_name })).not.toBeInTheDocument();
});

it('opens an avatar group when people arrive after an empty state', async () => {
  const input = userEvent.setup();
  const view = show(<AvatarStack users={[]} />);
  expect(screen.queryByRole('button')).not.toBeInTheDocument();
  view.rerender(<AvatarStack users={[DORA, GRACE]} />);
  await input.click(screen.getByRole('button', { name: /People: Dora Developer, Grace Hopper/ }));
  expect(within(await screen.findByRole('dialog', { name: 'People' })).getByRole('button', { name: GRACE.display_name })).toBeVisible();
});
