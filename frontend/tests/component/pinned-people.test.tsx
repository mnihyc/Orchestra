/** Pinned-message authors keep their person cards beside the message jump. */
import { screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { Pinned } from '@/features/chat/Pinned';
import { DORA, message } from '../doubles/chat-rows';
import { show } from './support';

vi.mock('@/api/chats', () => ({
  pinnedQuery: () => ({ queryKey: ['pinned-test'], queryFn: () => Promise.resolve([message(1, { author: DORA, parts: [{ type: 'text', text: 'Please read' }] })]) }),
}));

it('opens the author card and keeps a separate jump to the message', async () => {
  const jump = vi.fn();
  const input = userEvent.setup();
  show(<Pinned chatId="chat" onJump={jump} />);
  await input.click(screen.getByRole('button', { name: 'Pinned messages' }));
  const author = await screen.findByRole('button', { name: DORA.display_name });
  expect(author).toBeVisible();
  const row = author.closest('li');
  expect(row).not.toBeNull();
  await input.click(within(row!).getByRole('button', { name: /Open original message.*Please read/ }));
  expect(jump).toHaveBeenCalledTimes(1);
});
