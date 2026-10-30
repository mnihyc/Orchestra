/** Real ViewGroup lifecycle with controlled DOM geometry; actual browser layout remains a browser proof. */
import { act, cleanup, fireEvent, screen } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { ViewGroup } from '@/ui/ViewGroup';
import { ViewTab } from '@/ui/ViewTab';
import { show } from './support';

const ids = ['start', 'providers', 'agents', 'machines', 'environments', 'templates', 'hosts', 'mcp', 'mounts'];
const titles = ['Acme', 'Providers', 'Agents', 'Machines', 'Environments', 'Templates', 'Hosts', 'MCP servers', 'Mounts'];
const elementId = (id: string) => `reveal-tab-${id}`;
const observers: ObservedSize[] = [];
class ObservedSize implements ResizeObserver {
  readonly callback: ResizeObserverCallback;
  readonly targets = new Set<Element>();
  disconnected = false;
  constructor(callback: ResizeObserverCallback) { this.callback = callback; observers.push(this); }
  observe(target: Element) { this.targets.add(target); }
  unobserve(target: Element) { this.targets.delete(target); }
  disconnect() { this.disconnected = true; this.targets.clear(); }
  deliver() { this.callback([], this); }
}
function Group({ active = 'mounts', loaded = false, marks = false }: { active?: string; loaded?: boolean; marks?: boolean }) {
  return <ViewGroup label="Retained resources" activeId={active} tabIds={[...ids]}
    tabElementId={elementId} panelId="reveal-body"
    tabInfo={ids.map((id, index) => ({ id, title: titles[index]!, onSelect: () => undefined, onClose: () => undefined }))}
    renderTab={(id, state) => marks
      ? <ViewTab id={elementId(id)} panelId="reveal-body" title={titles[ids.indexOf(id)] ?? 'Screen'} icon={null} active={state.active} tabbable={state.tabbable} menu={[]} onSelect={() => undefined} onClose={() => undefined} />
      : <button id={elementId(id)} role="tab" aria-selected={state.active} tabIndex={state.tabbable ? 0 : -1}><span>{loaded ? titles[ids.indexOf(id)] : 'Screen'}</span></button>}>
    <textarea aria-label="Retained body draft" defaultValue="Original words" />
  </ViewGroup>;
}
function geometry(marks = 0) {
  const strip = screen.getByRole('tablist');
  let width = 420, widths = ids.map(() => 20);
  Object.defineProperty(strip, 'clientWidth', { configurable: true, get: () => width });
  vi.spyOn(strip, 'getBoundingClientRect').mockImplementation(() => new DOMRect(40, 10, width, 32));
  for (let index = 0; index < ids.length; index += 1) {
    const tab = document.getElementById(elementId(ids[index]!));
    if (tab === null) throw new Error('Each real rendered tab must be present');
    const left = () => 40 + widths.slice(0, index).reduce((sum, value) => sum + value, 0) - strip.scrollLeft;
    vi.spyOn(tab, 'getBoundingClientRect').mockImplementation(() => new DOMRect(left(), 10, (widths[index] ?? 0) - marks, 32));
    if (marks > 0) {
      const whole = tab.parentElement;
      if (whole?.parentElement !== strip) throw new Error('The real ViewTab root must contain its separate close mark');
      vi.spyOn(whole, 'getBoundingClientRect').mockImplementation(() => new DOMRect(left(), 10, widths[index], 32));
    }
  }
  return { strip, widen: () => { widths = ids.map(() => 140); }, narrow: () => { width = 260; } };
}
function currentObserver(): ObservedSize {
  const strip = screen.getByRole('tablist'), observer = observers.find((entry) => entry.targets.has(strip) && !entry.disconnected); if (observer === undefined) throw new Error('The current group must observe its actual strip geometry'); return observer;
}
function visible(id: string) {
  const strip = screen.getByRole('tablist'), tab = document.getElementById(elementId(id));
  if (tab === null) throw new Error('The selected tab must exist');
  const viewport = strip.getBoundingClientRect(), box = tab.getBoundingClientRect();
  return box.left >= viewport.left && box.right <= viewport.left + strip.clientWidth;
}
beforeEach(() => { observers.length = 0; vi.stubGlobal('ResizeObserver', ObservedSize); });
afterEach(() => { cleanup(); vi.restoreAllMocks(); vi.unstubAllGlobals(); });

it('reveals the same active tab after restored labels widen without moving focus or body scroll/draft', () => {
  const view = show(<Group />), box = geometry(), observer = currentObserver();
  const body = screen.getByRole('tabpanel'), input = screen.getByRole('textbox', { name: 'Retained body draft' });
  body.scrollTop = 91; input.focus(); fireEvent.change(input, { target: { value: 'Keep these words' } });
  const scrollIntoView = vi.spyOn(Element.prototype, 'scrollIntoView');
  act(() => observer.deliver()); expect(box.strip.scrollLeft).toBe(0); expect(visible('mounts')).toBe(true);
  box.widen(); view.rerender(<Group loaded />);
  expect(currentObserver()).toBe(observer); expect(visible('mounts')).toBe(false);
  expect(observer.targets.has(box.strip)).toBe(true); expect(observer.targets.has(screen.getByRole('tab', { name: 'Mounts' }))).toBe(true);
  act(() => observer.deliver());
  expect(visible('mounts')).toBe(true); expect(box.strip.scrollLeft).toBeGreaterThan(0);
  expect(body.scrollTop).toBe(91); expect(input).toHaveFocus(); expect(input).toHaveValue('Keep these words'); expect(screen.getByRole('textbox')).toBe(input);
  expect(scrollIntoView).not.toHaveBeenCalled();
});

it('keeps the selected tab visible when a restored side bar narrows the strip after selection', () => {
  show(<Group loaded />); const box = geometry(); box.widen();
  act(() => currentObserver().deliver()); const before = box.strip.scrollLeft;
  box.narrow(); expect(visible('mounts')).toBe(false); act(() => currentObserver().deliver());
  expect(box.strip.scrollLeft).toBeGreaterThan(before); expect(visible('mounts')).toBe(true);
});

it('reveals the real ViewTab close mark together with its label at the end of a restored strip', () => {
  show(<Group loaded marks />); const box = geometry(24); box.widen();
  act(() => currentObserver().deliver());
  const selected = screen.getByRole('tab', { name: 'Mounts' }), whole = selected.parentElement;
  if (whole === null) throw new Error('The real selected tab root must exist');
  expect(whole.querySelector('button[aria-label="Close Mounts"]')).not.toBeNull();
  const viewport = box.strip.getBoundingClientRect(), bounds = whole.getBoundingClientRect();
  expect(bounds.left).toBeGreaterThanOrEqual(viewport.left);
  expect(bounds.right).toBeLessThanOrEqual(viewport.left + box.strip.clientWidth);
});

it('preserves explicit horizontal strip browsing through later size/title changes until selection changes', () => {
  const view = show(<Group loaded />), box = geometry(); box.widen(); act(() => currentObserver().deliver());
  fireEvent.wheel(box.strip, { deltaX: -120 }); box.strip.scrollLeft = 0; fireEvent.scroll(box.strip);
  box.narrow(); view.rerender(<Group loaded />); act(() => currentObserver().deliver());
  expect(box.strip.scrollLeft).toBe(0); expect(visible('mounts')).toBe(false);
  view.rerender(<Group loaded active="mcp" />);
  expect(box.strip.scrollLeft).toBeGreaterThan(0); expect(visible('mcp')).toBe(true);
});

it.each(['scrollbar', 'touch', 'tab-drag'] as const)('preserves manual %s browsing without preventing its native event', (kind) => {
  show(<Group loaded />); const box = geometry(); box.widen(); act(() => currentObserver().deliver());
  const event = Object.assign(new Event('pointerdown', { bubbles: true, cancelable: true }), { pointerType: kind === 'touch' ? 'touch' : 'mouse' });
  fireEvent(kind === 'scrollbar' ? box.strip : screen.getByRole('tab', { name: 'Mounts' }), event);
  expect(event.defaultPrevented).toBe(false); box.strip.scrollLeft = 0; box.narrow(); act(() => currentObserver().deliver());
  expect(box.strip.scrollLeft).toBe(0);
});

it('disconnects and retires an old geometry callback after replacement or unmount', () => {
  const view = show(<Group loaded />), box = geometry(); box.widen(); const old = currentObserver();
  act(() => old.deliver()); view.rerender(<Group loaded active="mcp" />); const current = currentObserver();
  expect(current).not.toBe(old); expect(old.disconnected).toBe(true); expect(old.targets.size).toBe(0);
  box.strip.scrollLeft = 0; act(() => old.deliver()); expect(box.strip.scrollLeft).toBe(0);
  act(() => current.deliver()); expect(visible('mcp')).toBe(true);
  view.unmount(); expect(current.disconnected).toBe(true); expect(current.targets.size).toBe(0);
  box.strip.scrollLeft = 17; act(() => { old.deliver(); current.deliver(); }); expect(box.strip.scrollLeft).toBe(17);
});


it('reveals a selected wrapped row within the tab viewport without scrolling its retained body', () => {
  show(<Group loaded marks />); const box = geometry(24);
  const tab = screen.getByRole('tab', { name: 'Mounts' }), whole = tab.parentElement!;
  Object.defineProperty(box.strip, 'clientHeight', { configurable: true, value: 64 });
  vi.spyOn(box.strip, 'getBoundingClientRect').mockImplementation(() => new DOMRect(40, 10, 420, 64));
  vi.spyOn(whole, 'getBoundingClientRect').mockImplementation(() => new DOMRect(40, 266 - box.strip.scrollTop, 140, 32));
  const body = screen.getByRole('tabpanel'), input = screen.getByRole('textbox');
  body.scrollTop = 91; input.focus();
  act(() => currentObserver().deliver());
  const frame = box.strip.getBoundingClientRect(), visible = whole.getBoundingClientRect();
  expect(visible.top).toBeGreaterThanOrEqual(frame.top);
  expect(visible.bottom).toBeLessThanOrEqual(frame.top + box.strip.clientHeight);
  expect(box.strip.scrollTop).toBeGreaterThan(0); expect(body.scrollTop).toBe(91); expect(input).toHaveFocus();
  fireEvent.wheel(box.strip, { deltaY: -120 }); box.strip.scrollTop = 0;
  act(() => currentObserver().deliver()); expect(box.strip.scrollTop).toBe(0);
});
