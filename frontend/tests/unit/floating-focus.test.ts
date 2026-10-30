/** Nested cards reveal their keyboard target locally without moving the underlying page. */
import { afterEach, expect, it, vi } from 'vitest';
import { revealFloatingFocus } from '@/ui/floating';

afterEach(() => { document.body.replaceChildren(); vi.restoreAllMocks(); });

it('reveals a focus target through its inner list and floating frame, preserving page scroll', () => {
  const layer = document.createElement('div'), list = document.createElement('div'), target = document.createElement('button');
  layer.className = 'floating-layer'; layer.style.overflowY = 'auto'; list.style.overflowY = 'auto';
  list.append(target); layer.append(list); document.body.append(layer);
  for (const node of [layer, list]) Object.defineProperty(node, 'clientHeight', { configurable: true, value: 100 });
  vi.spyOn(layer, 'getBoundingClientRect').mockImplementation(() => new DOMRect(0, 100, 200, 100));
  vi.spyOn(list, 'getBoundingClientRect').mockImplementation(() => new DOMRect(0, 300 - layer.scrollTop, 200, 100));
  vi.spyOn(target, 'getBoundingClientRect').mockImplementation(() => new DOMRect(0, 500 - layer.scrollTop - list.scrollTop, 80, 24));
  const page = document.documentElement.scrollTop; document.documentElement.scrollTop = 91;
  try {
    target.focus(); revealFloatingFocus(target);
    const box = target.getBoundingClientRect();
    for (const frame of [layer, list]) { const rect = frame.getBoundingClientRect(); expect(box.top).toBeGreaterThanOrEqual(rect.top); expect(box.bottom).toBeLessThanOrEqual(rect.top + frame.clientHeight); }
    expect(target).toHaveFocus(); expect(document.documentElement.scrollTop).toBe(91);
  } finally { document.documentElement.scrollTop = page; }
});

it('leaves a page-owned focus target to the page’s own scrolling policy', () => {
  const page = document.createElement('div'), target = document.createElement('button'); page.style.overflowY = 'auto'; page.append(target); document.body.append(page);
  page.scrollTop = 71; revealFloatingFocus(target); expect(page.scrollTop).toBe(71);
});
