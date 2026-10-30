/** A quote's author and the link to the quoted item stay separately interactive. */
import { screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { QuotedItem } from '@/features/chat/QuotedItem';
import { DORA } from '../doubles/chat-rows';
import { userMessage } from '../doubles/thread-rows';
import { show } from './support';

const nav = vi.hoisted(() => ({ open: vi.fn() }));
vi.mock('@/features/chat/open', () => ({ isPlainClick: () => true, useOpenPath: () => nav.open }));
vi.mock('@/api/chats', () => ({
  quotedItemQuery: () => ({ queryKey: ['quote-test'], queryFn: () => Promise.resolve(userMessage('item', 'Earlier words', { author: DORA })) }),
}));

it('opens the author card without nesting it in the original-item link', async () => {
  const input = userEvent.setup();
  show(<QuotedItem threadId="thread" itemId="item" />);
  const author = await screen.findByRole('button', { name: DORA.display_name });
  const link = screen.getByRole('link', { name: 'Go to the quoted item in the conversation' });
  expect(link).not.toContainElement(author);
  await input.click(link);
  expect(nav.open).toHaveBeenCalledWith(expect.stringContaining('/thread/thread?item=item'));
});
