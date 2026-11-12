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
  constructor(protected readonly callback: IntersectionObserverCallback) {}
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

  it('keep their place as the shape of two rows while nothing has been read, and say nothing is loading, for nothing is', async () => {
    const view = show(<MessagesSection scope={{ kind: 'system' }} />);
    await act(async () => { await Promise.resolve(); });
    expect(calls.list).not.toHaveBeenCalled();
    expect(view.container.querySelector('[aria-busy]')).toBeNull();
    expect(screen.queryByRole('status')).toBeNull();
    expect(screen.queryByText(i18n.t('common.loading'))).toBeNull();
    const shapes = view.container.querySelector('[data-read-not-begun]');
    expect(shapes).not.toBeNull();
    expect(shapes?.children).toHaveLength(2);
  });

  it('are loading, and say so, once their section has been seen and the read has begun', async () => {
    calls.list.mockReturnValue(new Promise(() => undefined));
    const view = show(<MessagesSection scope={{ kind: 'system' }} />);
    act(() => { watchers[0]?.callback([{ isIntersecting: true, target: watchers[0].element } as IntersectionObserverEntry], {} as IntersectionObserver); });
    await waitFor(() => expect(calls.list).toHaveBeenCalledTimes(1));
    const busy = await screen.findByRole('status');
    expect(busy).toHaveAttribute('aria-busy', 'true');
    expect(busy).toHaveTextContent(i18n.t('common.loading'));
    expect(view.container.querySelector('[data-read-not-begun]')).toBeNull();
  });

  it('are read when focus enters the section by keyboard, though the view has not scrolled to its list', async () => {
    show(<MessagesSection scope={{ kind: 'system' }} />);
    await act(async () => { await Promise.resolve(); });
    expect(calls.list).not.toHaveBeenCalled();
    await userEvent.tab();
    expect(screen.getByRole('button', { name: i18n.t('adminMessages.messageEveryone') })).toHaveFocus();
    await waitFor(() => expect(calls.list).toHaveBeenCalledTimes(1));
  });

  it('are read at once when their section is already in view as the page opens', async () => {
    class InView extends Watch {
      override observe(element: Element) { this.callback([{ isIntersecting: true, target: element } as IntersectionObserverEntry], this as unknown as IntersectionObserver); }
    }
    globalThis.IntersectionObserver = InView as unknown as typeof IntersectionObserver;
    show(<MessagesSection scope={{ kind: 'system' }} />);
    await waitFor(() => expect(calls.list).toHaveBeenCalledTimes(1));
  });

  it('are read when the composer is opened before the section has been seen', async () => {
    show(<MessagesSection scope={{ kind: 'system' }} />);
    await userEvent.click(screen.getByRole('button', { name: i18n.t('adminMessages.messageEveryone') }));
    await waitFor(() => expect(calls.list).toHaveBeenCalledTimes(1));
    expect(screen.getByRole('dialog', { name: i18n.t('adminMessages.messageEveryone') })).toBeVisible();
  });
});
