/** Shared page controls keep one action, readable states, reachable sections and explicit dates in both languages and themes. */
import { cleanup, fireEvent, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { afterEach, expect, it, vi } from 'vitest';

import { ParticipationFilter } from '@/features/access/ParticipationFilter';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { PageHeader } from '@/shell/Page';
import { actionName } from '@/ui/actions';
import { Badge } from '@/ui/Badge';
import { Button } from '@/ui/Button';
import { DateField, type DateFieldProps } from '@/ui/DateField';
import { FilterBar } from '@/ui/FilterBar';
import { Input } from '@/ui/Input';
import { SectionNav } from '@/ui/SectionNav';
import { Select } from '@/ui/Select';
import { StateChip } from '@/ui/StateChip';
import { StatusDot } from '@/ui/StatusDot';
import { show } from './support';

const modes = [['en', 'light'], ['en', 'dark'], ['zh-CN', 'light'], ['zh-CN', 'dark']] as const;
const appearance = useAppearance.getState();
const themeBefore = document.documentElement.dataset.theme;
const darkBefore = document.documentElement.classList.contains('dark');

afterEach(async () => {
  cleanup(); useAppearance.setState(appearance);
  document.documentElement.classList.toggle('dark', darkBefore);
  if (themeBefore === undefined) delete document.documentElement.dataset.theme;
  else document.documentElement.dataset.theme = themeBefore;
  await setLocale('en');
});

async function mode(locale: 'en' | 'zh-CN', theme: 'light' | 'dark') {
  await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme });
  document.documentElement.dataset.theme = theme; document.documentElement.classList.toggle('dark', theme === 'dark');
}

function Filters({ create }: { create: (value: { text: string; participating: boolean; sort: string }) => void }) {
  const [text, setText] = useState(''); const [participating, setParticipating] = useState(false); const [sort, setSort] = useState('name');
  return <>
    <PageHeader title={i18n.t('screens.machines.title')} />
    <FilterBar search={<Input type="search" label={i18n.t('everything.search')} value={text} onChange={(event) => setText(event.target.value)} />}
      participation={<ParticipationFilter checked={participating} onCheckedChange={setParticipating} />}
      action={<Button action="create" variant="primary" onClick={() => create({ text, participating, sort })} />}>
      <Select label={i18n.t('everything.sortBy')} value={sort} onValueChange={setSort} options={(['name', 'created'] as const).map((value) => ({ value, label: i18n.t(`inventoryChoices.everything.columns.${value}.name`) }))} />
    </FilterBar>
  </>;
}

it.each(modes)('keeps search, participation, selects and one final action usable in %s/%s', async (locale, theme) => {
  await mode(locale, theme); const create = vi.fn(); const user = userEvent.setup();
  show(<Filters create={create} />);
  expect(screen.getAllByRole('heading', { level: 1 })).toHaveLength(1);
  const bar = screen.getByRole('group', { name: i18n.t('ui.filterBar.label') });
  const search = within(bar).getByRole('searchbox'); const participation = within(bar).getByRole('switch');
  const select = within(bar).getByRole('combobox'); const action = within(bar).getByRole('button', { name: actionName('create') });
  expect(screen.getAllByRole('button', { name: actionName('create') })).toHaveLength(1);
  expect(search.compareDocumentPosition(participation) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  expect(participation.compareDocumentPosition(select) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  expect(select.compareDocumentPosition(action) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  await user.type(search, 'alpha'); await user.click(participation);
  await user.click(select); await user.click(screen.getByRole('option', { name: i18n.t('inventoryChoices.everything.columns.created.name') }));
  await user.click(action);
  expect(create).toHaveBeenCalledTimes(1); expect(create).toHaveBeenCalledWith({ text: 'alpha', participating: true, sort: 'created' });
});

function DateDemo({ write, submit, initial = '2040-02-28', ...props }: Omit<DateFieldProps, 'label' | 'value' | 'onValueChange'> & { write: (value: string) => void; submit?: () => void; initial?: string }) {
  const [value, setValue] = useState(initial);
  return <form onSubmit={(event) => { event.preventDefault(); submit?.(); }}>
    <DateField {...props} label={i18n.t('trace.from')} value={value} onValueChange={(next) => { setValue(next); write(next); }} />
    <Button type="submit" action="save" />
  </form>;
}

it.each(modes)('refuses impossible dates and lets the keyboard choose a bounded leap day in %s/%s', async (locale, theme) => {
  await mode(locale, theme); const write = vi.fn(); const submit = vi.fn(); const user = userEvent.setup();
  show(<DateDemo write={write} submit={submit} min="2040-02-28" max="2040-02-29" />);
  const input = screen.getByRole('textbox', { name: i18n.t('trace.from') });
  expect(input).toHaveAttribute('type', 'text'); expect(input).toHaveValue('2040-02-28');
  fireEvent.change(input, { target: { value: '2040-02-30' } });
  await user.click(screen.getByRole('button', { name: actionName('save') }));
  expect(submit).not.toHaveBeenCalled(); expect(write).not.toHaveBeenCalled();
  expect(input).toHaveAttribute('aria-invalid', 'true'); expect(screen.getByText(i18n.t('ui.date.invalid'))).toBeVisible();
  fireEvent.change(input, { target: { value: '2040-02-28' } });
  await user.click(screen.getByRole('button', { name: i18n.t('ui.date.choose', { field: i18n.t('trace.from') }) }));
  const formatter = new Intl.DateTimeFormat(locale, { dateStyle: 'full', timeZone: 'UTC' });
  const name = (date: string) => formatter.format(new Date(`${date}T00:00:00Z`));
  await waitFor(() => expect(screen.getByRole('button', { name: name('2040-02-28') })).toHaveFocus());
  expect(screen.getByRole('button', { name: name('2040-02-27') })).toBeDisabled();
  await user.keyboard('{ArrowLeft}'); expect(screen.getByRole('button', { name: name('2040-02-28') })).toHaveFocus();
  await user.keyboard('{ArrowRight}');
  await waitFor(() => expect(screen.getByRole('button', { name: name('2040-02-29') })).toHaveFocus());
  await user.keyboard('{Enter}');
  expect(write).toHaveBeenLastCalledWith('2040-02-29'); expect(input).toHaveValue('2040-02-29');
  expect(screen.queryByRole('grid')).not.toBeInTheDocument();
});

it.each(modes)('clears an optional date and keeps local date/time values unchanged on the wire in %s/%s', async (locale, theme) => {
  await mode(locale, theme); const write = vi.fn(); const user = userEvent.setup();
  show(<DateDemo write={write} type="datetime-local" initial="2040-02-28T12:30" min="" max="" />);
  const input = screen.getByRole('textbox', { name: i18n.t('trace.from') });
  expect(input).toHaveValue('2040-02-28 12:30');
  fireEvent.change(input, { target: { value: '2040-02-29 13:45' } });
  expect(write).toHaveBeenLastCalledWith('2040-02-29T13:45');
  await user.click(screen.getByRole('button', { name: i18n.t('ui.date.choose', { field: i18n.t('trace.from') }) }));
  await user.click(screen.getByRole('button', { name: i18n.t('ui.date.clear') }));
  expect(write).toHaveBeenLastCalledWith(''); expect(input).toHaveValue('');
});

it.each(modes)('keeps every section link visible and every status in words exactly once in %s/%s', async (locale, theme) => {
  await mode(locale, theme);
  const pages = ['profile', 'security', 'notifications', 'tokens', 'exports', 'interface'] as const;
  show(<>
    <SectionNav label={i18n.t('settings.personal.nav')} current="security" items={pages.map((id) => ({ id, label: i18n.t(`screens.${id}.title`), link: <a href={`/settings/${id}`} /> }))} />
    <StateChip state="running" /><StatusDot showLabel tone="success" label={i18n.t('state.done')} />
    <Badge tone="warning">{i18n.t('componentUpdates.available')}</Badge><Badge count={7} />
  </>);
  const nav = screen.getByRole('navigation', { name: i18n.t('settings.personal.nav') });
  expect(within(nav).getAllByRole('link')).toHaveLength(pages.length);
  expect(within(nav).getByRole('link', { name: i18n.t('screens.security.title') })).toHaveAttribute('aria-current', 'page');
  for (const id of pages) expect(within(nav).getByRole('link', { name: i18n.t(`screens.${id}.title`) })).toHaveAttribute('href', `/settings/${id}`);
  expect(within(nav).getByRole('combobox', { name: i18n.t('settings.personal.nav') })).toHaveValue(i18n.t('screens.security.title'));
  for (const key of ['state.running', 'state.done', 'componentUpdates.available'] as const) expect(screen.getAllByText(i18n.t(key))).toHaveLength(1);
  expect(screen.getByText('7')).toBeVisible();
});
