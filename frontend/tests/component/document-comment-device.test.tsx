/** The real device controls retire exactly the records they successfully erase, including mounted writers. */
import { act, cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { QueryClient } from '@tanstack/react-query';
import { Activity, type ReactNode } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Auth from '@/api/auth';
import type * as Personal from '@/api/personal';
import { client } from '@/api/client';
import { meStateKey } from '@/api/personal';
import { getCommentDrafts, forgetCommentDraftDeviceData, retireCommentDraftSession, setCommentText, commentSlotKey } from '@/features/documents/commentDrafts';
import { InterfaceSettings } from '@/features/settings/personal/Interface';
import { i18n, setLocale } from '@/i18n';
import { forgetViewChoiceDeviceData, readViewChoice, useRememberedView, useViewChoice, writeViewChoice } from '@/lib/viewChoices';
import { DeviceChoiceFacts } from '@/ui/DeviceChoiceFacts';
import { Select } from '@/ui/Select';
import { Tabs } from '@/ui/Tabs';
import { SegmentedControl } from '@/ui/SegmentedControl';
import { Switch } from '@/ui/Switch';
import { Menu } from '@/ui/Menu';
import { useListChoice } from '@/lib/listState';
import { me } from '../doubles/fixtures';
import { show } from './support';

vi.mock('@/features/settings/PersonalFrame', () => ({ PersonalFrame: ({ children }: { children: ReactNode }) => <main>{children}</main> }));
vi.mock('@/features/settings/personal/Profile', () => ({ LookRows: () => null }));
const writes = vi.hoisted(() => vi.fn<(key: string, value: unknown) => Promise<void>>());
vi.mock('@/api/personal', async (load) => ({ ...await load<typeof Personal>(), setMeState: writes, meStateQuery: { queryKey: ['device-personal'], queryFn: () => Promise.resolve({}) } }));
vi.mock('@/api/auth', async (load) => ({ ...await load<typeof Auth>(), meQuery: { queryKey: ['device-me'], queryFn: () => Promise.resolve(me(1, 'ada', 'Ada')) } }));
const session = client.session.getState(), person = 'device-comment-person';
const target = { kind: 'reply' as const, discussion: 'original-discussion' };
beforeEach(() => { writes.mockReset().mockResolvedValue(undefined); localStorage.clear(); client.session.setState({ status: 'signed_in', userId: person, readerVersion: (session.readerVersion ?? 0) + 1 }); });
afterEach(async () => { cleanup(); vi.restoreAllMocks(); forgetCommentDraftDeviceData(person, null); forgetViewChoiceDeviceData(person, null); retireCommentDraftSession(); localStorage.clear(); client.session.setState(session); await setLocale('en'); });

function OpenPageChoice() {
  const [choice] = useViewChoice('page', 'state', 'Default filters', 'screen-0');
  return <output aria-label="Mounted page filters">{choice}</output>;
}

function ObservedChoice({ explicit, offered = ['overview', 'threads'] }: { explicit?: string; offered?: readonly string[] }) {
  const [value, , observation] = useRememberedView('topic', 'tab', explicit, 'overview', offered);
  const name = (value: unknown) => value === 'overview' ? 'Overview' : value === 'threads' ? 'Threads' : offered.includes(String(value)) ? 'Restored view' : undefined;
  return <><output aria-label="Current view">{name(value)}</output><DeviceChoiceFacts observation={observation} current={name(value)} saved={name(observation?.saved)} /></>;
}

it.each(['remembered', 'list'] as const)('shows %s choice facts inside the existing picker without another storage read or choice', async (source) => {
  const kind = source === 'remembered' ? 'topic' : 'page.metadata-list.metadata-list', key = source === 'remembered' ? 'tab' : 'view';
  writeViewChoice(person, kind, key, source === 'remembered' ? 'threads' : JSON.stringify('threads'));
  const stored = `orch.${person}.choices.${kind}.${key}`;
  const get = localStorage.getItem.bind(localStorage), set = localStorage.setItem.bind(localStorage);
  const reads: string[] = [], writes: string[] = [];
  vi.spyOn(Storage.prototype, 'getItem').mockImplementation((key) => { if (key === stored) reads.push(key); return get(key); });
  vi.spyOn(Storage.prototype, 'setItem').mockImplementation((key, value) => { if (key === stored) writes.push(key); set(key, value); });
  function Remembered() { const [value, choose] = useRememberedView('topic', 'tab', undefined, 'overview', ['overview', 'threads']); return <Select label="View" value={value} onValueChange={choose} options={[{ value: 'overview', name: 'Overview' }, { value: 'threads', name: 'Threads' }]} />; }
  function List() { const [value, choose] = useListChoice('metadata-list', 'view', 'overview'); return <Select label="View" value={value} onValueChange={choose} options={[{ value: 'overview', name: 'Overview' }, { value: 'threads', name: 'Threads' }]} />; }
  show(source === 'remembered' ? <Remembered /> : <List />);
  const before = reads.length;
  await userEvent.click(screen.getByRole('combobox', { name: 'View' }));
  const body = document.querySelector<HTMLElement>('[data-fact-disclosure-body]');
  if (body === null) throw new Error('The existing picker must own its current device facts');
  expect(screen.getByRole('dialog', { name: 'View' })).toContainElement(body);
  expect(body).toHaveTextContent('Threads'); expect(body).toHaveTextContent(source === 'remembered' ? 'This kind' : 'In this page');
  expect(reads).toHaveLength(before); expect(writes).toHaveLength(0);
  expect(within(screen.getByRole('listbox')).queryByRole('button', { name: i18n.t('common.details') })).not.toBeInTheDocument();
});

it.each(['tabs', 'segments', 'switch', 'menu'] as const)('keeps a failed device save beside its actual %s choice and retires it with the reader', async (control) => {
  const original = localStorage.setItem.bind(localStorage);
  vi.spyOn(Storage.prototype, 'setItem').mockImplementation((key, value) => {
    if (key === `orch.${person}.choices.topic.tab`) throw new DOMException('Synthetic refusal', 'QuotaExceededError');
    original(key, value);
  });
  function Choice() {
    const [value, choose] = useViewChoice<string>('topic', 'tab', 'overview');
    const [on, toggle] = useViewChoice<boolean>('topic', 'enabled', false);
    if (control === 'tabs') return <Tabs label="View" value={value} onValueChange={choose} tabs={[{ value: 'overview', label: 'Overview', panel: null }, { value: 'threads', label: 'Threads', panel: null }]} />;
    if (control === 'segments') return <SegmentedControl label="View" value={value} onValueChange={choose} segments={[{ value: 'overview', name: 'Overview' }, { value: 'threads', name: 'Threads' }]} />;
    if (control === 'switch') return <Switch label="Remember this view" checked={on} onCheckedChange={toggle} />;
    return <Menu label="View" trigger={<button>View choices</button>} entries={[{ type: 'radio', id: 'view', label: 'View', value, onValueChange: choose, options: [{ value: 'overview', name: 'Overview' }, { value: 'threads', name: 'Threads' }] }]} />;
  }
  show(<Choice />);
  if (control === 'switch') {
    await userEvent.click(screen.getByRole('switch'));
    await userEvent.click(screen.getByRole('button', { name: i18n.t('common.details') }));
    expect(document.querySelector('[data-fact-disclosure-body]')).toHaveTextContent('On');
  } else {
    if (control === 'menu') await userEvent.click(screen.getByRole('button', { name: 'View choices' }));
    await userEvent.click(screen.getByRole(control === 'tabs' ? 'tab' : control === 'menu' ? 'menuitemradio' : 'radio', { name: 'Threads' }));
    expect(await screen.findByText('Not saved on this device')).toBeVisible();
    if (control === 'menu') await userEvent.click(screen.getByRole('button', { name: 'View choices' }));
    else await userEvent.click(screen.getByRole('button', { name: i18n.t('common.details') }));
    expect(document.querySelector('[data-fact-disclosure-body]')).toHaveTextContent('Threads');
  }
  act(() => client.session.setState({ userId: 'replacement-picker-reader', readerVersion: (client.session.getState().readerVersion ?? 0) + 1 }));
  expect(screen.queryByText('Not saved on this device')).not.toBeInTheDocument();
});

it('preserves an unavailable saved view through an explicit address and restores it when offered again', async () => {
  const key = `orch.${person}.choices.topic.tab`, raw = JSON.stringify('temporarily-unavailable'); localStorage.setItem(key, raw);
  const view = show(<ObservedChoice explicit="threads" />);
  expect(screen.getByLabelText('Current view')).toHaveTextContent('Threads');
  await userEvent.click(screen.getByRole('button', { name: i18n.t('common.details') }));
  const body = document.querySelector('[data-fact-disclosure-body]');
  expect(body).toHaveTextContent('Address'); expect(body).not.toHaveTextContent('temporarily-unavailable');
  expect(localStorage.getItem(key)).toBe(raw);
  view.rerender(<ObservedChoice />);
  expect(screen.getByLabelText('Current view')).toHaveTextContent('Overview');
  expect(screen.getByText('Using default · Saved choice unavailable')).toBeVisible(); expect(localStorage.getItem(key)).toBe(raw);
  view.rerender(<ObservedChoice offered={['overview', 'threads', 'temporarily-unavailable']} />);
  expect(screen.getByLabelText('Current view')).toHaveTextContent('Restored view'); expect(localStorage.getItem(key)).toBe(raw);
});

it.each(['en', 'zh-CN'] as const)('keeps malformed saved choice bytes distinct from a device-storage read failure in %s', async (locale) => {
  await setLocale(locale); const key = `orch.${person}.choices.topic.tab`, raw = 'private-malformed-json'; localStorage.setItem(key, raw);
  show(<ObservedChoice />);
  expect(screen.getByLabelText('Current view')).toHaveTextContent('Overview');
  expect(await screen.findByText(locale === 'en' ? 'Using default · Saved choice unavailable' : '使用默认值 · 已保存的选择不可用')).toBeVisible();
  expect(screen.queryByText(locale === 'en' ? 'Unable to read device storage' : '无法读取设备存储')).not.toBeInTheDocument();
  expect(localStorage.getItem(key)).toBe(raw); expect(document.body).not.toHaveTextContent(raw);
});

it.each(['en', 'zh-CN'] as const)('keeps a failed disappeared choice factual on this device without inventing stored bytes in %s', async (locale) => {
  await setLocale(locale);
  const original = localStorage.setItem.bind(localStorage);
  vi.spyOn(Storage.prototype, 'setItem').mockImplementation(function (this: Storage, key, value) {
    if (key.startsWith(`orch.${person}.choices.`)) throw new DOMException('Synthetic device refusal', 'QuotaExceededError');
    original(key, value);
  });
  function Choice() {
    const [value, choose] = useViewChoice<string>('page', 'state', 'Default filters', 'opaque-disappeared-control');
    return <button onClick={() => choose('Private unsaved filters')}>{value}</button>;
  }
  const view = show(<><Choice /><InterfaceSettings /></>);
  await userEvent.click(screen.getByRole('button', { name: 'Default filters' }));
  expect(screen.getByRole('button', { name: 'Private unsaved filters' })).toBeVisible();
  view.rerender(<InterfaceSettings />);
  const failure = await screen.findByText(locale === 'en' ? 'Not saved on this device' : '未保存到此设备');
  const row = failure.closest('li'); if (row === null) throw new Error('A disappeared choice keeps its safe kind in the existing device list');
  expect(row).toHaveTextContent(i18n.t('craftChoiceNames.kinds.page'));
  expect(row).not.toHaveTextContent(/Private unsaved filters|opaque-disappeared-control|0 B|0 字节/);
  await userEvent.click(within(row).getByRole('button', { name: i18n.t('common.details') }));
  const body = document.querySelector<HTMLElement>('[data-fact-disclosure-body]');
  expect(body).toHaveTextContent(locale === 'en' ? 'Not saved on this device' : '未保存到此设备');
  expect(body).not.toHaveTextContent(/Private unsaved filters|opaque-disappeared-control/);
  act(() => client.session.setState({ userId: 'replacement-device-reader', readerVersion: (client.session.getState().readerVersion ?? 0) + 1 }));
  expect(screen.queryByText(locale === 'en' ? 'Not saved on this device' : '未保存到此设备')).not.toBeInTheDocument();
});

it.each(['en', 'zh-CN'] as const)('keeps an unavailable device inventory distinct from an empty device in %s', async (locale) => {
  await setLocale(locale);
  vi.spyOn(Storage.prototype, 'length', 'get').mockImplementation(() => { throw new DOMException('Synthetic read refusal', 'SecurityError'); });
  show(<InterfaceSettings />);
  expect(await screen.findByText(locale === 'en' ? 'Unable to read device storage' : '无法读取设备存储')).toBeVisible();
  expect(screen.queryByText(i18n.t('settings.interface.device.none'))).not.toBeInTheDocument();
});

it('keeps every live choice after a refused large save and makes a retired reader’s writer inert', async () => {
  const large = 'x'.repeat(3 * 1024 * 1024);
  const writes = vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => { throw new DOMException('Synthetic quota refusal', 'QuotaExceededError'); });
  let retired: ((value: string) => void) | undefined;
  function Choices() {
    const [one, chooseOne] = useViewChoice<string>('page', 'state', '', 'large-one');
    const [two, chooseTwo] = useViewChoice<string>('page', 'state', '', 'large-two');
    retired = chooseOne;
    return <><button onClick={() => chooseOne(large)}>First choice</button><button onClick={() => chooseTwo(large)}>Second choice</button><output aria-label="First length">{one.length}</output><output aria-label="Second length">{two.length}</output></>;
  }
  const view = show(<Choices />);
  await userEvent.click(screen.getByRole('button', { name: 'First choice' }));
  await userEvent.click(screen.getByRole('button', { name: 'Second choice' }));
  expect(screen.getByLabelText('First length')).toHaveTextContent(String(large.length));
  expect(screen.getByLabelText('Second length')).toHaveTextContent(String(large.length));
  const oldWriter = retired; view.unmount();
  act(() => client.session.setState({ userId: 'replacement-large-choice', readerVersion: (client.session.getState().readerVersion ?? 0) + 1 }));
  const attempted = writes.mock.calls.length;
  oldWriter?.('Retired private value');
  expect(writes).toHaveBeenCalledTimes(attempted);
});

it('keeps a hidden tab’s refused choice in its owned state without another storage read on return', async () => {
  const large = 'x'.repeat(3 * 1024 * 1024), get = localStorage.getItem.bind(localStorage);
  vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => { throw new DOMException('Synthetic quota refusal', 'QuotaExceededError'); });
  const reads = vi.spyOn(Storage.prototype, 'getItem').mockImplementation((key) => get(key));
  function One() { const [value, choose] = useViewChoice<string>('page', 'state', '', 'hidden-choice'); return <><button onClick={() => choose(large)}>Choose before hiding</button><output aria-label="Kept length">{value.length}</output></>; }
  function Two() { const [, choose] = useViewChoice<string>('page', 'state', '', 'other-choice'); return <button onClick={() => choose(large)}>Choose in other tab</button>; }
  function Tabs({ hidden }: { hidden: boolean }) { return <><Activity mode={hidden ? 'hidden' : 'visible'}><One /></Activity><Two /></>; }
  const view = show(<Tabs hidden={false} />);
  await userEvent.click(screen.getByRole('button', { name: 'Choose before hiding' }));
  view.rerender(<Tabs hidden />);
  await userEvent.click(screen.getByRole('button', { name: 'Choose in other tab' }));
  const before = reads.mock.calls.length;
  view.rerender(<Tabs hidden={false} />);
  expect(screen.getByLabelText('Kept length')).toHaveTextContent(String(large.length)); expect(reads).toHaveBeenCalledTimes(before);
});

it('keeps an unsigned choice usable in its own view without saving another person’s data', async () => {
  client.session.setState({ status: 'signed_out', userId: null, readerVersion: (client.session.getState().readerVersion ?? 0) + 1 });
  const saved = vi.spyOn(Storage.prototype, 'setItem');
  function Choice() { const [value, choose] = useViewChoice<string>('help', 'part', 'articles'); return <button onClick={() => choose('settings')}>{value}</button>; }
  show(<Choice />); await userEvent.click(screen.getByRole('button', { name: 'articles' }));
  expect(screen.getByRole('button', { name: 'settings' })).toBeVisible(); expect(saved).not.toHaveBeenCalled();
});

it('clears a hidden item’s inherited choice while preserving an independent item override', () => {
  writeViewChoice(person, 'topic', 'tab', 'threads'); writeViewChoice(person, 'topic', 'tab', 'documents', 'independent-item');
  function Choice({ item }: { item: string }) { const [value] = useViewChoice<string>('topic', 'tab', 'overview', item); return <output aria-label={item}>{value}</output>; }
  function Tabs({ hidden }: { hidden: boolean }) { return <><Activity mode={hidden ? 'hidden' : 'visible'}><Choice item="inherited-item" /></Activity><Choice item="independent-item" /></>; }
  const view = show(<Tabs hidden={false} />); expect(screen.getByLabelText('inherited-item')).toHaveTextContent('threads');
  view.rerender(<Tabs hidden />); const key = `orch.${person}.choices.topic.tab`;
  act(() => { localStorage.removeItem(key); forgetViewChoiceDeviceData(person, [key]); });
  view.rerender(<Tabs hidden={false} />);
  expect(screen.getByLabelText('inherited-item')).toHaveTextContent('overview'); expect(screen.getByLabelText('independent-item')).toHaveTextContent('documents');
});

it('names personal and provider usage choices once and clears both through their familiar device group', async () => {
  writeViewChoice(person, 'usage', 'tab', 'usage'); writeViewChoice(person, 'providerUsage', 'tab', 'performance');
  show(<InterfaceSettings />);
  const label = i18n.t('craftChoiceNames.scope', { kind: i18n.t('craftChoiceNames.kinds.usage'), choice: i18n.t('craftChoiceNames.keys.tab') });
  const name = await screen.findByText(label);
  expect(screen.getAllByText(label)).toHaveLength(1);
  expect(screen.queryByText(i18n.t('craftChoiceNames.unknown'))).not.toBeInTheDocument();
  const row = name.closest('li'); if (row === null) throw new Error('Usage choices share their own existing device row');
  await userEvent.click(within(row).getByRole('button'));
  expect(readViewChoice(person, 'usage', 'tab', '')).toBe(''); expect(readViewChoice(person, 'providerUsage', 'tab', '')).toBe('');
});

it('saves reading choices under their own native key and preserves other interface and layout choices', async () => {
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const named = { review: { title: 'Review' } };
  cache.setQueryData(meStateKey, { layouts: { named }, interface: { otherChoice: 'kept' } });
  const view = show(<InterfaceSettings />, cache);
  try {
    const recent = await screen.findByLabelText(i18n.t('settings.interface.reading.recent'));
    const delay = screen.getByLabelText(i18n.t('settings.interface.reading.delay'));
    await userEvent.clear(recent); await userEvent.type(recent, '27');
    await userEvent.clear(delay); await userEvent.type(delay, '333');
    const section = screen.getByRole('heading', { name: i18n.t('settings.interface.reading.title') }).closest('section');
    if (section === null) throw new Error('Reading choices need their own section.');
    await userEvent.click(within(section).getByRole('button', { name: i18n.t('common.save') }));
    await waitFor(() => expect(writes).toHaveBeenCalledWith('interface', { otherChoice: 'kept', kept_closed_tabs: 27, progress_delay_ms: 333 }));
    expect(cache.getQueryData(meStateKey)).toMatchObject({ layouts: { named }, interface: { otherChoice: 'kept', kept_closed_tabs: 27, progress_delay_ms: 333 } });
  } finally { view.unmount(); cache.clear(); }
});

it.each([['en', 'List filters and tabs'], ['zh-CN', '列表筛选和标签']] as const)('names one page-choice group and clears its real mounted values without touching another kind or person in %s', async (locale, label) => {
  await setLocale(locale);
  for (let at = 0; at < 40; at += 1) writeViewChoice(person, 'page', 'state', 'Saved filters', `screen-${at}`);
  writeViewChoice(person, 'resource-environments', 'listOrigin', 'Environment filters');
  writeViewChoice('another-person', 'page', 'state', 'Private filters', 'screen-0');
  show(<><OpenPageChoice /><InterfaceSettings /></>);
  const name = await screen.findByText(label);
  expect(screen.getAllByText(label)).toHaveLength(1);
  expect(screen.queryByText(i18n.t('craftChoiceNames.unknown'))).not.toBeInTheDocument();
  const row = name.closest('li'); if (row === null) throw new Error('The grouped choices need one ordinary device row');
  expect(within(row).getAllByRole('button')).toHaveLength(1);
  expect(row).not.toHaveTextContent(/screen-\d|Private filters/);
  expect(screen.getByLabelText('Mounted page filters')).toHaveTextContent('Saved filters');
  const environmentName = i18n.t('craftChoiceNames.scope', { kind: i18n.t('craftChoiceNames.kinds.resource-environments'), choice: i18n.t('craftChoiceNames.keys.listOrigin') });
  expect(screen.getByText(environmentName)).toBeVisible();
  await userEvent.click(within(row).getByRole('button'));
  expect(screen.getByLabelText('Mounted page filters')).toHaveTextContent('Default filters');
  for (let at = 0; at < 40; at += 1) expect(readViewChoice(person, 'page', 'state', 'Default filters', `screen-${at}`)).toBe('Default filters');
  expect(readViewChoice(person, 'resource-environments', 'listOrigin', '')).toBe('Environment filters');
  expect(readViewChoice('another-person', 'page', 'state', '', 'screen-0')).toBe('Private filters');
  expect(screen.queryByText(label)).not.toBeInTheDocument(); expect(screen.getByText(environmentName)).toBeVisible();
});

it('labels a mounted draft with human words and clearing it cannot be undone by that writer', async () => {
  const draft = getCommentDrafts(person, 'opaque-view', 'opaque-document'); setCommentText(draft, target, 'Forget these words');
  const user = userEvent.setup(); show(<InterfaceSettings />);
  const name = await screen.findByText(i18n.t('settings.interface.device.commentDrafts'));
  const row = name.closest('li'); if (row === null) throw new Error('The device setting must use its ordinary row');
  expect(row).not.toHaveTextContent('opaque-view'); expect(row).not.toHaveTextContent('opaque-document');
  await user.click(within(row).getByRole('button'));
  expect(localStorage.getItem(draft.key)).toBeNull(); expect(draft.store.getState().data.slots).toEqual({});
  expect(screen.queryByText(i18n.t('settings.interface.device.commentDrafts'))).not.toBeInTheDocument();
});

it('a partly refused Forget all clears successful writers and preserves the failed record and other person', async () => {
  const a = getCommentDrafts(person, 'view', 'a'), b = getCommentDrafts(person, 'view', 'b');
  setCommentText(a, target, 'First retained draft'); setCommentText(b, target, 'Second retained draft');
  const foreign = 'orch.another-person.unsaved.document-comments.private'; localStorage.setItem(foreign, 'Other person’s draft');
  const native = localStorage.removeItem.bind(localStorage);
  const failing = vi.spyOn(Storage.prototype, 'removeItem').mockImplementation(function (this: Storage, key: string) {
    if (key === b.key) throw new DOMException('refused', 'SecurityError'); native(key);
  });
  const user = userEvent.setup(); show(<InterfaceSettings />);
  await user.click(await screen.findByRole('button', { name: i18n.t('settings.interface.device.forgetAll') }));
  const decision = screen.getByRole('alertdialog', { name: i18n.t('settings.interface.device.forgetAllTitle') });
  await user.click(within(decision).getByRole('button', { name: i18n.t('settings.interface.device.forgetAllConfirm') }));
  expect(localStorage.getItem(a.key)).toBeNull(); expect(a.store.getState().data.slots).toEqual({});
  expect(localStorage.getItem(b.key)).not.toBeNull();
  expect(b.store.getState().data.slots[commentSlotKey(target)]?.text).toBe('Second retained draft');
  expect(localStorage.getItem(foreign)).toBe('Other person’s draft'); expect(decision).toBeInTheDocument();
  failing.mockRestore();
  await user.click(within(decision).getByRole('button', { name: i18n.t('settings.interface.device.forgetAllConfirm') }));
  await waitFor(() => expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument());
  expect(b.store.getState().data.slots).toEqual({}); expect(localStorage.getItem(foreign)).toBe('Other person’s draft');
  act(() => forgetCommentDraftDeviceData(person, null));
});
