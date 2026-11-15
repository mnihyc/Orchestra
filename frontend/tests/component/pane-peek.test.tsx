/** A hidden bar peeks only for a person who asked for it, only after the pointer has rested at its edge, never from a crossing, a drag, a scroll, a press,
 * a control or the pointer's leaving the window; with the choice off nothing opens from the pointer at all. */
import { act, cleanup, render } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { PEEK_DWELL_MS, PEEK_RIGHT_EDGE_PX, usePeek } from '@/features/workbench/peek';

const peeks = vi.fn<(bar: string) => void>();
function Edges({ enabled, bars = ['right'] }: { enabled: boolean; bars?: ('left' | 'right')[] }) {
  usePeek({ enabled, bars, onPeek: peeks });
  return <main data-testid="page" />;
}

const WIDTH = 1440;
function at(x: number, init: { buttons?: number; pointerType?: string; y?: number } = {}): void {
  act(() => {
    const event = new MouseEvent('pointermove', { clientX: x, clientY: init.y ?? 400, buttons: init.buttons ?? 0, bubbles: true });
    Object.defineProperty(event, 'pointerType', { value: init.pointerType ?? 'mouse' });
    document.dispatchEvent(event);
  });
}
const wait = (ms: number) => act(() => { vi.advanceTimersByTime(ms); });
const leaveWindow = () => act(() => { document.documentElement.dispatchEvent(new MouseEvent('mouseleave')); document.documentElement.dispatchEvent(new MouseEvent('pointerleave')); });
const edge = WIDTH - 1;

beforeEach(() => {
  vi.useFakeTimers();
  peeks.mockReset();
  Object.defineProperty(window, 'innerWidth', { configurable: true, value: WIDTH });
  document.elementFromPoint = () => document.body;
});
afterEach(() => { cleanup(); vi.useRealTimers(); });

describe('with hidden bars not shown on hover', () => {
  it('opens nothing when the pointer crosses the edge, rests there for a long time, drags there or leaves the window', () => {
    render(<Edges enabled={false} />);
    at(700); at(edge);
    wait(PEEK_DWELL_MS * 10);
    leaveWindow();
    at(edge, { buttons: 1 });
    wait(PEEK_DWELL_MS * 10);
    expect(peeks).not.toHaveBeenCalled();
  });
});

describe('with hidden bars shown on hover', () => {
  it('peeks after the pointer has rested in the strip for the dwell, and not before', () => {
    render(<Edges enabled />);
    at(700); at(WIDTH - PEEK_RIGHT_EDGE_PX); at(edge);
    wait(PEEK_DWELL_MS - 1);
    expect(peeks).not.toHaveBeenCalled();
    wait(1);
    expect(peeks).toHaveBeenCalledExactlyOnceWith('right');
  });

  it('peeks nothing from a pointer that crosses the strip and goes on, or that leaves the window over it', () => {
    render(<Edges enabled />);
    at(700); at(WIDTH - PEEK_RIGHT_EDGE_PX - 1); at(edge);
    wait(PEEK_DWELL_MS / 2);
    leaveWindow();
    wait(PEEK_DWELL_MS * 5);
    at(edge); wait(PEEK_DWELL_MS / 2); at(WIDTH + 40);
    wait(PEEK_DWELL_MS * 5);
    expect(peeks).not.toHaveBeenCalled();
  });

  it('peeks nothing from a drag, a touch, a press that begins at the edge, a scroll or a window that lost the focus', () => {
    render(<Edges enabled />);
    at(edge, { buttons: 1 }); wait(PEEK_DWELL_MS * 3);
    at(edge, { pointerType: 'touch' }); wait(PEEK_DWELL_MS * 3);
    at(edge); wait(PEEK_DWELL_MS / 2); act(() => { document.dispatchEvent(new MouseEvent('pointerdown', { bubbles: true })); }); wait(PEEK_DWELL_MS * 3);
    at(edge); act(() => { document.dispatchEvent(new Event('scroll')); }); wait(PEEK_DWELL_MS * 3);
    at(edge); wait(PEEK_DWELL_MS / 2); act(() => { window.dispatchEvent(new Event('blur')); }); wait(PEEK_DWELL_MS * 3);
    expect(peeks).not.toHaveBeenCalled();
  });

  it('peeks nothing from a rest on the body of the scroll bar, 3 px or more from the window\'s edge', () => {
    render(<Edges enabled />);
    at(WIDTH - PEEK_RIGHT_EDGE_PX - 1); wait(PEEK_DWELL_MS * 3);
    at(WIDTH - 8); wait(PEEK_DWELL_MS * 3);
    expect(peeks).not.toHaveBeenCalled();
  });

  it('peeks nothing over a control at the edge, and peeks over the page beside it', () => {
    render(<Edges enabled />);
    const button = document.createElement('button'); document.body.append(button);
    document.elementFromPoint = () => button;
    at(edge); wait(PEEK_DWELL_MS * 3);
    expect(peeks).not.toHaveBeenCalled();
    document.elementFromPoint = () => document.body;
    at(edge); wait(PEEK_DWELL_MS);
    expect(peeks).toHaveBeenCalledExactlyOnceWith('right');
  });

  it('peeks over the handle that sizes the bar, which is the last pixel of the window and not a control', () => {
    render(<Edges enabled />);
    const handle = document.createElement('div'); handle.setAttribute('role', 'separator'); handle.tabIndex = 0; document.body.append(handle);
    document.elementFromPoint = () => handle;
    at(edge); wait(PEEK_DWELL_MS);
    expect(peeks).toHaveBeenCalledExactlyOnceWith('right');
  });

  it('peeks the left bar at the edge of the page beside the rail', () => {
    const rail = document.createElement('nav'); rail.setAttribute('data-navigation-rail', '');
    rail.getBoundingClientRect = () => ({ right: 56, left: 0, top: 0, bottom: 900, width: 56, height: 900, x: 0, y: 0, toJSON: () => ({}) });
    document.body.append(rail);
    render(<Edges enabled bars={['left']} />);
    at(300); at(56 + 3);
    wait(PEEK_DWELL_MS);
    expect(peeks).toHaveBeenCalledExactlyOnceWith('left');
    peeks.mockReset();
    at(20); wait(PEEK_DWELL_MS * 3);
    expect(peeks).not.toHaveBeenCalled();
  });
});
