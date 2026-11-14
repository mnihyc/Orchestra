/** The actual thread keeps its one composer for viewers and uses the existing native request form. */
import { act, cleanup, fireEvent, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ReactNode } from 'react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type * as AccessApi from '@/api/access';
import type * as AuthApi from '@/api/auth';
import type * as SettingsApi from '@/api/settings';
import type * as ThreadsApi from '@/api/threads';
import { client } from '@/api/client';
import { captureThreadRead, selectThreadContinuation } from '@/api/threadRead';
import type * as Draft from '@/features/composer/Draft';
import { useComposerStore } from '@/features/composer/Draft';
import { ThreadScreen } from '@/features/thread/Thread';
import { ViewCompactContext } from '@/features/workbench/Narrow';
import type { MissingAccess, Thread, ThreadLimits, ThreadParameters } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import type * as Live from '@/realtime/live';
import { threadWriter } from '@/realtime/stores/thread';
import { defaultThreadRead } from '@/realtime/threadScope';
import type * as Screen from '@/shell/screen';
import { ACME, meta } from '../doubles/fixtures';
import { World } from '../doubles/world';
import { CATALOGUE } from '../unit/general-access-fixtures';
import { A, B, projection, snapshot } from '../unit/continuation-fixtures';
import { show } from './support';

const calls = vi.hoisted(() => ({ request: vi.fn<typeof AccessApi.requestAccess>(), reload: vi.fn(), submit: vi.fn<typeof Draft.submitInput>(), draft: vi.fn<typeof Draft.useDraftSync>() }));
vi.mock('@/features/composer/Draft', async original => ({ ...await original<typeof Draft>(), submitInput: calls.submit, useDraftSync: (...args: Parameters<typeof Draft.useDraftSync>) => calls.draft(...args) }));
vi.mock('@/realtime/live', async original => ({ ...await original<typeof Live>(), useThreadFeed: () => undefined, useOrgTreeFeed: () => undefined, useSocketStatus: () => ({ phase: 'ready' }), reloadThread: calls.reload }));
vi.mock('@/shell/screen', async original => ({ ...await original<typeof Screen>(), useScreenHeading: () => undefined }));
vi.mock('@/shell/org', () => ({ selectOrg: () => undefined }));
vi.mock('@/features/inbox/Counters', () => ({ useReadWhenOpened: () => undefined }));
vi.mock('@/features/thread/Header', () => ({ ThreadHeader: ({ thread }: { thread: Thread }) => <header><h1>{thread.name}</h1></header> }));
vi.mock('@/features/thread/ConnectionBanner', () => ({ ConnectionBanner: () => null }));
vi.mock('@/features/schedules/Schedules', () => ({ PendingAgentSchedules: () => null }));
vi.mock('@/features/thread/BlockedBanner', () => ({ BlockedBanner: () => null }));
vi.mock('@/features/thread/LockBanner', () => ({ LockBanner: () => null }));
vi.mock('@/features/thread/ContinuationNotice', () => ({ ContinuationNotice: () => null }));
vi.mock('@/features/source/Preparation', () => ({ SourcePreparation: () => null }));
vi.mock('@/features/source/Review', () => ({ ReviewTray: () => null }));
vi.mock('@/features/gateways/ThreadGatewayStatus', () => ({ ThreadGatewayStatus: () => null }));
vi.mock('@/features/timeline/FindInConversation', () => ({ FindInConversation: () => null }));
vi.mock('@/features/timeline/Timeline', () => ({ Timeline: () => <p>The conversation stays here.</p> }));
vi.mock('@/features/ports/LocalAddresses', () => ({ LocalAddresses: ({ children }: { children: ReactNode }) => children }));
vi.mock('@/features/usage/context/ContextRing', () => ({ ContextRing: () => <span>Context</span> }));
vi.mock('@/api/auth', async original => ({ ...await original<typeof AuthApi>(), metaQuery: { queryKey: ['viewer-meta'], queryFn: () => Promise.resolve(meta()) } }));
vi.mock('@/api/settings', async original => ({ ...await original<typeof SettingsApi>(), settingsQuery: (id: string) => ({ queryKey: ['viewer-settings', id], queryFn: () => Promise.resolve([]), staleTime: Infinity }) }));
vi.mock('@/api/threads', async original => ({ ...await original<typeof ThreadsApi>(),
  parametersQuery: (id: string) => ({ queryKey: ['viewer-parameters', id], queryFn: () => Promise.resolve(parameters), staleTime: Infinity }),
  limitsQuery: (id: string) => ({ queryKey: ['viewer-limits', id], queryFn: () => Promise.resolve(limits), staleTime: Infinity }),
}));
vi.mock('@/api/access', async original => ({ ...await original<typeof AccessApi>(),
  permissionsQuery: { queryKey: ['viewer-permissions'], queryFn: () => Promise.resolve(CATALOGUE.reply), staleTime: Infinity },
  requestAccess: (...args: Parameters<typeof AccessApi.requestAccess>) => calls.request(...args),
  accessRequestQuery: (id: string) => ({ queryKey: ['viewer-request', id], queryFn: () => Promise.resolve({ batch_id: id, approvals: [], complete: false }) }),
}));

const MODEL = 'vendor/internal-model-route-2026';
const parameters: ThreadParameters = { approval_mode: 'on_request', approval_reviewer: null, approval_modes: ['ask', 'on_request'],
  model: { value: MODEL, writable: false, options: [], source: { scope: 'thread', node: null } },
  effort: { value: 'high', writable: false, supported: true, source: { scope: 'thread', node: null } }, parameters: [] };
const limits: ThreadLimits = { reached: [], bypass_all: false };
const modes = ([400, 1440] as const).flatMap(width => (['light', 'dark'] as const).map(theme => ({ width, theme })));
let native: Thread;
let session = client.session.getState();
let appearance = useAppearance.getState();
let width: PropertyDescriptor | undefined;
let theme: string | null = null;

beforeEach(async () => {
  sessionStorage.clear();
  session = client.session.getState(); appearance = useAppearance.getState();
  width = Object.getOwnPropertyDescriptor(window, 'innerWidth'); theme = document.documentElement.getAttribute('data-theme');
  await setLocale('en');
  const world = new World(), person = world.people.get('ada'), reader = world.people.get('vera'), row = world.places.rowOf(ACME.mainThread);
  if (person === undefined || reader === undefined || row === undefined) throw new Error('A complete native viewer fixture is required');
  const original = world.threads.thread(person, row);
  native = { ...original, seq: 10, model: MODEL, model_display: 'Reasoner', effort: 'high',
    access: { ...original.access, permissions: ['thread.view', 'thread.manage'] }, state: 'idle', current_turn_id: null,
    can: { ...original.can, archive: false }, input_access: { allowed: false, missing: [] } };
  native.input_access.missing = missingAccess(native);
  client.session.setState({ status: 'signed_in', userId: reader.me.id });
  useComposerStore.setState({ text: {}, parts: {}, sending: {}, refused: {}, cannotSteer: {}, passNext: {}, planning: {} });
  calls.request.mockReset().mockResolvedValue({ batch_id: 'viewer-request', approvals: [] });
  calls.reload.mockReset(); calls.submit.mockReset(); calls.draft.mockReset().mockReturnValue('idle');
});
afterEach(async () => {
  cleanup(); threadWriter.evict(native.id);
  client.session.setState(session); useAppearance.setState(appearance);
  useComposerStore.setState({ text: {}, parts: {}, sending: {}, refused: {}, cannotSteer: {}, passNext: {}, planning: {} });
  if (width === undefined) Reflect.deleteProperty(window, 'innerWidth'); else Object.defineProperty(window, 'innerWidth', width);
  if (theme === null) document.documentElement.removeAttribute('data-theme'); else document.documentElement.setAttribute('data-theme', theme);
  await setLocale('en');
});

function missingAccess(thread: Thread): MissingAccess[] {
  if (thread.binding === null) throw new Error('The viewer fixture must expose its native binding');
  return [
    { object: { id: thread.id, kind: 'thread', name: thread.name, owner: thread.owner }, permissions: ['thread.interact'], options: null,
      ask: [thread.owner], ask_more: 0, override: false, closed: null },
    { object: { id: thread.binding.provider.id, kind: 'provider', name: thread.binding.provider.name, owner: thread.owner }, permissions: ['provider.use'],
      options: { models: [MODEL], efforts: ['high'] }, ask: [thread.owner], ask_more: 0, override: false, closed: null },
  ];
}

function draw(mode = modes[0], thread = native, historical = false) {
  if (mode === undefined) throw new Error('The viewer fixture needs a presentation');
  Object.defineProperty(window, 'innerWidth', { configurable: true, value: mode.width });
  document.documentElement.dataset.theme = mode.theme;
  useAppearance.setState({ theme: mode.theme, resolvedTheme: mode.theme });
  const selected = historical ? B : null;
  selectThreadContinuation(thread.id, selected);
  const frame = { ...snapshot(projection(historical ? B : A, A, 1, !historical)), thread };
  expect(threadWriter.reset(thread.id, frame, false, captureThreadRead(thread.id))).not.toBeNull();
  return show(<ViewCompactContext.Provider value={mode.width === 400}><div style={{ width: mode.width }}><ThreadScreen threadId={thread.id} item={null} continuation={selected} /></div></ViewCompactContext.Provider>);
}

describe.each(modes)('a live viewer at $width px in $theme', mode => {
  it('keeps the composer, disabled attachments and Send, and the actual model and effort', async () => {
    const user = userEvent.setup(), view = draw(mode, { ...native, state: 'running' });
    const box = screen.getByRole('textbox', { name: i18n.t('composer.label') });
    expect(box).toBeVisible(); expect(box).toBeDisabled(); expect(box).toHaveAttribute('placeholder', 'You can watch this thread');
    expect(view.container.querySelectorAll('[data-composer]')).toHaveLength(1);
    expect(view.container.querySelector('[data-watching]')).toBeNull();
    const controls = within(screen.getByRole('group', { name: i18n.t('composer.row.label') }));
    expect(controls.getByRole('button', { name: i18n.t('composer.plus.label') })).toBeDisabled();
    expect(view.container.querySelector('input[type="file"]')).toBeDisabled();
    const model = controls.getByRole('button', { name: i18n.t('composer.chip.value', { what: i18n.t('composer.model.chip'), value: 'Reasoner' }) });
    expect(model).toBeDisabled(); expect(model).toHaveTextContent('Reasoner');
    const effort = controls.getByRole('button', { name: i18n.t('composer.chip.value', { what: i18n.t('composer.effort.title'), value: 'High' }) });
    expect(effort).toBeDisabled(); expect(effort).toHaveTextContent('High');
    const send = controls.getByRole('button', { name: i18n.t('composer.send') });
    expect(send).toBeVisible(); expect(send).toBeDisabled();
    expect(controls.getByRole('button', { name: i18n.t('composer.deliveryMenu') })).toBeDisabled();
    expect(controls.queryByRole('button', { name: i18n.t('composer.stop') })).not.toBeInTheDocument();
    const line = controls.getByRole('group', { name: i18n.t('thread.watching.label') });
    expect(line).toHaveTextContent(i18n.t('thread.watching.talk', { name: native.name }));
    expect(line).toHaveTextContent(i18n.t('thread.watching.useAt', { name: native.binding?.provider.name, model: 'Reasoner', effort: 'High' }));
    expect(line).toHaveTextContent(i18n.t('thread.watching.askWho', { names: native.owner.display_name }));
    expect(view.container).not.toHaveTextContent('vendor/internal-model-route-2026');
    await user.hover(send);
    expect(await screen.findByRole('tooltip')).toHaveTextContent(i18n.t('composer.row.readOnly'));
    fireEvent.change(box, { target: { value: 'Cannot send this' } }); fireEvent.keyDown(box, { key: 'Enter' });
    expect(useComposerStore.getState().text[native.id]).toBeUndefined();
    expect(calls.submit).not.toHaveBeenCalled(); expect(calls.request).not.toHaveBeenCalled();
    expect(calls.draft).toHaveBeenCalled();
    expect(calls.draft.mock.calls.every(([, enabled]) => !enabled)).toBe(true);
  });

  it('opens the shared request form addressed to its native giver and submits the exact missing options', async () => {
    const user = userEvent.setup(); draw(mode);
    await user.click(screen.getByRole('button', { name: i18n.t('access.request.title') }));
    const dialog = screen.getByRole('dialog', { name: i18n.t('access.request.title') });
    expect(within(dialog).getByText(i18n.t('access.request.goesTo', { names: native.owner.display_name }))).toBeVisible();
    expect(within(dialog).queryByRole('radiogroup', { name: i18n.t('access.request.choose') })).not.toBeInTheDocument();
    await user.type(within(dialog).getByRole('textbox', { name: i18n.t('access.request.reason') }), 'Please let me work here.');
    await user.click(within(dialog).getByRole('button', { name: i18n.t('access.request.send') }));
    await waitFor(() => expect(calls.request).toHaveBeenCalledOnce());
    expect(calls.request.mock.calls[0]?.[0]).toEqual({
      items: native.input_access.missing.map(entry => ({ object: { kind: entry.object.kind, id: entry.object.id }, permissions: entry.permissions,
        ...(entry.options === null ? {} : { options: entry.options }) })),
      reason: 'Please let me work here.', for_thread: native.id,
    });
    expect(calls.reload).toHaveBeenCalledExactlyOnceWith(defaultThreadRead(native.id));
  });

  it.each(['nobody', 'person'] as const)('keeps the disabled composer and no empty request action when %s can give access', kind => {
    const environment = native.environment;
    if (kind === 'person' && environment === null) throw new Error('The connected-machine fixture needs its native environment');
    const missing = native.input_access.missing.slice(0, 1).map(entry => ({ ...entry, ask: [], ask_more: 0 }));
    const thread: Thread = { ...native, ...(environment === null ? {} : { environment: { ...environment, placement: kind === 'person' ? 'connected' : 'managed' } }), input_access: { allowed: false, missing } };
    draw(mode, thread);
    expect(screen.getByRole('textbox', { name: i18n.t('composer.label') })).toBeDisabled();
    const line = within(screen.getByRole('group', { name: i18n.t('thread.watching.label') }));
    expect(line.queryByText(/Reasoner/)).not.toBeInTheDocument();
    expect(line.getByText(kind === 'person' ? i18n.t('thread.watching.person', { owner: native.owner.display_name, thread: native.name }) : i18n.t('thread.watching.nobody'))).toBeVisible();
    expect(line.queryByRole('button')).not.toBeInTheDocument();
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument(); expect(calls.request).not.toHaveBeenCalled();
  });

  it('keeps the same text area when Interact access arrives and removes the request line', () => {
    draw(mode);
    const box = screen.getByRole('textbox', { name: i18n.t('composer.label') });
    const granted: Thread = { ...native, seq: native.seq + 1, access: { ...native.access, permissions: ['thread.view', 'thread.interact'] }, input_access: { allowed: true, missing: [] } };
    act(() => threadWriter.apply(native.id, { thread: granted, projection: projection() }));
    expect(screen.getByRole('textbox', { name: i18n.t('composer.label') })).toBe(box);
    expect(box).toBeEnabled();
    expect(screen.queryByRole('group', { name: i18n.t('thread.watching.label') })).not.toBeInTheDocument();
  });

  it('keeps an interactor on the native send path when use of the model is missing', async () => {
    const user = userEvent.setup();
    const thread: Thread = { ...native, access: { ...native.access, permissions: ['thread.view', 'thread.interact'] },
      input_access: { allowed: false, missing: native.input_access.missing.filter(entry => entry.object.kind === 'provider') } };
    draw(mode, thread);
    expect(screen.queryByRole('group', { name: i18n.t('thread.watching.label') })).not.toBeInTheDocument();
    const box = screen.getByRole('textbox', { name: i18n.t('composer.label') });
    expect(box).toBeEnabled();
    await user.type(box, 'Hold this input for model access.');
    await user.click(screen.getByRole('button', { name: i18n.t('composer.send') }));
    expect(calls.submit).toHaveBeenCalledExactlyOnceWith(defaultThreadRead(thread.id), [{ type: 'text', text: 'Hold this input for model access.' }], 'queue', undefined, undefined, false);
    expect(calls.request).not.toHaveBeenCalled();
  });
});

it.each(['archived', 'inspection'] as const)('retains the existing %s exclusion instead of showing a viewer composer', state => {
  const view = draw(modes[0], { ...native, archived: state === 'archived', archived_by: null }, state === 'inspection');
  expect(view.container.querySelector('[data-composer]')).toBeNull();
  expect(screen.queryByRole('group', { name: i18n.t('thread.watching.label') })).not.toBeInTheDocument();
  if (state === 'archived') expect(screen.getByText(i18n.t('thread.archived.lineAnonymous'))).toBeVisible();
});

it.each(['en', 'zh-CN'] as const)('shows the disabled placeholder, model, effort and access action in %s', async locale => {
  await setLocale(locale); draw();
  expect(screen.getByRole('textbox', { name: i18n.t('composer.label') })).toHaveAttribute('placeholder', i18n.t('thread.watching.title'));
  const high = i18n.t('choiceCatalog.effort.high.name');
  const model = screen.getByRole('button', { name: i18n.t('composer.chip.value', { what: i18n.t('composer.model.chip'), value: 'Reasoner' }) });
  expect(model).toBeDisabled(); expect(model).toHaveTextContent('Reasoner');
  const effort = screen.getByRole('button', { name: i18n.t('composer.chip.value', { what: i18n.t('composer.effort.title'), value: high }) });
  expect(effort).toBeDisabled(); expect(effort).toHaveTextContent(high);
  expect(screen.getByRole('button', { name: i18n.t('access.request.title') })).toBeEnabled();
});

describe.each(modes)('the thread dock at $width px in $theme', mode => {
  it('keeps where the thread runs with its composer controls as the conversation scrolls', () => {
    const view = draw(mode);
    const where = view.container.querySelector('[data-where-chip]');
    expect(where).not.toBeNull();
    expect(view.container.querySelectorAll('[data-where-chip]')).toHaveLength(1);
    expect(view.container.querySelector('[data-composer-controls]')).toContainElement(where as HTMLElement);
    expect(view.container.querySelector('[data-thread-dock]')).toContainElement(where as HTMLElement);
    expect(view.container.querySelector('[data-scroll-area]')).not.toContainElement(where as HTMLElement);
    expect(view.container.querySelector('header')).not.toContainElement(where as HTMLElement);
    expect(where).toHaveTextContent(native.environment?.name ?? '');
    if (native.directory !== '') expect(where).toHaveTextContent(native.directory);
    expect(view.container.querySelector('[data-runs-line]')).toBeNull();
  });

  it.each(['archived', 'inspection'] as const)('keeps one read-only set of run facts and location in the %s dock', state => {
    const view = draw(mode, { ...native, archived: state === 'archived', archived_by: null }, state === 'inspection');
    expect(view.container.querySelector('[data-composer]')).toBeNull();
    const runs = screen.getByRole('group', { name: i18n.t('composer.row.runsWith') });
    expect(view.container.querySelectorAll('[data-runs-line]')).toHaveLength(1);
    expect(view.container.querySelector('[data-thread-dock]')).toContainElement(runs);
    expect(view.container.querySelector('[data-scroll-area]')).not.toContainElement(runs);
    expect(runs.querySelector('[data-readonly-chip="agent"]')).toHaveTextContent(native.binding?.agent.name ?? '');
    expect(runs.querySelector('[data-readonly-chip="model"]')).toHaveTextContent('Reasoner');
    expect(runs.querySelector('[data-readonly-chip="effort"]')).toHaveTextContent('High');
    const where = runs.querySelector('[data-where-chip]');
    expect(where).not.toBeNull();
    expect(view.container.querySelectorAll('[data-where-chip]')).toHaveLength(1);
    expect(where).toHaveAttribute('aria-disabled', 'true');
    expect(where).toHaveTextContent(native.environment?.name ?? '');
    expect(where).toHaveAttribute('aria-description', expect.stringContaining(native.environment?.name ?? ''));
    if (native.directory !== '') expect(where).toHaveAttribute('aria-description', expect.stringContaining(native.directory));
    expect(where).toHaveAttribute('aria-description', expect.stringContaining(i18n.t('composer.row.readOnly')));
  });
});
