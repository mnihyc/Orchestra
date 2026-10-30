/** A long table keeps native table rows while showing only a measured window of them. */
import { fireEvent, screen, within } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { Table } from '@/ui/Table';
import { show } from './support';

it('keeps the first and later rows reachable without mounting the whole list', () => {
  vi.spyOn(HTMLElement.prototype, 'offsetHeight', 'get').mockImplementation(function (this: HTMLElement) {
    return this.tagName === 'TR' ? 72 : 384;
  });
  vi.spyOn(HTMLElement.prototype, 'offsetWidth', 'get').mockReturnValue(600);
  const rows = Array.from({ length: 200 }, (_, index) => ({ id: String(index), name: `Row ${index}` }));
  show(<Table label="Long report" rows={rows} rowKey={(row) => row.id} virtual columns={[{ id: 'name', header: 'Item', cell: (row) => <a href={`#${row.id}`}>{row.name}</a> }]} />);
  const table = screen.getByRole('table', { name: 'Long report' });
  const region = screen.getByRole('region', { name: 'Long report' });
  expect(table).toHaveAttribute('aria-rowcount', '201');
  expect(within(table).getByText('Row 0')).toBeVisible();
  expect(within(table).queryByText('Row 100')).not.toBeInTheDocument();
  expect(within(table).getAllByRole('row').length).toBeLessThan(30);
  fireEvent.scroll(region, { target: { scrollTop: 7_200 } });
  expect(within(table).getByText('Row 100')).toBeVisible();
  expect(within(table).queryByText('Row 0')).not.toBeInTheDocument();
  expect(within(table).getAllByRole('row').length).toBeLessThan(30);
  fireEvent.scroll(region, { target: { scrollTop: 0 } });
  fireEvent.focus(within(table).getByText('Row 0'));
  fireEvent.scroll(region, { target: { scrollTop: 7_200 } });
  expect(within(table).getByText('Row 0')).toBeInTheDocument();
  expect(within(table).getByText('Row 100')).toBeVisible();
  expect(within(table).getByText('Row 100').closest('tr')).toHaveAttribute('aria-rowindex', '102');
});

it('keeps short tables entirely available without virtualizing', () => {
  const rows = [{ id: 'one', name: 'One' }, { id: 'two', name: 'Two' }];
  show(<Table label="Short report" rows={rows} rowKey={(row) => row.id} virtual columns={[{ id: 'name', header: 'Item', cell: (row) => row.name }]} />);
  expect(screen.getByText('One')).toBeVisible();
  expect(screen.getByText('Two')).toBeVisible();
  expect(screen.getByRole('table', { name: 'Short report' })).not.toHaveAttribute('aria-rowcount');
});

it('keeps the scroll position when more pages turn a table into a window', () => {
  vi.spyOn(HTMLElement.prototype, 'offsetHeight', 'get').mockImplementation(function (this: HTMLElement) {
    return this.tagName === 'TR' ? 72 : 384;
  });
  vi.spyOn(HTMLElement.prototype, 'offsetWidth', 'get').mockReturnValue(600);
  const rows = Array.from({ length: 100 }, (_, index) => ({ id: String(index), name: `Row ${index}` }));
  const table = (count: number) => <Table label="Growing report" rows={rows.slice(0, count)} rowKey={(row) => row.id} virtual columns={[{ id: 'name', header: 'Item', cell: (row) => row.name }]} />;
  const view = show(table(50));
  const region = screen.getByRole('region', { name: 'Growing report' });
  fireEvent.scroll(region, { target: { scrollTop: 720 } });
  view.rerender(table(100));
  expect(region.scrollTop).toBe(720);
  expect(screen.getByText('Row 10')).toBeVisible();
});
