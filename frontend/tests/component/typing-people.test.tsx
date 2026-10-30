/** Typing status uses the shared person and people cards while keeping its live sentence. */
import { screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it } from 'vitest';
import { Typing } from '@/features/chat/Typing';
import { DORA, GRACE } from '../doubles/chat-rows';
import { show } from './support';

it('uses a person chip and preserves the live typing sentence', () => {
  show(<Typing chatId="chat" me={null} typing={new Map([[DORA.id, { user: DORA, at: Date.now() }]])} />);
  expect(screen.getByRole('button', { name: DORA.display_name })).toBeVisible();
  expect(screen.getByRole('status')).toHaveTextContent('Dora Developer is typing');
});

it('opens the shared people list for several people typing', async () => {
  const now = Date.now();
  show(<Typing chatId="chat" me={null} typing={new Map([[DORA.id, { user: DORA, at: now }], [GRACE.id, { user: GRACE, at: now }]])} />);
  const input = userEvent.setup();
  await input.click(screen.getByRole('button', { name: /People: Dora Developer, Grace Hopper/ }));
  const list = await screen.findByRole('dialog', { name: 'People' });
  expect(within(list).getByRole('button', { name: DORA.display_name })).toBeVisible();
  expect(within(list).getByRole('button', { name: GRACE.display_name })).toBeVisible();
  expect(screen.getByRole('status')).toHaveTextContent('Dora Developer and Grace Hopper are typing');
});
