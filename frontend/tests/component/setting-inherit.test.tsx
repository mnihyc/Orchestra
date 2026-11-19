/** Editable values stay in their compact row, with source, guards and failed drafts retained. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, fireEvent, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { clearSetting, putSetting, type SettingsTarget } from '@/api/settings';
import { SettingRow } from '@/features/settings/SettingRow';
import { ValueControl } from '@/features/settings/ValueControl';
import type { KeyDef } from '@/features/settings/settingsModel';
import type { ResolvedSetting, SettingPart } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

vi.mock('@/features/help/open', () => ({ useHelpOpener: () => vi.fn() }));
vi.mock('@/api/settings', async (original) => ({ ...await original<object>(), putSetting: vi.fn(), clearSetting: vi.fn() }));
vi.mock('@/api/nodes', async (original) => ({ ...await original<object>(), nodeQuery: (id: string) => ({
  queryKey: ['setting-source-node', id], queryFn: () => Promise.resolve({ kind: 'org', id, address: id, name: id }),
}) }));

const acme = { id: 'acme', kind: 'org', name: 'Acme', redacted: false } as const;
const web = { id: 'web', kind: 'project', name: 'Web', redacted: false } as const;
const target: SettingsTarget = { kind: 'node', id: 'web' };
const place = { name: 'Web', kind: 'project', access: { permissions: ['project.manage', 'settings.manage'] }, owner: null };

function key(name: string, schema: Record<string, unknown>, fallback: unknown, more: Partial<KeyDef> = {}): KeyDef {
  return { key: name, title: name, description: 'What ' + name + ' does', group: 'agent', class: 'parameter',
    scopes: ['system', 'org', 'project', 'topic'], merge: 'replace', default: fallback, example: fallback,
    schema, text_key: null, keywords: [], ...more };
}
function part(node: typeof acme | typeof web | null, value: unknown, scope: SettingPart['source']['scope']): SettingPart {
  return { source: { scope, node }, value, locked: false, ignored: false };
}
function inherited(def: KeyDef, above: unknown, own?: unknown): ResolvedSetting {
  return { key: def.key, value: own === undefined ? above : own,
    own: own === undefined ? null : { value: own, revision: 2, locked: false },
    source: own === undefined ? { scope: 'org', node: acme } : { scope: 'project', node: web },
    locked_by: null, writable: true,
    parts: [part(null, def.default, 'default'), part(acme, above, 'org'), ...(own === undefined ? [] : [part(web, own, 'project')])] };
}
function draw(def: KeyDef, setting: ResolvedSetting) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  // The ordinary page already has its org's served metadata; the compact rows do not fetch it again.
  client.setQueryData(['setting-source-node', 'acme'], { kind: 'org', id: 'acme', address: 'acme', name: 'Acme' });
  const view = show(<QueryClientProvider client={client}><ul><SettingRow def={def} setting={setting} target={target} place={place} /></ul></QueryClientProvider>);
  const element = view.container.querySelector<HTMLElement>('[data-setting]');
  if (element === null) throw new Error('The setting row was not drawn');
  return { ...view, row: within(element), element };
}
beforeEach(async () => {
  await setLocale('en');
  vi.clearAllMocks();
  class Observer { observe = vi.fn(); unobserve = vi.fn(); disconnect = vi.fn(); }
  vi.stubGlobal('ResizeObserver', Observer);
});
afterEach(async () => { vi.unstubAllGlobals(); await setLocale('en'); });

it.each(['en', 'zh-CN'] as const)('edits an inherited boolean directly with one value switch in %s', async (locale) => {
  await setLocale(locale);
  const def = key('agent.web_search', { type: 'boolean' }, false);
  const next = inherited(def, true, false);
  vi.mocked(putSetting).mockResolvedValue(next);
  const user = userEvent.setup();
  const { row } = draw(def, inherited(def, true));
  expect(row.getAllByRole('switch')).toHaveLength(1);
  const value = row.getByRole('switch', { name: def.title });
  expect(value).toBeChecked();
  expect(row.queryAllByRole('radio')).toHaveLength(0);
  expect(row.getAllByText(def.description)).toHaveLength(1);
  await user.click(value);
  await waitFor(() => expect(putSetting).toHaveBeenCalledExactlyOnceWith(target, def.key, { value: false, expectedRevision: null }));
  expect(row.getByRole('switch', { name: def.title })).not.toBeChecked();
});

it('edits a number in place and automatically commits its guarded value on blur', async () => {
  const def = key('env.idle_stop_minutes', { type: 'integer', minimum: 0 }, 60, { display: { unit: 'minutes', zero: 'never' } });
  vi.mocked(putSetting).mockResolvedValue(inherited(def, 720, 90));
  const { row } = draw(def, inherited(def, 720));
  const field = row.getByRole('spinbutton', { name: def.title });
  expect(field).toHaveValue(720);
  expect(row.queryByRole('button', { name: 'Save' })).not.toBeInTheDocument();
  fireEvent.change(field, { target: { value: '90' } });
  expect(putSetting).not.toHaveBeenCalled();
  fireEvent.blur(field);
  await waitFor(() => expect(putSetting).toHaveBeenCalledWith(target, def.key, { value: 90, expectedRevision: null }));
  expect(row.getByRole('spinbutton', { name: def.title })).toHaveValue(90);
  expect(row.queryByRole('button', { name: 'Save' })).not.toBeInTheDocument();
  expect(row.getByText('Saved')).toBeVisible();
});

it.each(['en', 'zh-CN'] as const)('shows a disabled numeric behavior as Off and edits its actual zero in %s', async (locale) => {
  await setLocale(locale);
  const def = key('env.unused_remove_days', { type: 'integer', minimum: 0 }, 0, { display: { unit: 'days', zero: 'off' } });
  vi.mocked(putSetting).mockResolvedValue(inherited(def, 0, 14));
  const user = userEvent.setup();
  const { row } = draw(def, inherited(def, 0));
  const field = row.getByRole('textbox', { name: def.title });
  expect(field).toHaveValue(i18n.t('common.off'));
  await user.click(field);
  expect(row.getByRole('spinbutton', { name: def.title })).toHaveValue(0);
  expect(putSetting).not.toHaveBeenCalled();
  fireEvent.change(field, { target: { value: '14' } });
  fireEvent.blur(field);
  await waitFor(() => expect(putSetting).toHaveBeenCalledExactlyOnceWith(target, def.key, { value: 14, expectedRevision: null }));
  expect(row.getByRole('spinbutton', { name: def.title })).toHaveValue(14);
});

it.each(['en', 'zh-CN'] as const)('shows an unset default Embeddings value as Not set in its real row in %s', async (locale) => {
  await setLocale(locale);
  const def = key('search.semantic.embeddings', {type: 'object', properties: {}}, null);
  const value: ResolvedSetting = {...inherited(def, null), source: {scope: 'default', node: null}, parts: [part(null, null, 'default')]};
  const view = draw(def, value);
  expect(view.row.getByText(i18n.t('admin.semantic.notSet'), {exact: true})).toBeVisible();
  expect(view.row.queryByText(i18n.t('settings.compact.defaultNone'), {exact: true})).toBeNull();
  view.unmount();
});

it.each(['en', 'zh-CN'] as const)('shows Off in the zero pasted-text percentage field and exposes its actual numeric value on focus in %s', async (locale) => {
  await setLocale(locale);
  show(<ValueControl shape={{ type: 'int', min: 0, max: 100, unit: 'percent' }} display={{ unit: 'percent', zero: 'off' }} value={0} onChange={vi.fn()} label="Paste percentage" />);
  const user=userEvent.setup();
  const field=screen.getByRole('textbox',{name:'Paste percentage'});
  expect(field).toHaveValue(i18n.t('common.off'));
  await user.click(field);
  expect(screen.getByRole('spinbutton', { name: 'Paste percentage' })).toHaveValue(0);
  expect(screen.getByText(i18n.t('common.off'))).toBeVisible();
  expect(screen.getByText(i18n.t('settings.editor.units.percent'))).toBeVisible();
  fireEvent.blur(field);
  expect(field).toHaveValue(i18n.t('common.off'));
});

it('keeps a newer live value when an earlier accepted write answers late, with its Undo fenced at the accepted revision', async () => {
  const def = key('test.number', { type: 'integer' }, 0);
  const initial = inherited(def, 720, 90);
  const accepted = { ...inherited(def, 720, 120), own: { value: 120, locked: false, revision: 3 } };
  const newer = { ...inherited(def, 720, 180), own: { value: 180, locked: false, revision: 4 } };
  let finish: (value: ResolvedSetting) => void = () => { throw new Error('The setting write is not pending'); };
  const pending = new Promise<ResolvedSetting>((resolve) => { finish = resolve; });
  vi.mocked(putSetting).mockReturnValueOnce(pending);
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  const node = (setting: ResolvedSetting) => <QueryClientProvider client={client}><ul><SettingRow def={def} setting={setting} target={target} place={place} /></ul></QueryClientProvider>;
  const view = show(node(initial));
  fireEvent.change(screen.getByRole('spinbutton', { name: def.title }), { target: { value: '120' } });
  fireEvent.blur(screen.getByRole('spinbutton', { name: def.title }));
  await waitFor(() => expect(putSetting).toHaveBeenCalledExactlyOnceWith(target, def.key, { value: 120, expectedRevision: 2 }));
  act(() => { client.setQueryData(['settings', 'web'], [newer]); });
  view.rerender(node(newer));
  await act(async () => { finish(accepted); await pending; });
  await waitFor(() => expect(screen.getByRole('spinbutton', { name: def.title })).toHaveValue(180));
  expect(client.getQueryData<ResolvedSetting[]>(['settings', 'web'])?.[0]?.own?.revision).toBe(4);
  const undo = screen.getByRole('button', { name: `Undo the change to ${def.title}` });
  expect(undo).toHaveAttribute('aria-disabled', 'true');
  await userEvent.click(undo);
  expect(clearSetting).not.toHaveBeenCalled();
  expect(putSetting).toHaveBeenCalledTimes(1);
});

it('Cancel restores the current value without writing, and Reset removes only the stored own key', async () => {
  const def = key('test.number', { type: 'integer' }, 0);
  vi.mocked(clearSetting).mockResolvedValue(inherited(def, 720));
  const user = userEvent.setup();
  const { row } = draw(def, inherited(def, 720, 90));
  row.getByRole('spinbutton', { name: def.title }).focus();
  fireEvent.change(row.getByRole('spinbutton', { name: def.title }), { target: { value: '120' } });
  await user.click(row.getByRole('button', { name: 'Cancel' }));
  expect(row.getByRole('spinbutton', { name: def.title })).toHaveValue(90);
  expect(putSetting).not.toHaveBeenCalled();
  await user.click(row.getByRole('button', { name: 'More about test.number' }));
  await user.click(screen.getByRole('menuitem', { name: 'Reset to inherited' }));
  await waitFor(() => expect(clearSetting).toHaveBeenCalledExactlyOnceWith(target, def.key, 2));
  expect(row.getByRole('spinbutton', { name: def.title })).toHaveValue(720);
});

it('shows its description once, keeps only supplementary constraints in the question, and links to the actual source', async () => {
  const def = key('test.limit', { type: 'integer', minimum: 1, maximum: 24 }, 8);
  const user = userEvent.setup();
  const { row } = draw(def, inherited(def, 12));
  expect(row.getAllByText(def.description)).toHaveLength(1);
  const source = await row.findByRole('link', { name: 'Inherited from Acme' });
  expect(source).toHaveAttribute('href', '/org/acme/settings/policies?setting=test.limit');
  const range = i18n.t('settings.editor.between', { min: 1, max: 24 });
  expect(screen.queryByText(range)).not.toBeInTheDocument();
  await user.click(row.getByRole('button', { name: i18n.t('help.hint.label', { term: def.title }) }));
  expect(screen.getByText(range)).toBeVisible();
  expect(screen.getAllByText(def.description)).toHaveLength(1);
  await user.keyboard('{Escape}');
  expect(screen.queryByText(range)).not.toBeInTheDocument();
  expect(row.getByText(def.description)).toBeVisible();
});

it('uses a short linked refusal and the effective locked value, without a grey disabled editor', async () => {
  const def = key('agent.web_search', { type: 'boolean' }, false);
  const setting = inherited(def, true, false);
  setting.value = true;
  setting.writable = false;
  setting.locked_by = { scope: 'org', node: acme };
  setting.parts = setting.parts.map((entry) => entry.source.node?.id === web.id ? { ...entry, ignored: true } : entry);
  const { row } = draw(def, setting);
  expect(row.queryByRole('switch')).not.toBeInTheDocument();
  expect(row.queryByRole('textbox')).not.toBeInTheDocument();
  expect(row.getByText('On')).toBeVisible();
  expect(row.queryByText('Off')).not.toBeInTheDocument();
  expect(await row.findByRole('link', { name: 'Locked at Acme' })).toHaveAttribute('href', '/org/acme/settings/policies?setting=agent.web_search');
  expect(putSetting).not.toHaveBeenCalled();
});

it('does not turn a hidden source into a link or a guessed destination', () => {
  const def = key('test.hidden', { type: 'integer' }, 0);
  const hidden = { id: 'secret', kind: 'org', name: null, redacted: true } as const;
  const setting = { ...inherited(def, 30), writable: false, source: { scope: 'org' as const, node: hidden }, locked_by: { scope: 'org' as const, node: hidden } };
  const { row, element } = draw(def, setting);
  expect(row.getByText('Set above')).toBeVisible();
  expect(element.querySelector('a[href*="secret"]')).toBeNull();
});

it('retains a refused draft and its original revision when a newer row arrives', async () => {
  const def = key('test.draft', { type: 'string', maxLength: 100 }, '');
  const first = inherited(def, 'Above', 'Mine');
  vi.mocked(putSetting).mockRejectedValue(new Error('refused'));
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  const frame = (setting: ResolvedSetting) => <QueryClientProvider client={cache}><ul><SettingRow def={def} setting={setting} target={target} place={place} /></ul></QueryClientProvider>;
  const view = show(frame(first));
  fireEvent.change(screen.getByRole('textbox', { name: def.title }), { target: { value: 'Still writing' } });
  fireEvent.blur(screen.getByRole('textbox', { name: def.title }));
  await waitFor(() => expect(putSetting).toHaveBeenCalledWith(target, def.key, { value: 'Still writing', expectedRevision: 2 }));
  await screen.findByRole('button', { name: 'Try again' });
  view.rerender(frame({ ...first, value: 'Other writer', own: { value: 'Other writer', revision: 3, locked: false } }));
  expect(screen.getByRole('textbox', { name: def.title })).toHaveValue('Still writing');
  expect(screen.getByText(i18n.t('settings.compact.changedReason'))).toBeVisible();
  expect(putSetting).toHaveBeenCalledTimes(1);
  cache.clear();
});

it('keeps long and joining values compact until opened, and submits only the local contribution', async () => {
  const def = key('agent.instructions', { type: 'string', maxLength: 20000 }, '', { merge: 'append' });
  const value = inherited(def, 'Ancestor', 'Local');
  value.value = 'Ancestor\n\nLocal';
  vi.mocked(putSetting).mockResolvedValue(value);
  const user = userEvent.setup();
  const { row } = draw(def, value);
  expect(row.queryByRole('textbox')).not.toBeInTheDocument();
  await user.click(row.getByRole('button', { name: 'Edit agent.instructions' }));
  const field = screen.getByRole('textbox', { name: def.title });
  expect(field).toHaveValue('Local');
  expect(field.closest('[data-setting-inline-editor]')).not.toBeNull();
  expect(row.queryByRole('group', { name: 'Inherited parts of agent.instructions' })).not.toBeInTheDocument();
  expect(row.queryByText('Own value', { exact: true })).not.toBeInTheDocument();
  expect(row.queryByText('Resolved value', { exact: true })).not.toBeInTheDocument();
  fireEvent.change(field, { target: { value: 'Edited local' } });
  field.focus();
  await user.keyboard('{Control>}{Enter}{/Control}');
  await waitFor(() => expect(putSetting).toHaveBeenCalledWith(target, def.key, { value: 'Edited local', expectedRevision: 2 }));
  await waitFor(() => expect(row.queryByRole('textbox')).not.toBeInTheDocument());
  expect(row.getByRole('button', { name: 'Edit agent.instructions' })).toHaveFocus();
});

it.each(['en', 'zh-CN'] as const)('shows a real default beside its control and an unset default as its placeholder, never an Empty edit button in %s', async (locale) => {
  await setLocale(locale);
  const number = key('test.defaultNumber', { type: 'integer', minimum: 0 }, 8);
  const text = key('test.defaultText', { type: 'string', maxLength: 100 }, '');
  const list = key('test.defaultModes', { type: 'array', uniqueItems: true, items: { type: 'string', enum: ['ask', 'auto'] } }, [], { merge: 'intersect' });
  const defaults = (def: KeyDef): ResolvedSetting => ({ key: def.key, value: def.default, own: null, locked_by: null,
    source: { scope: 'default', node: null }, parts: [], writable: true });
  const numeric = draw(number, defaults(number));
  expect(numeric.row.getByRole('spinbutton', { name: number.title })).toHaveValue(8);
  expect(numeric.row.getAllByText(i18n.t('settings.compact.defaultPrefix'), { exact: true })).toHaveLength(1);
  expect(numeric.row.queryByText(i18n.t('settings.editor.from.default'), { exact: true })).not.toBeInTheDocument();
  numeric.unmount();
  const unset = draw(text, defaults(text));
  expect(unset.row.getByRole('textbox', { name: text.title })).toHaveValue('');
  expect(unset.row.getByRole('textbox', { name: text.title })).toHaveAttribute('placeholder', i18n.t('settings.compact.defaultNone'));
  unset.unmount();
  const modes = draw(list, defaults(list));
  expect(modes.row.getByText(i18n.t('settings.compact.defaultNone'), { exact: true })).toBeVisible();
  expect(modes.row.getByRole('button', { name: i18n.t('settings.compact.editValue', { name: list.title }) })).toHaveAccessibleDescription(i18n.t('settings.compact.defaultNone'));
  expect(modes.row.queryByRole('button', { name: i18n.t('day100.settings.empty') })).not.toBeInTheDocument();
  expect(putSetting).not.toHaveBeenCalled();
});

it('commits a completed complex draft when it closes and keeps its value in words', async () => {
  const def = key('test.properties', { type: 'object', additionalProperties: true }, {});
  const user = userEvent.setup();
  vi.mocked(putSetting).mockResolvedValue(inherited(def, {}, { safe: true }));
  const { row } = draw(def, inherited(def, {}));
  expect(row.getByText(i18n.t('settings.compact.none'), { exact: true })).toBeVisible();
  expect(row.getByRole('button', { name: 'Edit test.properties' })).toHaveAccessibleDescription(i18n.t('settings.compact.none'));
  expect(row.queryByRole('button', { name: 'Empty' })).not.toBeInTheDocument();
  expect(row.queryByText('{}')).not.toBeInTheDocument();
  await user.click(row.getByRole('button', { name: 'Edit test.properties' }));
  const field = screen.getByRole('textbox', { name: def.title });
  expect(field).toHaveFocus();
  fireEvent.change(field, { target: { value: '{"safe":true}' } });
  field.focus();
  await user.keyboard('{Escape}');
  await waitFor(() => expect(putSetting).toHaveBeenCalledExactlyOnceWith(target, def.key, { value: { safe: true }, expectedRevision: null }));
  expect(row.getByRole('button', { name: 'Edit test.properties' })).toHaveFocus();
  expect(row.queryByRole('textbox')).not.toBeInTheDocument();
  expect(row.getByText('Saved')).toBeVisible();
  await user.click(row.getByRole('button', { name: 'Edit test.properties' }));
  expect(screen.getByRole('textbox', { name: def.title })).toHaveValue('{\n  "safe": true\n}');
  expect(screen.getByRole('textbox', { name: def.title })).toHaveFocus();
  await user.keyboard('{Escape}');
  expect(row.getByRole('button', { name: 'Edit test.properties' })).toHaveAccessibleDescription('safe: On');
  expect(putSetting).toHaveBeenCalledTimes(1);
});

it('retains incomplete JSON on close and refuses to save it until it can be read', async () => {
  const def = key('test.json', { type: 'object', additionalProperties: true }, {});
  const user = userEvent.setup();
  const { row } = draw(def, inherited(def, {}));
  await user.click(row.getByRole('button', { name: 'Edit test.json' }));
  const field = row.getByRole('textbox', { name: def.title });
  fireEvent.change(field, { target: { value: '{"unfinished":' } });
  field.focus();
  await user.keyboard('{Escape}');
  expect(row.getByText(i18n.t('settings.compact.invalidValue'))).toBeVisible();
  await user.click(row.getByRole('button', { name: 'Edit test.json' }));
  expect(row.getByRole('textbox', { name: def.title })).toHaveValue('{"unfinished":');
  expect(row.getByRole('textbox', { name: def.title })).toBeInvalid();
  const form = row.getByRole('textbox', { name: def.title }).closest('form');
  if (form === null) throw new Error('The setting has no committing form');
  fireEvent.submit(form);
  expect(putSetting).not.toHaveBeenCalled();
  await user.click(row.getByRole('button', { name: 'Cancel' }));
  await user.click(row.getByRole('button', { name: 'Edit test.json' }));
  expect(row.getByRole('textbox', { name: def.title })).toHaveValue('{}');
  expect(row.getByRole('textbox', { name: def.title })).toBeValid();
});

it('undoes a newly stored value by removing only the returned revision', async () => {
  const def = key('test.number', { type: 'integer' }, 0);
  vi.mocked(putSetting).mockResolvedValue(inherited(def, 720, 90));
  vi.mocked(clearSetting).mockResolvedValue(inherited(def, 720));
  const user = userEvent.setup();
  const { row } = draw(def, inherited(def, 720));
  const field = row.getByRole('spinbutton', { name: def.title });
  fireEvent.change(field, { target: { value: '90' } });
  fireEvent.blur(field);
  const undo = await row.findByRole('button', { name: 'Undo the change to test.number' });
  await user.click(undo);
  await waitFor(() => expect(clearSetting).toHaveBeenCalledExactlyOnceWith(target, def.key, 2));
  expect(row.getByRole('spinbutton', { name: def.title })).toHaveValue(720);
  expect(row.getByText('Undone')).toBeVisible();
});

it('undoes a replacement with its original own value and lock, under the saved answer’s fence', async () => {
  const def = key('test.number', { type: 'integer' }, 0);
  const original = inherited(def, 720, 90);
  const updated = { ...inherited(def, 720, 120), own: { value: 120, revision: 3, locked: false } };
  vi.mocked(putSetting).mockResolvedValueOnce(updated).mockResolvedValueOnce({ ...original, own: { ...original.own!, revision: 4 } });
  const user = userEvent.setup();
  const { row } = draw(def, original);
  fireEvent.change(row.getByRole('spinbutton', { name: def.title }), { target: { value: '120' } });
  fireEvent.blur(row.getByRole('spinbutton', { name: def.title }));
  await user.click(await row.findByRole('button', { name: 'Undo the change to test.number' }));
  await waitFor(() => expect(putSetting).toHaveBeenLastCalledWith(target, def.key, { value: 90, locked: false, expectedRevision: 3 }));
  expect(row.getByRole('spinbutton', { name: def.title })).toHaveValue(90);
  expect(row.getByText('Undone')).toBeVisible();
});
