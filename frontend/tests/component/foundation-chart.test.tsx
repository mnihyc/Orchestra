/** The retained chart gives its zero and peak as words and data even before a canvas can be drawn. */
import { screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { TimeSeriesChart } from '@/ui/TimeSeriesChart';
import { show } from './support';

afterEach(() => { vi.unstubAllGlobals(); });

it('shows the first table rows and expands the rest in the page', async () => {
  vi.stubGlobal('ResizeObserver', class { observe() { return undefined; } disconnect() { return undefined; } });
  const user = userEvent.setup();
  const from = Date.parse('2026-10-14T12:00:00Z');
  const points: [number, number][] = Array.from({ length: 12 }, (_, index) => [from + index * 1000, index]);
  show(<TimeSeriesChart label="Runtime" lines={[{ id: 'cpu', label: 'CPU', slot: 1, points }]} format={(value) => `${value}%`} from={from} to={from + 12000} words={{ table: 'Data table', time: 'Time', now: 'Current', peak: 'Peak', keys: 'Use the arrow keys' }} empty="No observations" />);
  await user.click(screen.getByText('Data table', { exact: true }));
  const table = screen.getByRole('table', { name: 'Runtime: Data table' });
  expect(within(table).getAllByRole('row')).toHaveLength(7);
  expect(within(table).queryByRole('cell', { name: '11%' })).not.toBeInTheDocument();
  await user.click(screen.getByRole('button', { name: 'Show all' }));
  expect(within(table).getAllByRole('row')).toHaveLength(13);
  expect(within(table).getByRole('cell', { name: '11%' })).toBeVisible();
  await user.click(screen.getByRole('button', { name: 'Show less' }));
  expect(within(table).getAllByRole('row')).toHaveLength(7);
});

it('keeps the current zero, peak and data table readable without a rendered chart', async () => {
  vi.stubGlobal('ResizeObserver', class { observe() { return undefined; } disconnect() { return undefined; } });
  const user = userEvent.setup();
  const from = Date.parse('2026-10-14T12:00:00Z');
  const to = Date.parse('2026-10-14T12:01:00Z');
  show(<TimeSeriesChart label="Runtime" lines={[{ id: 'cpu', label: 'CPU', slot: 1, points: [[from, 8], [to, 0]] }]} format={(value) => `${value}%`} from={from} to={to} words={{ table: 'Data table', time: 'Time', now: 'Current', peak: 'Peak', keys: 'Use the arrow keys' }} empty="No observations" />);
  expect(screen.getByText(/Current 0%.*Peak 8%/)).toBeInTheDocument();
  await user.click(screen.getByText('Data table', { exact: true }));
  const table = screen.getByRole('table', { name: 'Runtime: Data table' });
  expect(within(table).getByRole('cell', { name: '0%' })).toBeInTheDocument();
  expect(within(table).getByRole('cell', { name: '8%' })).toBeInTheDocument();
});
