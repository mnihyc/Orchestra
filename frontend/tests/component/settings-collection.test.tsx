/** Long settings pages keep readable values, one authority sentence and keyboard-reachable group contents. */
import { act, fireEvent, screen, waitFor, within } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as SettingsApi from '@/api/settings';
import { putSetting, targetSettingsKey } from '@/api/settings';
import { SettingsEditor } from '@/features/settings/Editor';
import { GROUP_ORDER } from '@/features/settings/settingsModel';
import type { ResolvedSetting, SettingDef } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { choiceCopy } from '@/i18n/choices';
import { show } from './support';

const server = vi.hoisted(() => ({ definitions: vi.fn<() => Promise<SettingDef[]>>(), values: vi.fn<() => Promise<ResolvedSetting[]>>(), location: { pathname: '/topic/topic', searchStr: '?tab=settings&section=policies' } }));
vi.mock('@/api/settings', async (load) => {
  const actual = await load<typeof SettingsApi>();
  return { ...actual, putSetting: vi.fn(),
    registryQuery: { queryKey: ['settings-registry'], queryFn: server.definitions },
    targetSettingsQuery: (target: SettingsApi.SettingsTarget) => ({ queryKey: actual.targetSettingsKey(target), queryFn: server.values }),
  };
});
vi.mock('@tanstack/react-router', async (load) => ({ ...await load<object>(), useLocation: () => server.location }));
vi.mock('@/features/help/open', () => ({ useHelpOpener: () => vi.fn() }));
const target = { kind: 'node', id: 'topic' } as const;
const place = { name: 'login', kind: 'topic', owner: { display_name: 'Ada Lovelace' }, access: { permissions: ['topic.view'] } };
const scroll = Object.getOwnPropertyDescriptor(Element.prototype, 'scrollIntoView');
beforeEach(async () => {
  await setLocale('en');
  vi.clearAllMocks();
  server.location = { pathname: '/topic/topic', searchStr: '?tab=settings&section=policies' };
  Object.defineProperty(Element.prototype, 'scrollIntoView', { configurable: true, writable: true, value: vi.fn() });
  class Observer { observe = vi.fn(); unobserve = vi.fn(); disconnect = vi.fn(); }
  vi.stubGlobal('ResizeObserver', Observer);
});
afterEach(async () => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
  if (scroll === undefined) Reflect.deleteProperty(Element.prototype, 'scrollIntoView');
  else Object.defineProperty(Element.prototype, 'scrollIntoView', scroll);
  await setLocale('en');
});

const definition = (index: number): SettingDef => ({ key: `test.feature${index}`, title: `Feature ${index}`, description: `Keep feature ${index} available.`,
  group: index % 2 === 0 ? 'agent' : 'confinement', class: 'parameter', scopes: ['topic'], merge: 'replace',
  schema: { type: 'boolean' }, default: false, example: true, text_key: null, keywords: [] });
const value = (def: SettingDef, current: unknown, writable: boolean): ResolvedSetting => ({ key: def.key, value: current, own: null,
  source: { scope: 'default', node: null }, locked_by: null, writable, parts: [] });

it.each(['en', 'zh-CN'] as const)('keeps one full search field and its Only here toggle on shared system and project editors in %s', async (locale) => {
  await setLocale(locale);
  const def = {...definition(0), scopes: ['system', 'project']};
  server.definitions.mockResolvedValue([def]);
  server.values.mockResolvedValue([value(def, false, true)]);
  for (const scope of ['system', 'project']) {
    const view = show(<SettingsEditor target={scope === 'system' ? {kind: 'system'} : {kind:'node',id:'web'}} scope={scope} place={{...place,kind:scope === 'system' ? null : 'project'}} compact />);
    const filter = await screen.findByRole('searchbox', {name: i18n.t('settings.editor.search')});
    expect(filter).toHaveAttribute('placeholder', locale === 'en' ? 'Search settings' : '搜索设置');
    expect(screen.getByRole('switch', {name: i18n.t('settings.editor.onlyHere')})).toBeEnabled();
    fireEvent.change(filter, {target: {value: 'Feature 0'}});
    await waitFor(() => expect(view.container.querySelectorAll('[data-setting]')).toHaveLength(1));
    view.unmount();
  }
});

it.each([false, true])('lands on a found setting while preserving an already opened choice: %s', async (interacting) => {
  const def: SettingDef = { ...definition(0), title: 'Work style', default: 'one', schema: { type: 'string', enum: ['one', 'two'] } };
  const initial = value(def, 'one', true);
  server.location = { pathname: '/topic/topic', searchStr: `?tab=settings&section=policies&setting=${def.key}` };
  server.definitions.mockResolvedValue([def]); server.values.mockResolvedValue([initial]);
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity } } });
  cache.setQueryData(['settings-registry'], [def]); cache.setQueryData(targetSettingsKey(target), [initial]);
  vi.useFakeTimers();
  const view = show(<QueryClientProvider client={cache}><SettingsEditor target={target} scope="topic" place={place} /></QueryClientProvider>);
  const field = screen.getByRole('combobox', { name: def.title });
  if (interacting) { act(() => field.focus()); fireEvent.click(field); }
  await act(async () => { await vi.advanceTimersByTimeAsync(250); });
  if (interacting) {
    expect(field).toHaveFocus();
    expect(screen.getByRole('listbox', { name: def.title })).toBeVisible();
    expect(field).toHaveAttribute('aria-expanded', 'true');
  } else expect(view.container.querySelector(`[data-setting="${def.key}"]`)).toHaveFocus();
  expect(putSetting).not.toHaveBeenCalled();
});

it.each(['en', 'zh-CN'] as const)('uses native topic write authority per row, explains a mixed collection honestly and folds fields when reach is lost in %s', async (locale) => {
  await setLocale(locale);
  const policy: SettingDef = { ...definition(0), key: 'test.topicPolicy', title: 'Topic policy', class: 'policy', schema: { type: 'integer' }, default: 8 };
  const parameter: SettingDef = { ...definition(2), key: 'test.topicParameter', title: 'Topic parameter', schema: { type: 'integer' }, default: 2 };
  const source = { scope: 'org', node: { id: 'acme', kind: 'org', name: 'Acme', redacted: false } } as const;
  server.definitions.mockResolvedValue([policy, parameter]);
  const rows = [{ ...value(policy, 8, false), source }, value(parameter, 2, true)];
  server.values.mockResolvedValue(rows);
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  const view = show(<QueryClientProvider client={cache}><SettingsEditor target={target} scope="topic" place={place} /></QueryClientProvider>);
  await screen.findByRole('spinbutton', { name: parameter.title });
  const policyRow = view.container.querySelector<HTMLElement>('[data-setting="test.topicPolicy"]');
  if (policyRow === null) throw new Error('The native policy row was not shown');
  expect(policyRow).toHaveAttribute('data-setting-readonly', 'true');
  expect(within(policyRow).getByText('8', { exact: true })).toBeVisible();
  expect(within(policyRow).getByRole('link', { name: i18n.t('settings.compact.inheritedFrom', { place: 'Acme' }) })).toBeVisible();
  expect(policyRow).toHaveTextContent('·');
  expect(within(policyRow).queryByRole('spinbutton')).not.toBeInTheDocument();
  expect(within(policyRow).queryByRole('button', { name: i18n.t('settings.compact.editValue', { name: policy.title }) })).not.toBeInTheDocument();
  expect(screen.getAllByText(i18n.t('settings.compact.someViewOnlyOwner', { person: 'Ada Lovelace' }))).toHaveLength(1);
  expect(screen.queryByText(i18n.t('settings.compact.viewOnlyOwner', { person: 'Ada Lovelace' }))).not.toBeInTheDocument();
  fireEvent.change(screen.getByRole('spinbutton', { name: parameter.title }), { target: { value: '4' } });
  act(() => { cache.setQueryData(targetSettingsKey(target), rows.map((row) => ({ ...row, writable: false }))); });
  await waitFor(() => expect(screen.queryByRole('spinbutton')).not.toBeInTheDocument());
  expect(screen.getAllByText(i18n.t('settings.compact.viewOnlyOwner', { person: 'Ada Lovelace' }))).toHaveLength(1);
  expect(screen.queryByText(i18n.t('settings.compact.someViewOnlyOwner', { person: 'Ada Lovelace' }))).not.toBeInTheDocument();
  expect(view.container.querySelector('[data-setting="test.topicParameter"]')).toHaveAttribute('data-setting-readonly', 'true');
  expect(putSetting).not.toHaveBeenCalled();
  expect(server.values).toHaveBeenCalledTimes(1);
});

// Draws every row and types through it as a person would: 4 to 15 s on a busy machine, so 30 s as the suite's other long cases.
it.each(['en', 'zh-CN'] as const)('condenses 146 view-only values under one sentence and shows approval modes as names in %s', { timeout: 30_000 }, async (locale) => {
  await setLocale(locale);
  const modes: SettingDef = { ...definition(145), key: 'approval.allowed_modes', title: 'Allowed approval modes', class: 'policy',
    group: 'confinement', merge: 'intersect', default: ['ask', 'on_request', 'auto', 'bypass'],
    schema: { type: 'array', uniqueItems: true, items: { enum: ['ask', 'on_request', 'auto', 'bypass'] } } };
  const definitions = [...Array.from({ length: 145 }, (_, index) => definition(index)), modes];
  server.definitions.mockResolvedValue(definitions);
  server.values.mockResolvedValue(definitions.map((def) => value(def, def.default, false)));
  const view = show(<SettingsEditor target={target} scope="topic" place={place} />);
  expect(await screen.findByText(i18n.t('settings.compact.viewOnlyOwner', { person: 'Ada Lovelace' }))).toBeVisible();
  expect(view.container.querySelectorAll('[data-read-only-settings]')).toHaveLength(1);
  const rows = view.container.querySelectorAll<HTMLElement>('[data-setting]');
  expect(rows).toHaveLength(146);
  expect(view.container.querySelectorAll('[data-setting-description]')).toHaveLength(146);
  for (const row of rows) {
    expect(within(row).queryByRole('switch')).not.toBeInTheDocument();
    expect(within(row).queryByRole('textbox')).not.toBeInTheDocument();
    expect(within(row).queryByRole('radio')).not.toBeInTheDocument();
    expect(within(row).queryByRole('button', { name: /^Edit / })).not.toBeInTheDocument();
  }
  const approval = view.container.querySelector<HTMLElement>('[data-setting="approval.allowed_modes"]');
  if (approval === null) throw new Error('The approval setting is missing');
  for (const mode of ['ask', 'on_request', 'auto', 'bypass'] as const) expect(within(approval).getByText(choiceCopy(i18n.t, `approvalMode.${mode}`).name)).toBeVisible();
  for (const marker of ['[', ']', '{', '}', '"', 'on_request']) expect(approval).not.toHaveTextContent(marker);
  expect(putSetting).not.toHaveBeenCalled();
  expect(server.definitions).toHaveBeenCalledTimes(1);
  expect(server.values).toHaveBeenCalledTimes(1);
});

it('jumps to a group without opening a new view or loading another setting list', { timeout: 30_000 }, async () => {
  const definitions = GROUP_ORDER.map((group, index) => ({ ...definition(index), group }));
  server.definitions.mockResolvedValue(definitions);
  server.values.mockResolvedValue(definitions.map((def) => value(def, false, false)));
  const view = show(<SettingsEditor target={target} scope="topic" place={place} />);
  const picker = await screen.findByRole('combobox', { name: i18n.t('settings.editor.groupsNav') });
  await userEvent.click(picker);
  expect(screen.getAllByRole('option')).toHaveLength(GROUP_ORDER.length);
  expect(GROUP_ORDER.length).toBeGreaterThan(10);
  await userEvent.type(picker, i18n.t('settings.editor.groups.system'));
  expect(putSetting).not.toHaveBeenCalled();
  expect(server.definitions).toHaveBeenCalledTimes(1);
  expect(server.values).toHaveBeenCalledTimes(1);
  await userEvent.click(screen.getByRole('option', { name: i18n.t('settings.editor.groups.system') }));
  const contents = view.container.querySelector<HTMLElement>('[data-settings-contents]');
  if (contents === null) throw new Error('The long page has no contents list');
  const link = within(contents).getByText(i18n.t('settings.editor.groups.system'));
  const href = link.getAttribute('href');
  if (href === null) throw new Error('The contents link has no destination');
  await waitFor(() => expect(document.getElementById(href.slice(1))).toHaveFocus());
  expect(link).toHaveAttribute('aria-current', 'location');
  expect(server.values).toHaveBeenCalledTimes(1);
});

it('searches a complete setting enum locally and autosaves only the exact selected value once', async () => {
  const choices = Array.from({ length: 12 }, (_, index) => `native_${index + 1}`);
  const def: SettingDef = { ...definition(0), title: 'Work style', default: choices[10], example: null,
    schema: { type: 'string', enum: choices } };
  const initial = value(def, 'future_style', true);
  server.definitions.mockResolvedValue([def]); server.values.mockResolvedValue([initial]);
  vi.mocked(putSetting).mockResolvedValue({ ...initial, value: 'native_12', own: { value: 'native_12', locked: false, revision: 1 },
    source: { scope: 'topic', node: { id: 'topic', kind: 'topic', name: 'login', redacted: false } } });
  const user = userEvent.setup();
  show(<SettingsEditor target={target} scope="topic" place={place} />);
  const field = await screen.findByRole('combobox', { name: def.title });
  expect(field).toHaveValue('Future style');
  await user.click(field);
  expect(screen.getAllByRole('option')).toHaveLength(12);
  await user.type(field, 'native_12');
  await user.keyboard('{Escape}');
  expect(field).toHaveValue('Future style');
  expect(putSetting).not.toHaveBeenCalled();
  expect(server.definitions).toHaveBeenCalledTimes(1); expect(server.values).toHaveBeenCalledTimes(1);
  await user.click(field); await user.type(field, 'native_12');
  expect(putSetting).not.toHaveBeenCalled();
  await user.keyboard('{Enter}');
  await waitFor(() => expect(putSetting).toHaveBeenCalledExactlyOnceWith(target, def.key, { value: 'native_12', expectedRevision: null }));
  expect(await screen.findByText('Saved')).toBeVisible();
  expect(screen.getByRole('button', { name: `Undo the change to ${def.title}` })).toBeVisible();
  expect(server.values).toHaveBeenCalledTimes(1);
});

it('commits a changed scalar once, shows Saved/Undo, and keeps Reset in the gear', async () => {
  const def: SettingDef = { ...definition(0), title: 'Concurrent tasks', default: 2, example: 4, schema: { type: 'integer', minimum: 1, maximum: 8 } };
  const initial = value(def, 2, true);
  server.definitions.mockResolvedValue([def]);
  server.values.mockResolvedValue([initial]);
  vi.mocked(putSetting).mockResolvedValue({ ...initial, value: 4, own: { value: 4, locked: false, revision: 1 }, source: { scope: 'topic', node: { id: 'topic', kind: 'topic', name: 'login', redacted: false } } });
  const user = userEvent.setup();
  show(<SettingsEditor target={target} scope="topic" place={place} />);
  const field = await screen.findByRole('spinbutton', { name: def.title });
  expect(screen.getAllByText(def.description)).toHaveLength(1);
  expect(screen.queryByRole('button', { name: 'Save' })).not.toBeInTheDocument();
  fireEvent.change(field, { target: { value: '4' } });
  expect(putSetting).not.toHaveBeenCalled();
  fireEvent.blur(field);
  await waitFor(() => expect(putSetting).toHaveBeenCalledExactlyOnceWith(target, def.key, { value: 4, expectedRevision: null }));
  expect(await screen.findByText('Saved')).toBeVisible();
  expect(screen.getByRole('button', { name: `Undo the change to ${def.title}` })).toBeVisible();
  expect(screen.queryByRole('button', { name: 'Reset to inherited' })).not.toBeInTheDocument();
  await user.click(screen.getByRole('button', { name: `More about ${def.title}` }));
  expect(screen.getByRole('menuitem', { name: 'Reset to inherited' })).toBeVisible();
  expect(server.values).toHaveBeenCalledTimes(1);
});
