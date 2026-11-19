/** Cut values remain readable by pointer and keyboard, including content inserted after startup. */
import { fireEvent } from '@testing-library/dom';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { hasOwnTooltip, installOverflowTooltips, overflowText, OVERFLOW_REST_MS, textOverflows } from '@/ui/overflowTooltips';

const releases: (() => void)[] = [];
let resize: ResizeObserverCallback | undefined;
let observer: ResizeObserver | undefined;
let observeSizes = vi.fn<ResizeObserver['observe']>();
let unobserveSizes = vi.fn<ResizeObserver['unobserve']>();

function label(text = 'The whole value, including its last word') {
  const element = document.createElement('span');
  element.textContent = text;
  element.style.cssText = 'text-overflow:ellipsis;overflow:hidden;white-space:nowrap';
  const size = { width: 100, contentWidth: 250, height: 20, contentHeight: 20 };
  Object.defineProperties(element, {
    clientWidth: { get: () => size.width }, scrollWidth: { get: () => size.contentWidth },
    clientHeight: { get: () => size.height }, scrollHeight: { get: () => size.contentHeight },
  });
  document.body.append(element);
  return { element, size };
}
function install() { const release = installOverflowTooltips(document); releases.push(release); return release; }
const tip = () => document.querySelector('[data-overflow-tooltip]');

function reportResize(element: HTMLElement): void {
  if (resize === undefined || observer === undefined) throw new Error('The overflow reader did not observe sizes.');
  const box: ResizeObserverSize = { inlineSize: element.clientWidth, blockSize: element.clientHeight };
  const entry: ResizeObserverEntry = { target: element, contentRect: new DOMRect(0, 0, box.inlineSize, box.blockSize),
    borderBoxSize: [box], contentBoxSize: [box], devicePixelContentBoxSize: [box] };
  resize([entry], observer);
}

beforeEach(() => {
  vi.useFakeTimers();
  resize = undefined;
  observer = undefined;
  observeSizes = vi.fn<ResizeObserver['observe']>();
  unobserveSizes = vi.fn<ResizeObserver['unobserve']>();
  // A regular constructor function returns the same object the reader receives and the callback uses.
  vi.stubGlobal('ResizeObserver', function createResizeObserver(callback: ResizeObserverCallback): ResizeObserver {
    const created: ResizeObserver = { observe: observeSizes, unobserve: unobserveSizes, disconnect: vi.fn() };
    resize = callback;
    observer = created;
    return created;
  });
});
afterEach(() => {
  for (const release of releases.splice(0)) release();
  document.body.replaceChildren();
  vi.useRealTimers();
});

describe('lost text, rather than a clipping class', () => {
  it('requires a real loss of space and ignores an ordinary horizontal scroller', () => {
    const { element, size } = label();
    size.contentWidth = 100;
    expect(textOverflows(element)).toBe(false);
    size.contentWidth = 101;
    expect(textOverflows(element)).toBe(true);
    element.style.textOverflow = 'clip';
    expect(textOverflows(element)).toBe(false);
    size.width = 0; size.height = 0;
    element.dataset.overflowText = 'A recorded preview';
    expect(textOverflows(element)).toBe(false);
  });
  it('recognizes a vertical line clamp and a character-limited preview with the original retained', () => {
    const { element, size } = label();
    element.style.textOverflow = 'clip';
    element.style.setProperty('-webkit-line-clamp', '2');
    size.contentWidth = 100; size.contentHeight = 40;
    expect(textOverflows(element)).toBe(true);
    element.style.removeProperty('-webkit-line-clamp');
    size.contentHeight = 20;
    element.textContent = 'The beginning…';
    element.dataset.overflowText = 'The beginning and the part the preview left out';
    expect(textOverflows(element)).toBe(true);
    expect(overflowText(element)).toBe(element.dataset.overflowText);
  });
});

describe('the single delegated reader', () => {
  it('reads text only for candidates through a deep tree and retires candidates that stop clipping', async () => {
    const root = document.createElement('section');
    let parent = root;
    // Ninety-six levels with thirty-one ordinary leaves each put over three thousand
    // elements around a handful of shortened values, without relying on elapsed time.
    for (let depth = 0; depth < 96; depth += 1) {
      for (let index = 0; index < 31; index += 1) {
        const ordinary = document.createElement('span');
        ordinary.textContent = `A complete value at ${depth}, ${index}`;
        parent.append(ordinary);
      }
      const child = document.createElement('section');
      parent.append(child);
      parent = child;
    }
    document.body.append(root);
    const clipped = label(), clamped = label(), recorded = label('The beginning…');
    const titled = label('…/work/main'), fitting = label(), excluded = label();
    clamped.element.style.textOverflow = 'clip';
    clamped.element.style.setProperty('-webkit-line-clamp', '2');
    clamped.size.contentWidth = 100; clamped.size.contentHeight = 40;
    recorded.element.style.textOverflow = 'clip';
    recorded.element.dataset.overflowText = 'The beginning and its missing ending';
    titled.element.style.textOverflow = 'clip';
    titled.element.title = '/a/long/workspace/work/main';
    fitting.size.contentWidth = 100;
    excluded.element.hidden = true;
    root.append(clipped.element, recorded.element, fitting.element, excluded.element);
    parent.append(clamped.element, titled.element);
    expect(root.querySelectorAll('*').length).toBeGreaterThan(3_000);
    // This measures text traversal independently of the DOM harness's CSS cascade.
    vi.spyOn(window, 'getComputedStyle').mockImplementation((element) => {
      if (!(element instanceof HTMLElement)) throw new Error('Expected an HTML text value.');
      return element.style;
    });
    const reads = vi.spyOn(Node.prototype, 'textContent', 'get');
    const wanted = [clipped.element, clamped.element, recorded.element, titled.element, fitting.element];
    install();
    if (observer === undefined) throw new Error('The overflow reader did not observe sizes.');
    expect(new Set(observeSizes.mock.calls.map(([element]) => element))).toEqual(new Set(wanted));
    for (const element of wanted.slice(0, 4)) expect(element).toHaveAttribute('tabindex', '0');
    expect(fitting.element).not.toHaveAttribute('tabindex');
    expect(excluded.element).not.toHaveAttribute('tabindex');

    clipped.element.style.textOverflow = 'clip';
    fireEvent.resize(window);
    await Promise.resolve();
    vi.advanceTimersByTime(20);
    expect(observeSizes).toHaveBeenCalledTimes(wanted.length);
    expect(unobserveSizes).toHaveBeenCalledTimes(1);
    expect(unobserveSizes).toHaveBeenCalledWith(clipped.element);
    expect(clipped.element).not.toHaveAttribute('tabindex');
    for (const element of wanted.slice(1, 4)) expect(element).toHaveAttribute('tabindex', '0');
    const candidateReaders = new Set<unknown>(wanted);
    const actualReaders = new Set(reads.mock.contexts);
    expect([...actualReaders].filter((element) => !candidateReaders.has(element)).length).toBe(0);
    expect(actualReaders.size).toBe(wanted.length);
  });
  it('shows literal full text after a pointer rests, and never parses it as HTML', () => {
    const { element } = label('完整的名字 <img src=x onerror=alert(1)> and the end');
    install();
    fireEvent.pointerOver(element, { pointerType: 'mouse' });
    vi.advanceTimersByTime(OVERFLOW_REST_MS - 1);
    expect(tip()).toBeNull();
    vi.advanceTimersByTime(1);
    expect(tip()).toHaveTextContent(element.textContent);
    expect(tip()?.querySelector('img')).toBeNull();
    fireEvent.pointerOut(element, { relatedTarget: document.body });
    vi.advanceTimersByTime(101);
    expect(tip()).toBeNull();
  });
  it('makes static cut text a keyboard stop and preserves an unrelated description', () => {
    const { element } = label();
    element.setAttribute('aria-describedby', 'field-hint');
    install();
    expect(element).toHaveAttribute('tabindex', '0');
    element.focus();
    expect(tip()).toHaveTextContent(overflowText(element));
    expect(element.getAttribute('aria-describedby')?.split(' ')).toEqual(['field-hint', tip()?.id]);
    element.blur();
    expect(tip()).toBeNull();
    expect(element).toHaveAttribute('aria-describedby', 'field-hint');
  });
  it('follows a link focus into its cut label without adding an inner keyboard stop', () => {
    const { element } = label();
    const link = document.createElement('a'); link.href = '/somewhere';
    document.body.append(link); link.append(element);
    install();
    expect(element).not.toHaveAttribute('tabindex');
    link.focus();
    expect(tip()).toHaveTextContent(overflowText(element));
    expect(link).toHaveAttribute('aria-describedby', tip()?.id);
  });
  it('does not mistake a focusable pane for the keyboard owner of every value inside it', () => {
    const { element } = label(); const pane = document.createElement('section'); pane.tabIndex = 0;
    document.body.append(pane); pane.append(element); install();
    expect(element).toHaveAttribute('tabindex', '0');
    pane.focus(); expect(tip()).toBeNull();
    element.focus(); expect(tip()).toHaveTextContent(overflowText(element));
  });
  it('finds a cut control label when the pointer lands on its SVG arrow', () => {
    const { element } = label(); const button = document.createElement('button');
    const arrow = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
    document.body.append(button); button.append(element, arrow); install();
    fireEvent.pointerOver(arrow); vi.advanceTimersByTime(OVERFLOW_REST_MS);
    expect(tip()).toHaveTextContent(overflowText(element));
  });
  it('respects an explicit shared tooltip even before that tooltip opens', () => {
    const { element } = label();
    element.setAttribute('data-tooltip-trigger', '');
    install();
    expect(hasOwnTooltip(element)).toBe(true);
    fireEvent.pointerOver(element); vi.advanceTimersByTime(OVERFLOW_REST_MS);
    element.focus();
    expect(tip()).toBeNull();
  });
  it('supplies keyboard coverage when a child tooltip cannot receive its parent link focus', () => {
    const { element } = label(); element.setAttribute('data-tooltip-trigger', '');
    const link = document.createElement('a'); link.href = '/somewhere'; document.body.append(link); link.append(element);
    install(); link.focus();
    expect(tip()).toHaveTextContent(overflowText(element));
    expect(link).toHaveAttribute('aria-describedby', tip()?.id);
  });
  it('reveals every clipped label owned by one keyboard control', () => {
    const one = label('The complete first label'), two = label('The complete second label');
    const button = document.createElement('button'); document.body.append(button); button.append(one.element, two.element);
    install(); button.focus();
    expect(tip()).toHaveTextContent('The complete first label');
    expect(tip()).toHaveTextContent('The complete second label');
  });
  it('respects an existing ARIA tooltip but does not mistake field help for one', () => {
    const { element } = label();
    const own = document.createElement('div'); own.id = 'own-tip'; own.setAttribute('role', 'tooltip');
    document.body.append(own); element.setAttribute('aria-describedby', own.id);
    expect(hasOwnTooltip(element)).toBe(true);
    own.setAttribute('role', 'note');
    expect(hasOwnTooltip(element)).toBe(false);
  });
  it('keeps a complete native title on hover and supplies its missing keyboard equivalent', () => {
    const { element } = label(); element.title = overflowText(element);
    install();
    fireEvent.pointerOver(element); vi.advanceTimersByTime(OVERFLOW_REST_MS);
    expect(tip()).toBeNull();
    element.focus();
    expect(tip()).toHaveTextContent(element.title);
    expect(element).toHaveAttribute('title', overflowText(element));
  });
  it('keeps the tooltip while the pointer crosses to read its own scrollable content', () => {
    const { element } = label(); install();
    fireEvent.pointerOver(element); vi.advanceTimersByTime(OVERFLOW_REST_MS);
    const visible = tip(); expect(visible).not.toBeNull();
    fireEvent.pointerOut(element, { relatedTarget: visible });
    if (visible !== null) fireEvent.pointerOver(visible);
    vi.advanceTimersByTime(101);
    expect(tip()).toBe(visible);
  });
  it('lets a new pointer target reveal its value while another clipped value still has focus', () => {
    const one = label('A focused first value'), two = label('A hovered second value');
    install(); one.element.focus();
    fireEvent.pointerOver(two.element); vi.advanceTimersByTime(OVERFLOW_REST_MS);
    expect(tip()).toHaveTextContent('A hovered second value');
  });
  it('dismisses with Escape without trapping focus or reopening on the same focused value', () => {
    const { element } = label(); install(); element.focus();
    expect(tip()).not.toBeNull();
    fireEvent.keyDown(element, { key: 'Escape' });
    expect(tip()).toBeNull(); expect(element).toHaveFocus();
    fireEvent.pointerOver(element); vi.advanceTimersByTime(OVERFLOW_REST_MS);
    expect(tip()).toBeNull();
    fireEvent.pointerOut(element, { relatedTarget: document.body }); fireEvent.pointerOver(document.body);
    vi.advanceTimersByTime(101);
    expect(tip()).toBeNull();
    element.blur(); element.focus();
    expect(tip()).toHaveTextContent(overflowText(element));
  });
  it('learns newly inserted cut text without reinstalling the handler', async () => {
    install(); const { element } = label('A later route and its full name');
    await Promise.resolve(); vi.advanceTimersByTime(20);
    expect(element).toHaveAttribute('tabindex', '0');
    element.focus(); expect(tip()).toHaveTextContent('A later route and its full name');
  });
  it('updates the whole value while a streamed label changes', async () => {
    const { element } = label('The first full value'); install(); element.focus();
    element.textContent = 'The later full value including its new last word';
    await Promise.resolve(); vi.advanceTimersByTime(20);
    expect(tip()).toHaveTextContent('The later full value including its new last word');
  });
  it('reads the latest value if it changed while the hover delay was still pending', async () => {
    const { element } = label('Before the hover pause'); install(); fireEvent.pointerOver(element);
    element.textContent = 'After the hover pause, including the new ending';
    await Promise.resolve(); vi.advanceTimersByTime(OVERFLOW_REST_MS);
    expect(tip()).toHaveTextContent('After the hover pause, including the new ending');
  });
  it('uses the full native title for a manually shortened directory even when it fits', () => {
    const { element, size } = label('…/work/main'); size.contentWidth = 90;
    element.title = '/a/long/team/workspace/work/main'; install(); element.focus();
    expect(tip()).toHaveTextContent(element.title);
  });
  it('removes only keyboard stops it added when resizing makes a value fit', () => {
    const { element, size } = label(); install();
    expect(element).toHaveAttribute('tabindex', '0');
    size.contentWidth = 100;
    reportResize(element);
    expect(element).not.toHaveAttribute('tabindex');
    const second = label(); second.element.tabIndex = -1;
    reportResize(second.element);
    expect(second.element).toHaveAttribute('tabindex', '-1');
  });
  it('shares one portal across nested providers and restores the page on the last release', () => {
    const { element } = label(); install();
    const nested = installOverflowTooltips(document);
    element.focus(); expect(document.querySelectorAll('[data-overflow-tooltip]')).toHaveLength(1);
    releases.pop()?.();
    expect(tip()).not.toBeNull();
    nested();
    expect(tip()).toBeNull();
    expect(element).not.toHaveAttribute('tabindex');
    expect(element).not.toHaveAttribute('aria-describedby');
    fireEvent.pointerOver(element); vi.advanceTimersByTime(OVERFLOW_REST_MS);
    expect(tip()).toBeNull();
  });
});
