/** Chat list rows keep person cards separate from navigation into a conversation. */
import { screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { AriaAttributes, ReactNode } from 'react';
import { expect, it, vi } from 'vitest';
import { ChatRow } from '@/features/chat/ChatRow';
import { ADA, DORA, GRACE, chat } from '../doubles/chat-rows';
import { show } from './support';

vi.mock('@tanstack/react-router', async (original) => ({
  ...await original<object>(),
  Link: ({ children, className, 'aria-label': label, 'aria-current': current }: { children: ReactNode; className?: string; 'aria-label'?: string; 'aria-current'?: AriaAttributes['aria-current'] }) => <a href="/org/acme/messages" className={className} aria-label={label} aria-current={current}>{children}</a>,
}));

it('opens a direct-chat person separately from the chat link', () => {
  const direct = chat({ kind: 'direct', node: null, name: null, members: [ADA, GRACE] });
  show(<ul><ChatRow chat={direct} slug="acme" selected={false} me={ADA.id} search={{}} /></ul>);
  const person = screen.getByRole('button', { name: GRACE.display_name });
  const link = screen.getByRole('link', { name: 'Open chat with Grace Hopper' });
  expect(person).toBeVisible();
  expect(link).not.toContainElement(person);
});

it('shows the group preview author and opens the shared list of participants', async () => {
  const group = chat({ kind: 'group', node: null, name: 'Release crew', members: [ADA, DORA, GRACE], preview: { id: 'message', author: DORA, text: 'Ready to review', deleted: false } });
  show(<ul><ChatRow chat={group} slug="acme" selected={false} me={ADA.id} search={{}} /></ul>);
  const person = screen.getByRole('button', { name: DORA.display_name });
  const link = screen.getByRole('link', { name: 'Release crew' });
  expect(person).toBeVisible();
  expect(link).not.toContainElement(person);
  expect(screen.getByText('Ready to review')).toBeVisible();
  await userEvent.setup().click(screen.getByRole('button', { name: /People: Dora Developer, Grace Hopper/ }));
  const list = await screen.findByRole('dialog', { name: 'People' });
  expect(within(list).getByRole('button', { name: DORA.display_name })).toBeVisible();
  expect(within(list).getByRole('button', { name: GRACE.display_name })).toBeVisible();
});
