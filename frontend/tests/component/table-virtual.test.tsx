/** A long table keeps native table rows while showing only a measured window of them. */
import { fireEvent, screen, within } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { Table } from '@/ui/Table';
import { CompleteCollection } from '@/ui/CompleteCollection';
import { show } from './support';

it('windows a first server page that already overflows its viewport', () => {
  vi.spyOn(HTMLElement.prototype, 'offsetHeight', 'get').mockImplementation(function (this: HTMLElement) { return this.tagName === 'TR' ? 72 : 384; });
  vi.spyOn(HTMLElement.prototype, 'offsetWidth', 'get').mockReturnValue(600);
  const rows = Array.from({ length: 37 }, (_, index) => ({ id: String(index), name: `Page item ${index}` }));
  show(<Table label="First page" rows={rows} rowKey={(row) => row.id} virtual columns={[{ id: 'name', header: 'Name', cell: (row) => row.name }]} />);
  const table = screen.getByRole('table', { name: 'First page' });
  expect(within(table).getAllByRole('row').length).toBeLessThan(30);
  expect(within(table).queryByText('Page item 36')).toBeNull();
  fireEvent.scroll(screen.getByRole('region', { name: 'First page' }), { target: { scrollTop: 2_592 } });
  expect(within(table).getByText('Page item 36')).toBeVisible();
});

it('gives an ordinary growing table its own bounded scroll without discarding row state', () => {
  const rows = Array.from({ length: 11 }, (_, index) => ({ id: String(index), name: `Item ${index}` }));
  show(<Table label="Editable report" rows={rows} rowKey={(row) => row.id} columns={[{ id: 'name', header: 'Name', cell: (row) => <input aria-label={row.name} defaultValue={row.name} /> }]} />);
  expect(screen.getByRole('region', { name: 'Editable report' })).toHaveClass('max-h-96', 'overflow-y-auto');
  expect(screen.getAllByRole('textbox')).toHaveLength(11);
});

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

it('keeps an active editor when focus leaves for its dialog and the rows scroll away', () => {
  vi.spyOn(HTMLElement.prototype, 'offsetHeight', 'get').mockImplementation(function (this: HTMLElement) { return this.tagName === 'TR' ? 72 : 384; });
  vi.spyOn(HTMLElement.prototype, 'offsetWidth', 'get').mockReturnValue(600);
  const rows = Array.from({ length: 200 }, (_, index) => ({ id: String(index), name: `Editor ${index}` }));
  const draw = (keep: ReadonlySet<string>) => <Table label="Editors" rows={rows} rowKey={(row) => row.id} virtual retainKeys={keep} columns={[{ id: 'name', header: 'Name', cell: (row) => <input aria-label={row.name} defaultValue={row.name} /> }]} />;
  const view = show(draw(new Set(['0'])));
  fireEvent.change(screen.getByRole('textbox', { name: 'Editor 0' }), { target: { value: 'Unfinished editor' } });
  fireEvent.scroll(screen.getByRole('region', { name: 'Editors' }), { target: { scrollTop: 7_200 } });
  expect(screen.getByRole('textbox', { name: 'Editor 0' })).toHaveValue('Unfinished editor');
  expect(screen.getByRole('textbox', { name: 'Editor 100' })).toBeInTheDocument();
  expect(screen.getAllByRole('textbox').length).toBeLessThan(30);
  view.rerender(draw(new Set()));
  expect(screen.queryByRole('textbox', { name: 'Editor 0' })).toBeNull();
});

it('keeps logical focus on its row when a live change reorders it outside the window', () => {
  vi.spyOn(HTMLElement.prototype, 'offsetHeight', 'get').mockImplementation(function (this: HTMLElement) { return this.tagName === 'TR' ? 72 : 384; });
  vi.spyOn(HTMLElement.prototype, 'offsetWidth', 'get').mockReturnValue(600);
  const rows = Array.from({ length: 200 }, (_, index) => ({ id: String(index), name: `Live ${index}` }));
  const draw = (items: typeof rows) => <Table label="Live editors" rows={items} rowKey={(row) => row.id} virtual columns={[{ id: 'name', header: 'Name', cell: (row) => <input aria-label={row.name} defaultValue={row.name} /> }]} />;
  const view = show(draw(rows));
  const first = screen.getByRole('textbox', { name: 'Live 0' });
  fireEvent.change(first, { target: { value: 'Current draft' } }); fireEvent.focus(first);
  const head = rows[0]; if (head === undefined) throw new Error('The first native row exists');
  view.rerender(draw([...rows.slice(1), head]));
  expect(screen.getByRole('textbox', { name: 'Live 0' })).toBe(first);
  expect(first).toHaveValue('Current draft');
});

it('finds a late declaration without dropping the rest of its complete source', () => {
  const items = Array.from({ length: 1_001 }, (_, index) => ({ id: String(index), name: `Declaration ${index}` }));
  show(<CompleteCollection label="Declarations" items={items} textOf={(row) => row.name} scroll>
    {(rows) => <ul>{rows.map((row) => <li key={row.id}>{row.name}</li>)}</ul>}
  </CompleteCollection>);
  expect(screen.queryByText('Declaration 1000')).toBeNull();
  fireEvent.click(screen.getByRole('button', { name: 'Next' }));
  expect(screen.getByText('Declaration 50')).toBeVisible();
  expect(screen.queryByText('Declaration 0')).toBeNull();
  expect(screen.getAllByRole('listitem')).toHaveLength(50);
  const search = screen.getByRole('searchbox', { name: 'Find in Declarations' });
  fireEvent.change(search, { target: { value: 'Declaration 1000' } });
  expect(screen.getByText('Declaration 1000')).toBeVisible();
  expect(screen.getByRole('region', { name: 'Find in Declarations' })).toHaveClass('max-h-96', 'overflow-y-auto');
  fireEvent.change(search, { target: { value: '' } });
  expect(screen.getByText('Declaration 0')).toBeVisible();
  expect(screen.queryByText('Declaration 1000')).toBeNull();
  expect(items).toHaveLength(1_001);
});
