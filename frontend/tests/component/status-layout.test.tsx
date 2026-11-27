/** Layout opens the same status editor and restores keyboard focus when it closes. */
import { act, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { ArrangementMenu } from '@/features/workbench/Arrangements';
import { StatusStrip } from '@/features/status-items/StatusStrip';
import { useWorkbench } from '@/features/workbench/store';
import { defaultStatus, moveStatus } from '@/features/status-items/arrangement';
import { show } from './support';

afterEach(() => useWorkbench.setState({ statusItems: defaultStatus() }));

it('fits status items to the observed width without forcing layout when a strip appears', () => {
  const report = new Map<Element, (width: number) => void>();
  const original = globalThis.ResizeObserver;
  vi.stubGlobal('ResizeObserver', class implements ResizeObserver {
    readonly receive: ResizeObserverCallback;
    constructor(receive: ResizeObserverCallback) { this.receive = receive; }
    observe(target: Element) {
      report.set(target, (width) => this.receive([{ target, contentRect: new DOMRect(0, 0, width, 20) } as ResizeObserverEntry], this));
    }
    unobserve(target: Element) { report.delete(target); }
    disconnect = vi.fn();
  });
  const bounds = vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect');
  try {
    const view = show(<StatusStrip items={[{ id: 'one', content: 'One' }, { id: 'two', content: 'Two' }]} />);
    const strip = view.container.querySelector<HTMLElement>('[data-status-strip]')!;
    const [first, last] = strip.querySelectorAll<HTMLElement>('[data-status-item]');
    expect(bounds.mock.contexts).not.toContain(strip);
    act(() => { report.get(strip)!(100); report.get(first!)!(60); report.get(last!)!(60); });
    expect(first).not.toHaveAttribute('aria-hidden');
    expect(last).toHaveAttribute('aria-hidden', 'true');
    act(() => report.get(last!)!(30));
    expect(last).not.toHaveAttribute('aria-hidden');
  } finally {
    bounds.mockRestore();
    vi.stubGlobal('ResizeObserver', original);
  }
});

it('updates status values without measuring the same slots again during render', () => {
  const view = show(<StatusStrip items={[{ id: 'progress', content: 'Starting', text: 'Starting' }]} />);
  const strip = view.container.querySelector<HTMLElement>('[data-status-strip]');
  if (strip === null) throw new Error('Missing status strip');
  const bounds = vi.spyOn(strip, 'getBoundingClientRect');
  view.rerender(<StatusStrip items={[{ id: 'progress', content: 'Running', text: 'Running' }]} />);
  expect(screen.getByText('Running')).toBeVisible();
  expect(bounds).not.toHaveBeenCalled();
});
it('opens Status items from Layout and edits the same saved places', async () => {
  const user = userEvent.setup();
  useWorkbench.setState({ statusItems: moveStatus(defaultStatus(), 'speed', 'header') });
  show(<ArrangementMenu screen="thread" current="conversation" onChoose={vi.fn()} onReset={vi.fn()} />);
  const layout = screen.getByRole('button', { name: 'Layout' });
  await user.click(layout); await user.click(screen.getByRole('menuitem', { name: 'Status items…' }));
  const editor = await screen.findByRole('dialog', { name: 'Status items' });
  await user.click(within(editor).getByRole('combobox', { name: 'Place' }));
  await user.click(screen.getByRole('option', { name: 'Header' }));
  expect(within(editor).getByRole('checkbox', { name: 'Output speed' })).toBeChecked();
  await user.click(within(editor).getByRole('checkbox', { name: 'Output speed' }));
  expect(useWorkbench.getState().statusItems.find((entry) => entry.id === 'speed')).toMatchObject({ place: 'header', shown: false });
  await user.keyboard('{Escape}');
  await waitFor(() => expect(screen.queryByRole('dialog', { name: 'Status items' })).not.toBeInTheDocument());
  expect(layout).toHaveFocus();
});
