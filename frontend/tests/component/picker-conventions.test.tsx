/** Every picker works as a quick pick does: the choices that fit first, the current one marked, one line a row, the
 * keyboard throughout, and the person's recent choices on top where the list remembers them. */
import { cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { QueryClient } from '@tanstack/react-query';
import { useState } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';

import { client } from '@/api/client';
import { pickerRecentsKey } from '@/api/pickerRecents';
import type * as Recents from '@/api/pickerRecents';
import { ChoiceChip, PickList, type PickGroup } from '@/features/composer/PickList';
import type { ResourceGroupMeta } from '@/features/resources/groups';
import { ResourcePickerScope } from '@/features/resources/PickerContext';
import { PlacePicker, type PlaceRow } from '@/features/start/PlacePicker';
import type { MePickerRecents, NodeRef } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { Combobox, type ComboboxOption } from '@/ui/Combobox';
import type { CardPreview } from '@/ui/DetailCard';
import { ResourcePicker } from '@/ui/ResourcePicker';
import { show, skipOverflowFallback } from './support';

const recentServer = vi.hoisted(() => ({ read: vi.fn<(person: string) => Promise<MePickerRecents>>(), record: vi.fn<typeof Recents.recordPickerChoice>() }));
vi.mock('@/api/pickerRecents', async (load) => {
  const actual = await load<typeof Recents>();
  return { ...actual, pickerRecentsQuery: (person: string) => ({ ...actual.pickerRecentsQuery(person), queryFn: () => recentServer.read(person) }), recordPickerChoice: recentServer.record };
});
vi.mock('@/features/navigation/places', async (load) => ({ ...await load<object>(), PlaceLink: ({ place, children }: { place: NodeRef; children: React.ReactNode }) => <a href={`#${place.id}`}>{children}</a> }));
skipOverflowFallback();

const session = client.session.getState();
beforeEach(() => {
  recentServer.read.mockReset().mockResolvedValue({ count: 5, choices: {} });
  recentServer.record.mockReset().mockResolvedValue({ count: 5, remembered: true });
});
afterEach(async () => { cleanup(); localStorage.clear(); client.session.setState(session); await setLocale('en'); });

const hosts = (count: number): ComboboxOption<string>[] => Array.from({ length: count }, (_, at) => ({ value: `host-${at}`, label: `Host ${at}`, text: `Host ${at}`, description: `machine ${at}` }));

/** A complete-choice field as the forms use it: the committed value starts the list and names the field. */
function HostField({ options, initial, onChoose = () => undefined }: { options: ComboboxOption<string>[]; initial: string; onChoose?: (value: string) => void }) {
  const [value, setValue] = useState(initial), [query, setQuery] = useState('');
  const chosen = options.find((option) => option.value === value);
  return <Combobox label="Host" query={query} onQueryChange={setQuery} displayValue={chosen?.text} initialActiveValue={value}
    groups={[{ id: 'hosts', label: '', options: options.filter((option) => option.text.toLowerCase().includes(query.toLowerCase())) }]}
    onSelect={(option) => { setValue(option.value); setQuery(''); onChoose(option.value); }} />;
}

const activeName = (input: HTMLElement) => document.getElementById(input.getAttribute('aria-activedescendant') ?? '')?.querySelector('[data-choice-name]')?.textContent;

for (const locale of ['en', 'zh-CN'] as const) it(`marks the current choice, keeps it named in the open field and puts a row's fact on its line in ${locale}`, async () => {
  await setLocale(locale);
  const user = userEvent.setup(), chosen = vi.fn();
  const options = [...hosts(3), { value: 'closed', label: 'Closed host', text: 'Closed host', disabled: 'Needs a running machine' }];
  show(<HostField options={options} initial="host-1" onChoose={chosen} />);
  const input = screen.getByRole('combobox', { name: 'Host' });
  expect(input).toHaveValue('Host 1');
  await user.click(input);
  // The field never reads blank while its list is open: the current choice stands as its placeholder.
  expect(input).toHaveValue('');
  expect(input).toHaveAttribute('placeholder', 'Host 1');
  const current = screen.getByRole('option', { name: 'Host 1' });
  expect(current).toHaveAttribute('aria-current', 'true');
  expect(current.querySelector('svg')).not.toBeNull();
  expect(screen.getAllByRole('option').filter((row) => row.getAttribute('aria-current') === 'true')).toEqual([current]);
  expect(activeName(input)).toBe('Host 1');
  // A row that can be chosen says its one fact beside its name; one that cannot keeps its reason under it.
  const inline = current.querySelector('[data-choice-inline]');
  expect(inline).not.toBeNull();
  expect(inline).toHaveTextContent('machine 1');
  const closed = screen.getByRole('option', { name: 'Closed host' });
  expect(closed.querySelector('[data-choice-inline]')).toBeNull();
  expect(closed).toHaveTextContent('Needs a running machine');
  await user.keyboard('{ArrowDown}{Enter}');
  expect(chosen).toHaveBeenCalledExactlyOnceWith('host-2');
  await user.click(input);
  expect(screen.getByRole('option', { name: 'Host 2' })).toHaveAttribute('aria-current', 'true');
  expect(screen.getByRole('option', { name: 'Host 1' })).not.toHaveAttribute('aria-current');
  await user.keyboard('{Escape}');
  expect(input).toHaveFocus();
  expect(input).toHaveValue('Host 2');
});

it('moves a page at a time with Page Down and Page Up, and to the ends with Home and End', async () => {
  const user = userEvent.setup();
  show(<HostField options={hosts(25)} initial="host-0" />);
  const input = screen.getByRole('combobox', { name: 'Host' });
  await user.click(input);
  expect(activeName(input)).toBe('Host 0');
  await user.keyboard('{PageDown}');
  expect(activeName(input)).toBe('Host 10');
  await user.keyboard('{PageDown}{PageDown}');
  expect(activeName(input)).toBe('Host 24');
  await user.keyboard('{PageUp}');
  expect(activeName(input)).toBe('Host 14');
  await user.keyboard('{Home}');
  expect(activeName(input)).toBe('Host 0');
  await user.keyboard('{End}');
  expect(activeName(input)).toBe('Host 24');
  // Typing narrows the list at once and starts on the first match.
  await user.keyboard('2');
  expect(screen.getAllByRole('option').map((row) => row.querySelector('[data-choice-name]')?.textContent)).toEqual(['Host 2', 'Host 12', 'Host 20', 'Host 21', 'Host 22', 'Host 23', 'Host 24']);
  expect(activeName(input)).toBe('Host 2');
});

it('opens an inspection card only when the person moves through the list, not by itself on opening', async () => {
  const user = userEvent.setup();
  const preview = (name: string): CardPreview => ({ label: name, content: () => <p>{`${name} details`}</p> });
  const options: ComboboxOption<string>[] = ['Alpha template', 'Beta template'].map((name) => ({ value: name, label: name, text: name, preview: preview(name) }));
  show(<HostField options={options} initial="Alpha template" />);
  await user.click(screen.getByRole('combobox', { name: 'Host' }));
  expect(screen.getByRole('option', { name: 'Alpha template' })).toBeVisible();
  expect(screen.queryByText('Alpha template details')).toBeNull();
  await user.keyboard('{ArrowDown}');
  expect(await screen.findByText('Beta template details')).toBeVisible();
});

const aaa: NodeRef = { id: 'aaa-home', name: 'Aaa home', kind: 'project', redacted: false };
const zed: NodeRef = { id: 'zed-home', name: 'Zed home', kind: 'project', redacted: false };
for (const locale of ['en', 'zh-CN'] as const) it(`offers the resources that can be chosen first, in their homes and across them, and finds a row by its fact in ${locale}`, async () => {
  await setLocale(locale);
  client.session.setState({ userId: 'fitting-reader' });
  const user = userEvent.setup(), chosen = vi.fn();
  show(<ResourcePickerScope screen="fitting" entries={new Map<string, ResourceGroupMeta>([
    ['template:cannot', { home: aaa, home_path: [aaa] }], ['template:blocked', { home: zed, home_path: [zed] }], ['template:usable', { home: zed, home_path: [zed] }]])}>
    <ResourcePicker pickerKey="fitting" label="Program" value="" onValueChange={chosen} options={[
      { value: 'cannot', name: 'Cannot use', disabled: true, description: 'Its machine keeps no sign-in', resource: { kind: 'template', id: 'cannot' } },
      { value: 'blocked', name: 'Blocked one', disabled: true, description: 'Support is unknown', resource: { kind: 'template', id: 'blocked' } },
      { value: 'usable', name: 'Usable one', description: 'walk-lab', resource: { kind: 'template', id: 'usable' } },
    ]} />
  </ResourcePickerScope>);
  const input = screen.getByRole('combobox', { name: 'Program' });
  await user.click(input);
  await screen.findByRole('option', { name: 'Usable one' });
  expect(screen.getAllByRole('option').map((row) => row.querySelector('[data-choice-name]')?.textContent)).toEqual(['Usable one', 'Blocked one', 'Cannot use']);
  expect(screen.getAllByRole('group').map((group) => group.getAttribute('aria-label'))).toEqual(['Zed home', 'Aaa home']);
  await user.type(input, 'walk');
  expect(screen.getAllByRole('option').map((row) => row.querySelector('[data-choice-name]')?.textContent)).toEqual(['Usable one']);
  await user.keyboard('{Enter}');
  expect(chosen).toHaveBeenCalledExactlyOnceWith('usable');
});

const agents: PickGroup[] = [{ id: 'fits', label: null, rows: [
  { id: 'antigravity', name: 'Antigravity', disabled: true, detail: 'not reported yet' },
  { id: 'claude', name: 'Claude Code', checked: true },
  { id: 'luna', name: 'gpt-6-luna', place: 'OpenAI API' },
  { id: 'codex', name: 'Codex', detail: 'Default' },
] }];
for (const locale of ['en', 'zh-CN'] as const) it(`lists the rows that can be chosen first, each on one line with its fact, in ${locale}`, async () => {
  await setLocale(locale);
  const chosen = vi.fn();
  show(<PickList label="Agent" groups={agents} onChoose={chosen} />);
  const menu = screen.getByRole('menu', { name: 'Agent' });
  const rows = within(menu).getAllByRole('menuitemradio');
  expect(rows.map((row) => row.querySelector('[data-choice-name]')?.textContent)).toEqual(['Claude Code', 'gpt-6-luna', 'Codex', 'Antigravity']);
  const luna = within(menu).getByRole('menuitemradio', { name: 'gpt-6-luna' });
  expect(luna).toHaveTextContent('OpenAI API');
  expect(luna).not.toHaveTextContent(`${i18n.t('startFlow.separator')}OpenAI API`);
  expect(luna.querySelector('[data-choice-inline]')).toHaveTextContent('OpenAI API');
  expect(within(menu).getByRole('menuitemradio', { name: 'Codex' }).querySelector('[data-choice-inline]')).toHaveTextContent('Default');
  const off = within(menu).getByRole('menuitemradio', { name: 'Antigravity' });
  expect(off).toBeDisabled();
  expect(off.querySelector('[data-choice-inline]')).toBeNull();
  expect(off).toHaveTextContent('not reported yet');
});

it('opens a chip list below its control where the screen has room for it', async () => {
  const user = userEvent.setup();
  show(<ChoiceChip what="Agent" value="Claude Code" groups={agents} onChoose={vi.fn()} />);
  await user.click(screen.getByRole('button', { name: i18n.t('composer.chip.value', { what: 'Agent', value: 'Claude Code' }) }));
  const menu = await screen.findByRole('menu', { name: 'Agent' });
  expect(menu.closest('[data-side]')).toHaveAttribute('data-side', 'bottom');
});

const place = (id: string, name: string, more: Partial<PlaceRow> = {}): PlaceRow => ({ id, name, on: `${name} machine`, state: null, group: 'ready', reason: null, status: 'running', ...more });
for (const locale of ['en', 'zh-CN'] as const) it(`puts the person's recent environments first, marks the current one and keeps each row to one line in ${locale}`, async () => {
  await setLocale(locale);
  client.session.setState({ userId: 'place-reader' });
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity } } });
  const reply = { count: 5, choices: { environment: ['beta', 'closed'] } };
  recentServer.read.mockResolvedValue(reply); cache.setQueryData(pickerRecentsKey('place-reader'), reply);
  const user = userEvent.setup(), picked = vi.fn(), fresh = vi.fn();
  function Where() {
    const [value, setValue] = useState('alpha');
    return <PlacePicker label="Where it runs" hideLabel={false} value={value} newRow={{ onNew: fresh }} onPick={(id) => { setValue(id); picked(id); }} rows={[
      place('alpha', 'Alpha env', { isDefault: true }),
      place('beta', 'Beta env'),
      place('closed', 'Closed env', { group: 'unavailable', reason: 'This environment is not running', status: 'stopped' }),
    ]} />;
  }
  show(<Where />, cache);
  const input = screen.getByRole('combobox', { name: 'Where it runs' });
  await user.click(input);
  const recent = await screen.findByRole('group', { name: i18n.t('ui.picker.recent') });
  expect(within(recent).getAllByRole('option').map((row) => row.querySelector('[data-choice-name]')?.textContent)).toEqual(['Beta env']);
  expect(screen.getAllByRole('option').map((row) => row.querySelector('[data-choice-name]')?.textContent)).toEqual([i18n.t('start.where.newRow'), 'Beta env', 'Alpha env', 'Closed env']);
  expect(input).toHaveAttribute('placeholder', 'Alpha env');
  const alpha = screen.getByRole('option', { name: 'Alpha env' });
  expect(alpha).toHaveAttribute('aria-current', 'true');
  const facts = alpha.querySelector('[data-choice-inline]');
  expect(facts).toHaveTextContent('Alpha env machine');
  expect(facts).toHaveTextContent(i18n.t('start.where.default'));
  expect(screen.getByRole('option', { name: 'Closed env' })).toHaveTextContent('This environment is not running');
  await user.click(within(recent).getByRole('option', { name: 'Beta env' }));
  expect(picked).toHaveBeenCalledExactlyOnceWith('beta');
  await waitFor(() => expect(recentServer.record).toHaveBeenCalledWith({ kind: 'environment', key: 'beta' }));
  await user.click(input);
  await user.click(await screen.findByRole('option', { name: i18n.t('start.where.newRow') }));
  expect(fresh).toHaveBeenCalledTimes(1);
  // Making a new environment is not a recent choice of one.
  expect(recentServer.record).toHaveBeenCalledTimes(1);
});
