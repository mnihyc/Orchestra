/** A list below the first screen is read when it comes into view, or when its composer is opened. */
import { act, cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type * as Messages from '@/api/adminMessages';
import { MessagesSection } from '@/features/admin-messages/Sent';
import type { AdminMessageListResponse } from '@/generated/api-types';
import { i18n } from '@/i18n';
import { show } from './support';

const calls = vi.hoisted(() => ({ list: vi.fn<(org: string | null) => Promise<AdminMessageListResponse>>() }));
vi.mock('@/api/adminMessages', () => ({
  sendMessage: vi.fn<typeof Messages.sendMessage>(),
  sentMessagesQuery: (org: string | null) => ({ queryKey: ['admin-messages', org ?? 'system'], initialPageParam: null,
    queryFn: () => calls.list(org), getNextPageParam: (page: AdminMessageListResponse) => page.next_cursor }),
  messageQuery: (id: string) => ({ queryKey: ['admin-message', id], queryFn: () => Promise.reject(new Error(id)) }),
}));

const watchers: { callback: IntersectionObserverCallback; element: Element }[] = [];
class Watch {
  constructor(private readonly callback: IntersectionObserverCallback) {}
  observe(element: Element) { watchers.push({ callback: this.callback, element }); }
  disconnect() { /* nothing is watched any more */ }
  unobserve() { /* nothing is watched any more */ }
  takeRecords() { return []; }
}
const before = globalThis.IntersectionObserver;
beforeEach(() => {
  watchers.length = 0;
  calls.list.mockReset().mockResolvedValue({ items: [], next_cursor: null, audience_count: 8 });
  globalThis.IntersectionObserver = Watch as unknown as typeof IntersectionObserver;
});
afterEach(() => { cleanup(); globalThis.IntersectionObserver = before; });

describe('the sent messages of administration', () => {
  it('are not read with the page, but when their section comes into view', async () => {
    show(<MessagesSection scope={{ kind: 'system' }} />);
    expect(watchers).toHaveLength(1);
    await act(async () => { await Promise.resolve(); });
    expect(calls.list).not.toHaveBeenCalled();
    act(() => { watchers[0]?.callback([{ isIntersecting: true, target: watchers[0].element } as IntersectionObserverEntry], {} as IntersectionObserver); });
    await waitFor(() => expect(calls.list).toHaveBeenCalledTimes(1));
  });

  it('are read when the composer is opened before the section has been seen', async () => {
    show(<MessagesSection scope={{ kind: 'system' }} />);
    await userEvent.click(screen.getByRole('button', { name: i18n.t('adminMessages.messageEveryone') }));
    await waitFor(() => expect(calls.list).toHaveBeenCalledTimes(1));
    expect(screen.getByRole('dialog', { name: i18n.t('adminMessages.messageEveryone') })).toBeVisible();
  });
});
