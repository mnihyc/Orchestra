/** Performance is readable, explains missing data and compares and narrows actual model choices. */
import { screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { PerformanceComparison, PerformanceFigures } from '@/features/usage/Performance';
import { UsageChart } from '@/features/usage/Chart';
import { barsOf } from '@/features/usage/series';
import { i18n } from '@/i18n';
import type { UsageAnalytics, UsagePerformance } from '@/generated/api-types';
import { performanceOf, timingFixture } from '../doubles/performance';
import { show } from './support';

const measured = (overrides: Partial<UsagePerformance> = {}): UsagePerformance => ({ ...performanceOf([timingFixture()]), ...overrides });

it('shows median units, the previous span change, error counts and hidden-thinking context', () => {
  show(<PerformanceFigures current={measured({ requests: 10, errors: { provider_busy: 2 }, hidden_requests: 6 })} previous={measured({ speed_milli: 30_643, first_token_ms: 1024, requests: 10 })} before="the 7 days before" />);
  const figures = screen.getByRole('list', { name: 'Performance figures' });
  expect(figures).toHaveTextContent('61 tok/s');
  expect(figures).toHaveTextContent('0.5 s');
  expect(figures).toHaveTextContent('20.0% (2)');
  expect(figures).toHaveTextContent('100');
  expect(figures).toHaveTextContent('the 7 days before');
  expect(figures).toHaveTextContent('Includes thinking');
});

it('keeps an unmeasured speed apart from zero and explains its hidden-thinking reason', async () => {
  show(<PerformanceFigures current={measured({ hidden_requests: 1, speed_milli: null, speed_samples: 0 })} />);
  const card = screen.getByText('Median speed').closest('li');
  expect(card).not.toBeNull();
  expect(card).toHaveTextContent('—');
  expect(card).not.toHaveTextContent('0 tok/s');
  await userEvent.setup().hover(screen.getByRole('button', { name: 'Not measured' }));
  expect(await screen.findByRole('tooltip')).toHaveTextContent('hidden thinking');
});

it('sorts the comparison by a measured column and filters all four choice dimensions', async () => {
  const user = userEvent.setup(), onFilter = vi.fn();
  show(<PerformanceComparison rows={[
    { model: 'swift-route', tier: 'fast', effort: 'high', program: 'research', performance: measured({ requests: 10, speed_milli: 128_000 }) },
    { model: 'steady-route', tier: null, effort: null, program: null, performance: measured({ requests: 20, speed_milli: 32_000 }) },
  ]} modelNames={new Map([['swift-route','Swift'],['steady-route','Steady']])} onFilter={onFilter} />);
  const table = screen.getByRole('table', { name: 'Compare models' });
  expect(within(table).getAllByRole('row')[1]).toHaveTextContent('Steady');
  await user.click(screen.getByRole('button', { name: 'Sort by Median speed' }));
  expect(within(table).getAllByRole('row')[1]).toHaveTextContent('Swift');
  expect(table).toHaveTextContent('128 tok/s');
  await user.click(within(table).getByRole('button', { name: /Swift/ }));
  expect(onFilter).toHaveBeenCalledWith({ model: 'swift-route', tier: 'fast', effort: 'high', program: 'research' });
  expect(table).not.toHaveTextContent('swift-route');expect(table).not.toHaveTextContent('steady-route');
});

it('shows a readable access program and filters with its original id', async () => {
  const onFilter = vi.fn();
  show(<PerformanceComparison rows={[{ model: 'route', tier: null, effort: null, program: 'daybreakRed', performance: measured() }]} modelNames={new Map([['route', 'Model']])} onFilter={onFilter} />);
  const table = screen.getByRole('table', { name: 'Compare models' });
  expect(table).toHaveTextContent('Daybreak Red');
  expect(table).not.toHaveTextContent('daybreakRed');
  const row = within(table).getAllByRole('row')[1];
  if (row === undefined) throw new Error('The model comparison has no row');
  await userEvent.click(within(row).getByRole('button'));
  expect(onFilter).toHaveBeenCalledWith({ model: 'route', tier: 'none', effort: 'none', program: 'daybreakRed' });
});

it('keeps hidden comparison columns readable in the narrow row and sortable from a named control', async () => {
  const user = userEvent.setup();
  show(<PerformanceComparison rows={[{ model: 'Swift', tier: 'fast', effort: 'high', program: 'research', performance: measured({ requests: 10 }) }]} modelNames={new Map([['Swift','Swift']])} onFilter={() => undefined} />);
  const table = screen.getByRole('table', { name: 'Compare models' });
  expect(table).toHaveTextContent('10 requests');
  expect(table).toHaveTextContent('First output: 0.5 s');
  expect(table).toHaveTextContent('Speed 90th: 61 tok/s');
  expect(table).toHaveTextContent('First output 90th: 0.5 s');
  expect(table).toHaveTextContent('research');
  await user.click(screen.getByRole('combobox', { name: 'Sort by' }));
  await user.click(screen.getByRole('option', { name: '90th percentile speed' }));
  expect(screen.getByRole('combobox', { name: 'Sort by' })).toHaveTextContent('90th percentile speed');
  expect(screen.getByRole('button', { name: 'Reverse order' })).toBeEnabled();
});

it('sorts by the displayed model name while filtering by its exact protocol identity',async()=>{
  const onFilter=vi.fn();
  show(<PerformanceComparison rows={[
    {model:'z-route',tier:null,effort:null,program:null,performance:measured()},
    {model:'a-route',tier:null,effort:null,program:null,performance:measured()},
  ]} modelNames={new Map([['z-route','Alpha'],['a-route','Omega']])} onFilter={onFilter} />);
  await userEvent.click(screen.getByRole('button',{name:'Sort by Model'}));
  const table=screen.getByRole('table',{name:'Compare models'});
  expect(within(table).getAllByRole('row')[1]).toHaveTextContent('Alpha');
  await userEvent.click(within(table).getByRole('button',{name:/Alpha/}));
  expect(onFilter).toHaveBeenCalledWith({model:'z-route',tier:'none',effort:'none',program:'none'});
  expect(table).not.toHaveTextContent(/z-route|a-route| · /);
});

it('offers the chart values as a labelled table with median and percentile units', async () => {
  const figures = { turns: 1, tokens: { input_tokens: 100, cached_input_tokens: 0, output_tokens: 64, reasoning_tokens: 0, total_tokens: 164 }, credits: '0', estimated_credits: '0', unrated_tokens: 0, performance: measured() };
  const analytics: UsageAnalytics = { scope: 'me', from: '2026-10-15', to: '2026-10-15', bucket: 'day', depth: 'own', me: null, totals: figures, breakdowns: [], series: [{ at: '2026-10-15T00:00:00Z', figures, by: [] }] };
  show(<UsageChart label="Performance over time" bars={barsOf(analytics, 'speed', 'none', i18n.t)} measure="speed" empty={<span>No measurements</span>} />);
  const toggle = screen.getByRole('button', { name: 'Show as table' });
  await userEvent.setup().click(toggle);
  expect(toggle).toHaveAttribute('aria-expanded', 'true');
  const table = screen.getByRole('table', { name: 'Performance over time: Show as table' });
  expect(within(table).getByRole('columnheader', { name: 'When' })).toBeInTheDocument();
  expect(table).toHaveTextContent('61 tok/s');
  expect(table).toHaveTextContent('90th: 61 tok/s');
});
