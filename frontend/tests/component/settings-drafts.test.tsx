/** An object or a list is edited as a draft that only its own Save writes: nothing else on the page saves, closes or hides it. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { Activity, useRef, useState } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as SettingsApi from '@/api/settings';
import { putSetting, targetSettingsKey } from '@/api/settings';
import { SettingsEditor } from '@/features/settings/Editor';
import { DetailPages, DetailSections, DETAIL_PAGE_ROWS } from '@/features/resources/DetailSections';
import { SectionNav } from '@/ui/SectionNav';
import { useUnsavedDrafts } from '@/features/settings/drafts';
import { CloseViews } from '@/features/workbench/CloseViews';
import { ViewInstanceContext } from '@/features/workbench/instance';
import { allTabs } from '@/features/workbench/layout';
import { useWorkbench } from '@/features/workbench/store';
import type { ResolvedSetting, SettingDef } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

const server = vi.hoisted(() => ({ definitions: vi.fn<() => Promise<SettingDef[]>>(), values: vi.fn<() => Promise<ResolvedSetting[]>>() }));
vi.mock('@/api/settings', async (load) => {
  const actual = await load<typeof SettingsApi>();
  return { ...actual, putSetting: vi.fn(),
    registryQuery: { queryKey: ['settings-registry'], queryFn: server.definitions },
    targetSettingsQuery: (target: SettingsApi.SettingsTarget) => ({ queryKey: actual.targetSettingsKey(target), queryFn: server.values }),
  };
});
vi.mock('@tanstack/react-router', async (load) => ({ ...await load<object>(), useLocation: () => ({ pathname: '/topic/topic', searchStr: '?tab=settings&section=defaults' }) }));
vi.mock('@/features/help/open', () => ({ useHelpOpener: () => vi.fn() }));

const target = { kind: 'node', id: 'topic' } as const;
const place = { name: 'login', kind: 'topic', owner: { display_name: 'Ada Lovelace' }, access: { permissions: ['topic.view'] } };
const here = { scope: 'topic', node: { id: 'topic', kind: 'topic', name: 'login', redacted: false } } as const;
const scroll = Object.getOwnPropertyDescriptor(Element.prototype, 'scrollIntoView');

/** What the registry ships for the summary targets: every target off, with the numbers the engine falls back to. */
const shipped = (seconds: number, on: string[], chars = 600) => ({ automatic: false, on, min_interval_secs: seconds, context_tokens: 32_000, output_chars: chars });
const SHIPPED = { thread: shipped(120, ['turn_completed', 'milestone']), topic: shipped(900, ['child_changed']), document: shipped(300, ['content_changed']),
  chat: shipped(300, ['message_posted']), chat_title: shipped(300, ['message_posted'], 80) };
const ENTRY = { type: 'object', additionalProperties: false, required: ['automatic'], properties: {
  automatic: { type: 'boolean' },
  on: { type: 'array', uniqueItems: true, items: { type: 'string', enum: ['turn_completed', 'milestone', 'interval', 'content_changed', 'message_posted', 'child_changed'] } },
  min_interval_secs: { type: 'integer', minimum: 0 }, context_tokens: { type: 'integer', minimum: 1 }, output_chars: { type: 'integer', minimum: 1 },
} };
const targets: SettingDef = { key: 'summary.targets', title: 'Summary targets', description: 'When and how much each target is summarized.', group: 'automation', class: 'parameter',
  scopes: ['topic'], merge: 'map', default: SHIPPED, example: null, text_key: null, keywords: [],
  schema: { type: 'object', additionalProperties: false, properties: { thread: ENTRY, topic: ENTRY, document: ENTRY, chat: ENTRY, chat_title: ENTRY } } };
const flag: SettingDef = { key: 'test.flag', title: 'Keep awake', description: 'Keep the thread available.', group: 'agent', class: 'parameter', scopes: ['topic'], merge: 'replace',
  schema: { type: 'boolean' }, default: false, example: true, text_key: null, keywords: [] };
const row = (def: SettingDef, current: unknown): ResolvedSetting => ({ key: def.key, value: current, own: null, source: { scope: 'default', node: null }, locked_by: null, writable: true, parts: [] });

function Count() { return <output aria-label="Open drafts">{useUnsavedDrafts()}</output>; }
function draw() {
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity }, mutations: { retry: false } } });
  return show(<QueryClientProvider client={cache}><Count /><SettingsEditor target={target} scope="topic" place={place} /></QueryClientProvider>);
}
const editorOf = (name: string) => screen.getByRole('button', { name: i18n.t('settings.compact.editValue', { name }) });

beforeEach(async () => {
  await setLocale('en'); vi.clearAllMocks();
  useWorkbench.getState().leave();
  Object.defineProperty(Element.prototype, 'scrollIntoView', { configurable: true, writable: true, value: vi.fn() });
  server.definitions.mockResolvedValue([flag, targets]);
  server.values.mockResolvedValue([row(flag, false), row(targets, SHIPPED)]);
  vi.mocked(putSetting).mockImplementation((_target, key, body) => Promise.resolve({ ...row(key === flag.key ? flag : targets, body.value), value: body.value,
    own: { value: body.value, locked: false, revision: 1 }, source: here }));
});
afterEach(async () => {
  cleanup();
  useWorkbench.getState().leave();
  if (scroll === undefined) Reflect.deleteProperty(Element.prototype, 'scrollIntoView'); else Object.defineProperty(Element.prototype, 'scrollIntoView', scroll);
  await setLocale('en');
});

/** Real close plans and Activity pauses, with one independent settings body per workbench instance. */
function SettingsView({ id, hidden = false, presentation = 'editor' }: { id: string; hidden?: boolean; presentation?: 'editor' | 'section' | 'page' | 'navigation' }) {
  const root = useRef<HTMLDivElement>(null);
  const live = useWorkbench((state) => allTabs(state.layout).some((tab) => tab.id === id));
  if (!live) return null;
  return <ViewInstanceContext value={{ id, root, target: null, headingOwner: null, content: true, focusedContent: true }}>
    <section aria-label={id}><Activity mode={hidden ? 'hidden' : 'visible'}><Count /><SettingsBody presentation={presentation} /></Activity></section>
  </ViewInstanceContext>;
}

function SettingsBody({ presentation }: { presentation: 'editor' | 'section' | 'page' | 'navigation' }) {
  const [section, setSection] = useState('defaults');
  const unsaved = useUnsavedDrafts() > 0;
  const editor = <SettingsEditor target={target} scope="topic" place={place} />;
  if (presentation === 'section') return <DetailSections label="Resource sections" sections={[{ value: 'defaults', label: 'Defaults', panel: editor }, { value: 'other', label: 'Other', panel: <p>Other section</p> }]} />;
  if (presentation === 'page') return <DetailPages items={Array.from({ length: DETAIL_PAGE_ROWS + 1 }, (_, index) => index)}>{(items) => items[0] === 0 ? editor : <p>Other page</p>}</DetailPages>;
  if (presentation === 'navigation') return <><SectionNav label="Place sections" current={section} unsaved={unsaved} items={['defaults', 'other'].map((id) => ({ id, label: id === 'defaults' ? 'Defaults' : 'Other', link: <a href={`#${id}`} onClick={(event) => { event.preventDefault(); setSection(id); }} /> }))} />{section === 'defaults' ? editor : <p>Other section</p>}</>;
  return editor;
}

it.each(['section', 'page'] as const)('keeps a draft protected while its resource %s is hidden', async (presentation) => {
  const user = userEvent.setup();
  const id = useWorkbench.getState().openTab('page', { href: '/topic/topic?tab=settings' });
  show(<><SettingsView id={id} presentation={presentation} /><CloseViews /></>);
  await user.click(await screen.findByRole('button', { name: i18n.t('settings.compact.editValue', { name: targets.title }) }));
  await user.click(screen.getByRole('switch', { name: 'Thread: Automatic' }));
  if (presentation === 'section') await user.click(screen.getByRole('link', { name: 'Other' }));
  else await user.click(screen.getByRole('button', { name: i18n.t('resourceDetails.layout.next') }));
  expect(useWorkbench.getState().unsaved.has(id)).toBe(true);
  act(() => useWorkbench.getState().close({ kind: 'tab', tab: id }));
  const question = await screen.findByRole('dialog');
  await user.click(within(question).getByRole('button', { name: i18n.t('workbench.workspace.saveAll') }));
  await waitFor(() => expect(putSetting).toHaveBeenCalledExactlyOnceWith(target, targets.key, { value: { thread: { automatic: true } }, expectedRevision: null }));
  await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
});

it('uses the view loss decision when changing settings sections and discards only that view', async () => {
  const user = userEvent.setup();
  const a = useWorkbench.getState().openTab('page', { href: '/topic/topic?tab=settings' }, 'new');
  const b = useWorkbench.getState().openTab('page', { href: '/topic/topic?tab=settings' }, 'beside');
  show(<><SettingsView id={a} presentation="navigation" /><SettingsView id={b} /><CloseViews /></>);
  const left = within(screen.getByRole('region', { name: a }));
  const right = within(screen.getByRole('region', { name: b }));
  await user.click(await left.findByRole('button', { name: i18n.t('settings.compact.editValue', { name: targets.title }) }));
  await user.click(left.getByRole('switch', { name: 'Thread: Automatic' }));
  await user.click(await right.findByRole('button', { name: i18n.t('settings.compact.editValue', { name: targets.title }) }));
  await user.click(right.getByRole('switch', { name: 'Topic: Automatic' }));
  await user.click(left.getByRole('link', { name: 'Other' }));
  const question = await screen.findByRole('dialog');
  await user.click(within(question).getByRole('button', { name: i18n.t('workbench.workspace.discard') }));
  expect(await left.findByText('Other section')).toBeVisible();
  expect(useWorkbench.getState().unsaved.has(a)).toBe(false);
  expect(useWorkbench.getState().unsaved.has(b)).toBe(true);
  expect(right.getByRole('switch', { name: 'Topic: Automatic' })).toBeChecked();
  expect(putSetting).not.toHaveBeenCalled();
});

it('asks before closing a hidden settings view and saves its retained draft', async () => {
  const user = userEvent.setup();
  const id = useWorkbench.getState().openTab('page', { href: '/topic/topic?tab=settings' });
  const view = show(<><SettingsView id={id} /><CloseViews /></>);
  await user.click(await screen.findByRole('button', { name: i18n.t('settings.compact.editValue', { name: targets.title }) }));
  await user.click(screen.getByRole('switch', { name: 'Thread: Automatic' }));
  view.rerender(<><SettingsView id={id} hidden /><CloseViews /></>);
  expect(useWorkbench.getState().unsaved.has(id)).toBe(true);
  act(() => useWorkbench.getState().close({ kind: 'tab', tab: id }));
  const question = await screen.findByRole('dialog');
  await user.click(within(question).getByRole('button', { name: i18n.t('workbench.workspace.saveAll') }));
  await waitFor(() => expect(putSetting).toHaveBeenCalledExactlyOnceWith(target, targets.key, { value: { thread: { automatic: true } }, expectedRevision: null }));
  await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
  expect(allTabs(useWorkbench.getState().layout).some((tab) => tab.id === id)).toBe(false);
});

it.each(['save', 'discard'] as const)('lets a view %s its own draft without changing another view of the same setting', async (action) => {
  const user = userEvent.setup();
  const a = useWorkbench.getState().openTab('page', { href: '/topic/topic?tab=settings' }, 'new');
  const b = useWorkbench.getState().openTab('page', { href: '/topic/topic?tab=settings' }, 'beside');
  show(<><SettingsView id={a} /><SettingsView id={b} /><CloseViews /></>);
  const left = within(screen.getByRole('region', { name: a }));
  const right = within(screen.getByRole('region', { name: b }));
  await user.click(await left.findByRole('button', { name: i18n.t('settings.compact.editValue', { name: targets.title }) }));
  await user.click(left.getByRole('switch', { name: 'Thread: Automatic' }));
  expect(useWorkbench.getState().unsaved.has(a)).toBe(true);
  expect(useWorkbench.getState().unsaved.has(b)).toBe(false);
  await user.click(await right.findByRole('button', { name: i18n.t('settings.compact.editValue', { name: targets.title }) }));
  await user.click(right.getByRole('switch', { name: 'Topic: Automatic' }));
  expect(left.getByLabelText('Open drafts')).toHaveTextContent('1');
  expect(right.getByLabelText('Open drafts')).toHaveTextContent('1');
  act(() => useWorkbench.getState().close({ kind: 'tab', tab: a }));
  const question = await screen.findByRole('dialog');
  await user.click(within(question).getByRole('button', { name: i18n.t(action === 'save' ? 'workbench.workspace.saveAll' : 'workbench.workspace.discard') }));
  await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
  expect(right.getByRole('switch', { name: 'Topic: Automatic' })).toBeChecked();
  expect(right.getByLabelText('Open drafts')).toHaveTextContent('1');
  expect(useWorkbench.getState().unsaved.has(b)).toBe(true);
  if (action === 'save') expect(putSetting).toHaveBeenCalledExactlyOnceWith(target, targets.key, { value: { thread: { automatic: true } }, expectedRevision: null });
  else expect(putSetting).not.toHaveBeenCalled();
});

it('keeps an object draft through another row\'s switch and the Only-set-here filter, and writes it whole once by Save', async () => {
  const user = userEvent.setup();
  draw();
  const keepAwake = await screen.findByRole('switch', { name: flag.title });
  await user.click(editorOf(targets.title));
  const save = screen.getByRole('button', { name: i18n.t('common.save') });
  expect(save).toBeDisabled();
  await user.click(screen.getByRole('switch', { name: 'Topic: Automatic' }));
  expect(save).toBeEnabled();
  expect(screen.getByLabelText('Open drafts')).toHaveTextContent('1');
  // Pressing another row's switch writes that row, and only that row.
  await user.click(keepAwake);
  await waitFor(() => expect(putSetting).toHaveBeenCalledExactlyOnceWith(target, flag.key, { value: true, expectedRevision: null }));
  expect(screen.getByRole('switch', { name: 'Topic: Automatic' })).toBeChecked();
  expect(screen.getByRole('button', { name: i18n.t('common.save') })).toBeEnabled();
  // The filter keeps what is set here; the row with an unsaved draft stays on the page whatever it says.
  await user.click(screen.getByRole('switch', { name: i18n.t('settings.editor.onlyHere') }));
  expect(screen.getByRole('switch', { name: 'Topic: Automatic' })).toBeChecked();
  expect(putSetting).toHaveBeenCalledTimes(1);
  await user.click(screen.getByRole('button', { name: i18n.t('common.save') }));
  await waitFor(() => expect(putSetting).toHaveBeenCalledTimes(2));
  expect(putSetting).toHaveBeenLastCalledWith(target, targets.key, { value: { topic: { automatic: true } }, expectedRevision: null });
  // Both rows say Saved: the switch that was pressed and the draft that was saved.
  await waitFor(() => expect(screen.getAllByText(i18n.t('settings.compact.saved'))).toHaveLength(2));
  await waitFor(() => expect(screen.getByLabelText('Open drafts')).toHaveTextContent('0'));
});

it('writes nothing when focus leaves a draft, and Discard puts the saved value back', async () => {
  const user = userEvent.setup();
  draw();
  await screen.findByRole('switch', { name: flag.title });
  await user.click(editorOf(targets.title));
  await user.click(screen.getByRole('switch', { name: 'Thread: Automatic' }));
  await user.click(document.body);
  await user.click(screen.getByRole('switch', { name: i18n.t('settings.editor.onlyHere') }));
  expect(putSetting).not.toHaveBeenCalled();
  expect(screen.getByRole('switch', { name: 'Thread: Automatic' })).toBeChecked();
  await user.click(screen.getByRole('button', { name: i18n.t('settingsEditors.draft.discard') }));
  expect(putSetting).not.toHaveBeenCalled();
  expect(screen.getByLabelText('Open drafts')).toHaveTextContent('0');
  // With its draft gone the row is held by nothing and the filter hides it; without the filter it is the saved value again.
  expect(screen.queryByRole('button', { name: i18n.t('settings.compact.editValue', { name: targets.title }) })).not.toBeInTheDocument();
  await user.click(screen.getByRole('switch', { name: i18n.t('settings.editor.onlyHere') }));
  await user.click(editorOf(targets.title));
  expect(screen.getByRole('switch', { name: 'Thread: Automatic' })).not.toBeChecked();
});

it('keeps Save off for a number the schema refuses and says so beside that field', async () => {
  const user = userEvent.setup();
  draw();
  await screen.findByRole('switch', { name: flag.title });
  await user.click(editorOf(targets.title));
  await user.click(screen.getByRole('switch', { name: 'Document: Automatic' }));
  const documentRow = screen.getByRole('group', { name: 'Document: when to refresh' }).closest('li');
  if (documentRow === null) throw new Error('The document target has no row');
  const tokens = within(documentRow).getByRole('spinbutton', { name: i18n.t('settingsEditors.targets.context_tokens') });
  await user.type(tokens, '0');
  expect(within(documentRow).getByText(i18n.t('settingsEditors.targets.least', { least: '1' }))).toBeVisible();
  expect(screen.getByRole('button', { name: i18n.t('common.save') })).toBeDisabled();
  await user.clear(tokens);
  expect(screen.getByRole('button', { name: i18n.t('common.save') })).toBeEnabled();
  expect(putSetting).not.toHaveBeenCalled();
});

it('keeps Save and Discard beside "Unsaved changes" when the editor is closed on an edit, and that Save writes the draft', async () => {
  const user = userEvent.setup();
  draw();
  await screen.findByRole('switch', { name: flag.title });
  await user.click(editorOf(targets.title));
  await user.click(screen.getByRole('switch', { name: 'Thread: Automatic' }));
  // The pencil again closes the editor: what was edited is not lost, and not written either.
  await user.click(editorOf(targets.title));
  expect(putSetting).not.toHaveBeenCalled();
  const unsaved = (await screen.findByText(i18n.t('settings.compact.draft'))).closest<HTMLElement>('[data-unsaved-draft]');
  if (unsaved === null) throw new Error('The unsaved draft has no Save beside its words');
  expect(within(unsaved).getByRole('button', { name: i18n.t('settingsEditors.draft.discard') })).toBeEnabled();
  expect(screen.getByLabelText('Open drafts')).toHaveTextContent('1');
  await user.click(within(unsaved).getByRole('button', { name: i18n.t('common.save') }));
  await waitFor(() => expect(putSetting).toHaveBeenCalledExactlyOnceWith(target, targets.key, { value: { thread: { automatic: true } }, expectedRevision: null }));
  await waitFor(() => expect(screen.queryByText(i18n.t('settings.compact.draft'))).not.toBeInTheDocument());
  await waitFor(() => expect(screen.getByLabelText('Open drafts')).toHaveTextContent('0'));
});

it('lets the Discard beside "Unsaved changes" put the saved value back', async () => {
  const user = userEvent.setup();
  draw();
  await screen.findByRole('switch', { name: flag.title });
  await user.click(editorOf(targets.title));
  await user.click(screen.getByRole('switch', { name: 'Thread: Automatic' }));
  await user.click(editorOf(targets.title));
  const unsaved = (await screen.findByText(i18n.t('settings.compact.draft'))).closest<HTMLElement>('[data-unsaved-draft]');
  if (unsaved === null) throw new Error('The unsaved draft has no Discard beside its words');
  await user.click(within(unsaved).getByRole('button', { name: i18n.t('settingsEditors.draft.discard') }));
  expect(putSetting).not.toHaveBeenCalled();
  expect(screen.queryByText(i18n.t('settings.compact.draft'))).not.toBeInTheDocument();
  await user.click(editorOf(targets.title));
  expect(screen.getByRole('switch', { name: 'Thread: Automatic' })).not.toBeChecked();
});

it('says beside Save why it is off: nothing has changed, or a value that cannot be saved as it stands, in the editor and after it is closed', async () => {
  const user = userEvent.setup();
  draw();
  await screen.findByRole('switch', { name: flag.title });
  await user.click(editorOf(targets.title));
  const save = screen.getByRole('button', { name: i18n.t('common.save') });
  const foot = () => document.querySelector<HTMLElement>('[data-draft-foot]');
  expect(save).toBeDisabled();
  expect(foot()).toHaveTextContent(i18n.t('settingsEditors.draft.nothingToSave'));
  await user.click(screen.getByRole('switch', { name: 'Document: Automatic' }));
  expect(foot()).not.toHaveTextContent(i18n.t('settingsEditors.draft.nothingToSave'));
  const documentRow = screen.getByRole('group', { name: 'Document: when to refresh' }).closest('li');
  if (documentRow === null) throw new Error('The document target has no row');
  await user.type(within(documentRow).getByRole('spinbutton', { name: i18n.t('settingsEditors.targets.context_tokens') }), '0');
  expect(save).toBeDisabled();
  // The reason is the browser's own check of the number or the editor's mark at its field: either way it is said beside Save.
  const reasons = [i18n.t('settings.compact.invalidValue'), i18n.t('settingsEditors.draft.fixMarked')];
  expect(reasons.some((reason) => foot()?.textContent.includes(reason) === true)).toBe(true);
  await user.click(editorOf(targets.title));
  const unsaved = (await screen.findByText(i18n.t('settings.compact.draft'))).closest<HTMLElement>('[data-unsaved-draft]');
  if (unsaved === null) throw new Error('The unsaved draft has no Save beside its words');
  expect(within(unsaved).getByRole('button', { name: i18n.t('common.save') })).toBeDisabled();
  expect(reasons.some((reason) => unsaved.textContent.includes(reason))).toBe(true);
});


it('keeps shared automation defaults when a registry provenance part is served and only Automatic is changed', async () => {
  const user = userEvent.setup();
  server.values.mockResolvedValue([
    row(flag, false),
    { ...row(targets, SHIPPED), parts: [{ source: { scope: 'default', node: null }, value: SHIPPED, locked: false, ignored: false }] },
    { ...row(targets, { min_interval_secs: 600, on: ['interval'] }), key: 'auto.triggers', source: here },
    { ...row(targets, { kind: 'model', context_tokens: 64_000 }), key: 'auto.summarizer', source: here },
  ]);
  draw();
  await user.click(await screen.findByRole('button', { name: i18n.t('settings.compact.editValue', { name: targets.title }) }));
  await user.click(screen.getByRole('switch', { name: 'Thread: Automatic' }));
  const thread = document.querySelector<HTMLElement>('[data-summary-target="thread"]');
  if (thread === null) throw new Error('The thread target must have a row.');
  const controls = within(thread);
  const interval = controls.getByRole('spinbutton', { name: i18n.t('settingsEditors.targets.min_interval_secs') });
  expect(interval).toHaveValue(null);
  expect(interval).toHaveAttribute('placeholder', '600');
  expect(controls.getByRole('spinbutton', { name: i18n.t('settingsEditors.targets.context_tokens') })).toHaveAttribute('placeholder', '64,000');
  expect(controls.getByRole('button', { name: 'Interval' })).toHaveAttribute('aria-pressed', 'true');
  await user.type(interval, '42');
  await user.clear(interval);
  expect(interval).toHaveAttribute('placeholder', '600');
  await user.click(screen.getByRole('button', { name: i18n.t('common.save') }));
  await waitFor(() => expect(putSetting).toHaveBeenCalledExactlyOnceWith(target, targets.key, { value: { thread: { automatic: true } }, expectedRevision: null }));
});


it('keeps an edited view protected when its write permission changes until its draft is discarded', async () => {
  const user = userEvent.setup();
  const id = useWorkbench.getState().openTab('page', { href: '/topic/topic?tab=settings' });
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity }, mutations: { retry: false } } });
  show(<><SettingsView id={id} /><CloseViews /></>, cache);
  await user.click(await screen.findByRole('button', { name: i18n.t('settings.compact.editValue', { name: targets.title }) }));
  await user.click(screen.getByRole('switch', { name: 'Thread: Automatic' }));
  act(() => { cache.setQueryData<ResolvedSetting[]>(targetSettingsKey(target), (rows) => rows?.map((entry) => entry.key === targets.key ? { ...entry, writable: false } : entry)); });
  await waitFor(() => expect(screen.queryByRole('switch', { name: 'Thread: Automatic' })).not.toBeInTheDocument());
  expect(useWorkbench.getState().unsaved.has(id)).toBe(true);
  act(() => useWorkbench.getState().close({ kind: 'tab', tab: id }));
  const question = await screen.findByRole('dialog');
  await user.click(within(question).getByRole('button', { name: i18n.t('workbench.workspace.discard') }));
  await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
  expect(putSetting).not.toHaveBeenCalled();
  expect(allTabs(useWorkbench.getState().layout).some((tab) => tab.id === id)).toBe(false);
});
