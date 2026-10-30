/** Content-sized tables retain facts, responsive summaries and keyboard acts in both languages and themes. */
import { act, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';

import { i18n, setLocale } from '@/i18n';
import { Checkbox } from '@/ui/Checkbox';
import { Table, type Column } from '@/ui/Table';

import { show } from './support';

let room = 1440;
const observers = new Set<() => void>();

beforeEach(() => {
  room = 1440;
  vi.spyOn(Element.prototype, 'clientWidth', 'get').mockImplementation(function (this: Element) {
    return this.getAttribute('data-scroll-allow') === 'table' ? room : 0;
  });
  vi.stubGlobal('ResizeObserver', class implements ResizeObserver {
    private readonly report: () => void;
    constructor(callback: ResizeObserverCallback) { this.report = () => callback([], this); }
    observe() { observers.add(this.report); }
    unobserve() { observers.delete(this.report); }
    disconnect() { observers.delete(this.report); }
  });
});

afterEach(async () => {
  observers.clear();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  await setLocale('en');
});

function resize(width: number) {
  act(() => { room = width; for (const report of observers) report(); });
}

interface Row { id: string; name: string; version: string; machine: string; image: string }
const row: Row = { id: 'profile', name: 'Team profile', version: 'The default build', machine: 'world2-sub', image: 'gcr.io/distroless/static-debian12:nonroot' };

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  it(`keeps full values once and folds facts into their row at a narrow width (${locale}, ${theme})`, async () => {
    await setLocale(locale);
    const values = { ...row, version: i18n.t('agents.list.pinDefault'), machine: i18n.t('agents.list.anyRuntime') };
    const columns: Column<Row>[] = [
      { id: 'name', header: i18n.t('agents.list.name'), cell: (item, shown) => <div>
        <a href="#profile">{item.name}</a>
        {!shown.has('version') && <p>{item.version}</p>}
        {!shown.has('machine') && <p>{item.machine}</p>}
        {!shown.has('image') && <p>{item.image}</p>}
      </div> },
      { id: 'version', header: i18n.t('agents.list.pin'), from: 'md', cell: (item) => item.version },
      { id: 'machine', header: i18n.t('agents.list.usable'), from: 'sm', cell: (item) => item.machine },
      { id: 'image', header: i18n.t('templates.list.source'), from: 'lg', cell: (item) => <code>{item.image}</code> },
    ];
    show(<div data-theme={theme}><Table label={i18n.t('agents.list.label')} columns={columns} rows={[values]} rowKey={(item) => item.id} /></div>);
    const table = screen.getByRole('table', { name: i18n.t('agents.list.label') });
    expect(within(table).getByRole('columnheader', { name: i18n.t('agents.list.pin') })).toBeVisible();
    for (const value of [values.name, values.version, values.machine, values.image]) expect(within(table).getAllByText(value, { exact: true })).toHaveLength(1);
    resize(400);
    expect(within(table).getAllByRole('columnheader')).toHaveLength(1);
    for (const value of [values.name, values.version, values.machine, values.image]) expect(within(table).getAllByText(value, { exact: true })).toHaveLength(1);
    resize(1440);
    expect(within(table).getAllByRole('columnheader')).toHaveLength(4);
    for (const value of [values.version, values.machine, values.image]) expect(within(table).getAllByText(value, { exact: true })).toHaveLength(1);
    const scroller = screen.getByRole('region', { name: i18n.t('agents.list.label') });
    scroller.focus();
    expect(scroller).toHaveFocus();
    expect(scroller).toHaveAttribute('data-scroll-allow', 'table');
  });

  it(`keeps selection and the row's keyboard action attached to a named row (${locale}, ${theme})`, async () => {
    await setLocale(locale);
    const choose = vi.fn();
    const open = vi.fn();
    const columns: Column<Row>[] = [
      { id: 'select', grow: false, header: i18n.t('resources.bulk.selectAll'), hideHeader: true,
        cell: (item) => <Checkbox label={i18n.t('resources.bulk.select', { name: item.name })} checked={false} onCheckedChange={choose} /> },
      { id: 'subject', grow: true, header: i18n.t('agents.list.name'), cell: (item) => <a href="#profile">{item.name}</a> },
      { id: 'version', header: i18n.t('agents.list.pin'), from: 'md', cell: (item) => item.version },
    ];
    show(<div data-theme={theme}><Table label={i18n.t('agents.list.label')} columns={columns} rows={[row]} rowKey={(item) => item.id}
      rowMenu={{ label: (item) => item.name, entries: () => [], keys: () => ({ F2: open }) }} /></div>);
    const user = userEvent.setup();
    const selectionName = i18n.t('resources.bulk.select', { name: row.name });
    await user.click(screen.getByRole('checkbox', { name: selectionName }));
    expect(choose).toHaveBeenCalledWith(true);
    const link = screen.getByRole('link', { name: row.name });
    link.focus();
    await user.keyboard('{F2}');
    expect(open).toHaveBeenCalledOnce();
    resize(400);
    expect(screen.getByRole('checkbox', { name: selectionName })).toBeVisible();
    expect(screen.getByRole('link', { name: row.name })).toBeVisible();
    expect(screen.queryByRole('columnheader', { name: i18n.t('agents.list.pin') })).not.toBeInTheDocument();
  });
}
