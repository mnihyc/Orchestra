/** The list a conversation is drawn in, when it is opened at its end: it holds on to the end while the rows that come into view are measured, and lets go when the list has stopped moving. */
import { render, screen } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { VirtualList } from '@/ui/VirtualList';

/** How tall the window of the list is. */
const WINDOW_PX = 600;

/**
 * The list as the page sees it, without a browser: a content that grows as its rows are measured, a position that cannot be further
 * than the end of it, and frames that come when the test says, each after the time that a machine takes for one.
 */
function machine() {
  const content = { height: 2_000 };
  let top = 0;
  let now = 0;
  let waiting: { id: number; callback: FrameRequestCallback }[] = [];
  let ids = 0;
  const furthest = () => Math.max(0, content.height - WINDOW_PX);
  vi.spyOn(Element.prototype, 'scrollHeight', 'get').mockImplementation(() => content.height);
  vi.spyOn(Element.prototype, 'clientHeight', 'get').mockImplementation(() => WINDOW_PX);
  vi.spyOn(HTMLElement.prototype, 'offsetHeight', 'get').mockImplementation(() => WINDOW_PX);
  vi.spyOn(Element.prototype, 'scrollTop', 'get').mockImplementation(() => top);
  vi.spyOn(Element.prototype, 'scrollTop', 'set').mockImplementation((value: number) => {
    top = Math.max(0, Math.min(value, furthest()));
  });
  vi.spyOn(performance, 'now').mockImplementation(() => now);
  vi.stubGlobal('requestAnimationFrame', (callback: FrameRequestCallback) => {
    ids += 1;
    waiting.push({ id: ids, callback });
    return ids;
  });
  vi.stubGlobal('cancelAnimationFrame', (id: number) => {
    waiting = waiting.filter((entry) => entry.id !== id);
  });
  return {
    /** The rows measured so far make the content this much taller. */
    grow: (pixels: number) => {
      content.height += pixels;
    },
    /** How far the end is from where the list stands. */
    distanceFromEnd: () => furthest() - top,
    /** A frame, after the time it took to come. */
    frame: (milliseconds: number) => {
      now += milliseconds;
      const due = waiting;
      waiting = [];
      for (const entry of due) entry.callback(now);
    },
  };
}

function drawn() {
  render(<VirtualList count={40} keyOf={(index) => `row-${index}`} estimate={() => 50} render={(index) => <p>Row {index}</p>} label="Conversation" />);
  return screen.getByRole('log', { name: 'Conversation' });
}

describe('a list opened at its end', () => {
  beforeEach(() => {
    vi.useRealTimers();
  });

  it('goes on holding the end while rows are still being measured, however few frames a slow machine draws in a second', () => {
    const slow = machine();
    const log = drawn();
    expect(log).toHaveAttribute('aria-busy', 'true');
    // Two frames in a second: thirty frames of rows measured, each making the conversation taller, a quarter of a minute in all.
    for (let frames = 0; frames < 30; frames += 1) {
      slow.grow(300);
      slow.frame(500);
      expect(slow.distanceFromEnd()).toBe(0);
    }
    // The rows have all been measured: the list is at the end, and lets go of it once nothing moves any more.
    for (let frames = 0; frames < 12; frames += 1) slow.frame(500);
    expect(slow.distanceFromEnd()).toBe(0);
    expect(log).not.toHaveAttribute('aria-busy');
  });

  it.each([
    ['at the rate of a machine that is not slow', 16],
    ['on a page whose frames hardly come', 2_000],
  ])('does not hold on for ever to a list that never stops changing, %s', (_, milliseconds) => {
    const endless = machine();
    const log = drawn();
    for (let frames = 0; frames < 400; frames += 1) {
      endless.grow(10);
      endless.frame(milliseconds);
    }
    expect(log).not.toHaveAttribute('aria-busy');
  });
});
