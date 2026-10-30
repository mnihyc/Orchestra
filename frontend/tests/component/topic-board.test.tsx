/** A status drag keeps a visible destination still even when more content follows the board. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, fireEvent, render, screen, within } from '@testing-library/react';
import type { ReactNode } from 'react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import * as day100 from '@/api/day100';
import { ApiError } from '@/api/client';
import { TopicBoard } from '@/features/project/TopicBoard';
import { openNodeDialog } from '@/features/navigation/NodeDialogs';
import type { Problem, Topic, UserRef } from '@/generated/api-types';
import { toast } from '@/ui/Toast';
import { TooltipProvider } from '@/ui/Tooltip';

vi.mock('@/features/navigation/NodeMenu', async () => {
  const { create } = await import('zustand');
  return {
    NodeMenuButton: () => null,
    useIsPending: () => false,
    usePendingPlaces: create<{ ids: ReadonlySet<string> }>(() => ({ ids: new Set() })),
  };
});
vi.mock('@/features/navigation/NodeDialogs', () => ({ openNodeDialog: vi.fn() }));
vi.mock('@/features/navigation/places', () => ({
  PlaceLink: ({ children }: { children: ReactNode }) => <a href="#topic">{children}</a>,
  placeOfNode: (node: Topic) => node,
}));
vi.mock('@/features/org/ProjectCards', () => ({ AttentionChips: () => null }));
vi.mock('@/features/project/TopicList', () => ({ TopicTags: () => null, topicAttention: () => ({}), lastActivityOf: () => null }));
vi.mock('@/features/navigation/refresh', () => ({ invalidatePlaces: () => Promise.resolve() }));
vi.mock('@/ui/RelativeTime', () => ({ RelativeTime: () => null }));
vi.mock('@/ui/Toast', () => ({ toast: vi.fn() }));

/** jsdom has no layout: two cards sit in the bottom fifth of a scrolling page with a footer below. */
const PAGE = new DOMRect(0, 0, 800, 600);
const ATTENTION = new DOMRect(0, 500, 200, 100);
const RUNNING = new DOMRect(200, 500, 200, 100);
const IDLE = new DOMRect(400, 500, 200, 100);
const DONE = new DOMRect(600, 500, 200, 100);
const CARD = new DOMRect(410, 530, 180, 60);
const TOPICS = [{ id: 'upgrade', kind: 'topic', name: 'Upgrade deps', description: '', status: 'open', thread_display_state: null, archived: false, revision: 7, access: { permissions: ['topic.interact'] }, attention: { blocked: 0, running: 0, waiting_approval: 0 } }] as Topic[];

class TestPointerEvent extends MouseEvent {
  readonly isPrimary = true;
  readonly pointerId = 1;
  readonly pointerType = 'mouse';
}

beforeEach(() => {
  vi.clearAllMocks();
  vi.useFakeTimers();
  vi.stubGlobal('PointerEvent', TestPointerEvent);
  vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function (this: HTMLElement) {
    if (this.tagName === 'LI' || this.classList.contains('cursor-grabbing')) return CARD;
    if (this.getAttribute('aria-labelledby') === 'column-attention') return ATTENTION;
    if (this.getAttribute('aria-labelledby') === 'column-running') return RUNNING;
    if (this.getAttribute('aria-labelledby') === 'column-idle') return IDLE;
    if (this.getAttribute('aria-labelledby') === 'column-done') return DONE;
    return PAGE;
  });
});

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

/** Draw the same scrolling board for a successful write and either kind of refusal. */
function drawBoard() {
  const cache = new QueryClient();
  const view = render(
    <QueryClientProvider client={cache}>
      <TooltipProvider>
        <div data-testid="page" style={{ overflowY: 'auto' }}>
          <TopicBoard topics={TOPICS} canSetStatus orgId="acme" />
          <footer>More page content</footer>
        </div>
      </TooltipProvider>
    </QueryClientProvider>,
  );
  const page = screen.getByTestId('page');
  Object.defineProperties(page, {
    scrollHeight: { value: 1200 },
    clientHeight: { value: PAGE.height },
    scrollWidth: { value: PAGE.width },
    clientWidth: { value: PAGE.width },
  });
  const scroll = vi.fn();
  page.scrollBy = scroll;
  const idle = screen.getByRole('region', { name: /Idle/ });
  return { idle, card: within(idle).getByRole('listitem'), scroll, finish: () => { view.unmount(); cache.clear(); } };
}

function refused(kind: 'access' | 'server'): ApiError {
  const owner: UserRef = { id: 'maintainer', kind: 'human', username: 'maintainer', display_name: 'Project maintainer', avatar: { text: 'M', color: 0 } };
  const code = kind === 'access' ? 'authz.denied' : 'internal';
  const problem: Problem = {
    type: `orch:error:${code}`, code, title: 'Change refused', status: kind === 'access' ? 403 : 500,
    detail: 'The topic was not changed.', request_id: 'board-change-request',
    ...(kind === 'access' ? {
      required: 'topic.interact', requestable: true, override: false,
      missing: [{ object: { kind: 'topic', id: 'upgrade', name: 'Upgrade deps', owner }, permissions: ['topic.interact'], options: null, ask: [owner], ask_more: 0, override: false, closed: null }],
    } : {}),
  };
  return new ApiError(problem, null);
}

describe('a board above more page content', () => {
  it('keeps the page still during a drag near its lower edge and sends Done when the card is dropped', async () => {
    const update = vi.spyOn(day100, 'changeTopicStatus').mockResolvedValue({ ...TOPICS[0], status: 'done', revision: 8 } as Topic);
    const { card, scroll, finish } = drawBoard();
    fireEvent.pointerDown(card, { button: 0, clientX: 500, clientY: 560 });
    fireEvent.pointerMove(document, { clientX: 512, clientY: 572 });
    fireEvent.pointerMove(document, { clientX: 514, clientY: 574 });
    await act(async () => { await vi.advanceTimersByTimeAsync(100); });
    expect(update).not.toHaveBeenCalled();
    expect(scroll).not.toHaveBeenCalled();
    expect(screen.getByRole('status')).toHaveTextContent('Over the Idle column.');

    fireEvent.pointerMove(document, { clientX: 700, clientY: 550 });
    await act(async () => { await vi.advanceTimersByTimeAsync(100); });
    expect(screen.getByRole('status')).toHaveTextContent('Over the Done column.');
    expect(scroll).not.toHaveBeenCalled();
    fireEvent.pointerUp(document);
    await act(async () => { await vi.advanceTimersByTimeAsync(1); });
    expect(update).toHaveBeenCalledExactlyOnceWith('upgrade', 'done', 7);
    finish();
  });

  it.each(['access', 'server'] as const)('keeps a failed %s change in Idle and preserves its details after leaving the board', async (kind) => {
    const error = refused(kind);
    vi.spyOn(day100, 'changeTopicStatus').mockRejectedValue(error);
    const { card, idle, finish } = drawBoard();
    fireEvent.pointerDown(card, { button: 0, clientX: 500, clientY: 560 });
    fireEvent.pointerMove(document, { clientX: 512, clientY: 572 });
    fireEvent.pointerMove(document, { clientX: 514, clientY: 574 });
    await act(async () => { await vi.advanceTimersByTimeAsync(100); });
    fireEvent.pointerMove(document, { clientX: 700, clientY: 550 });
    await act(async () => { await vi.advanceTimersByTimeAsync(100); });
    fireEvent.pointerUp(document);
    await act(async () => { await vi.advanceTimersByTimeAsync(1); });
    expect(within(idle).getByRole('link', { name: 'Upgrade deps' })).toBeVisible();
    const notice = vi.mocked(toast).mock.calls.at(-1)?.[0];
    expect(notice?.action?.label).toBe('Details');
    finish();
    await act(async () => { notice?.action?.run(); await vi.advanceTimersByTimeAsync(1); });
    expect(openNodeDialog).toHaveBeenCalledExactlyOnceWith({ type: 'error', title: 'Upgrade deps could not be changed', error });
  });
});
