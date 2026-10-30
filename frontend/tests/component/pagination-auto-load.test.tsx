/** A list that grows asks for its next page when its foot comes into view, and keeps the button for the keyboard and for a page that failed. */
import { act, cleanup, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';

import { Pagination } from '@/ui/Pagination';

type Watch = (entries: { isIntersecting: boolean }[]) => void;
let watchers: Watch[] = [];

beforeEach(() => {
  watchers = [];
  vi.stubGlobal('IntersectionObserver', class {
    constructor(callback: Watch) { watchers.push(callback); }
    observe() { /* a watcher is told by the test */ }
    disconnect() { /* nothing is held */ }
    unobserve() { /* nothing is held */ }
    takeRecords() { return []; }
  });
});
afterEach(() => { cleanup(); vi.unstubAllGlobals(); });

it('asks for the next page when the foot is in view, and only then', () => {
  const more = vi.fn();
  render(<Pagination autoLoad hasMore loading={false} onMore={more} />);
  expect(more).not.toHaveBeenCalled();
  act(() => watchers.at(-1)?.([{ isIntersecting: false }]));
  expect(more).not.toHaveBeenCalled();
  act(() => watchers.at(-1)?.([{ isIntersecting: true }]));
  expect(more).toHaveBeenCalledTimes(1);
  expect(screen.getByRole('button', { name: 'Show more' })).toBeVisible();
});

it('does not watch while a page is coming, after a failure, or when the list does not ask', () => {
  const more = vi.fn();
  const view = render(<Pagination autoLoad hasMore loading onMore={more} />);
  expect(watchers).toHaveLength(0);
  view.rerender(<Pagination autoLoad hasMore loading={false} error={new Error('no')} onMore={more} />);
  expect(watchers).toHaveLength(0);
  view.rerender(<Pagination hasMore loading={false} onMore={more} />);
  expect(watchers).toHaveLength(0);
});

it('draws nothing at the end of a list', () => {
  const view = render(<Pagination autoLoad hasMore={false} loading={false} onMore={vi.fn()} />);
  expect(view.container).toBeEmptyDOMElement();
});
