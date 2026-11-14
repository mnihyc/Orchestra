/** The running composer's hint matches its actual native delivery keys, independent of the place's preferred delivery. */
import { act, fireEvent, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type * as ThreadsApi from '@/api/threads';
import type * as SettingsApi from '@/api/settings';
import type * as AuthApi from '@/api/auth';
import type * as ContextApi from '@/api/context';
import type * as Uploads from '@/lib/uploads';
import type * as Live from '@/realtime/live';
import type * as Draft from '@/features/composer/Draft';
import { Composer } from '@/features/composer/Composer';
import { composerWriter, useComposerStore } from '@/features/composer/Draft';
import { client } from '@/api/client';
import type { ResolvedSetting, Thread, ThreadContext, ThreadParameters, ThreadLimits, LimitReached } from '@/generated/api-types';
import type { PastedTextPart } from '@/lib/pastedText';
import { rememberPaste } from '@/lib/pastedText';
import type { CodeViewProps } from '@/ui/CodeView';
import { currentLocale, i18n, setLocale } from '@/i18n';
import { isApplePlatform } from '@/lib/shortcuts';
import { defaultThreadRead } from '@/realtime/threadScope';
import { threadWriter } from '@/realtime/stores/thread';
import { ACME, meta } from '../doubles/fixtures';
import { turn } from '../doubles/thread-rows';
import { harnessName } from '../doubles/harness-names';
import { World } from '../doubles/world';
import { choiceCap } from '../unit/resource-facts-fixtures';
import { show } from './support';

const server = vi.hoisted(() => {
  const limits: ThreadLimits = { reached: [], bypass_all: false };
  return { settingsRead: vi.fn<() => Promise<ResolvedSetting[]>>(), submit: vi.fn<typeof Draft.submitInput>(), context: vi.fn<() => Promise<ThreadContext>>(), cancelUpload: vi.fn<() => Promise<void>>(), live: true, limits };
});
vi.mock('@/features/composer/Draft', async (load) => ({ ...await load<typeof Draft>(), useDraftSync: () => 'idle', submitInput: server.submit }));
vi.mock('@/realtime/live', async (load) => ({ ...await load<typeof Live>(), useSocketStatus: () => ({ phase: server.live ? 'ready' : 'reconnecting' }) }));
vi.mock('@/api/threads', async (load) => ({ ...await load<typeof ThreadsApi>(),
  parametersQuery: (id: string) => ({ queryKey: ['keyboard-parameters', id], queryFn: () => Promise.resolve(parameters) }),
  limitsQuery: (id: string) => ({ queryKey: ['keyboard-limits', id], initialData: server.limits, queryFn: () => Promise.resolve(server.limits) }),
}));
vi.mock('@/api/settings', async (load) => ({ ...await load<typeof SettingsApi>(), settingsQuery: (id: string) => ({ queryKey: ['keyboard-settings', id], queryFn: server.settingsRead }) }));
vi.mock('@/api/auth', async (load) => ({ ...await load<typeof AuthApi>(), metaQuery: { queryKey: ['keyboard-meta'], queryFn: () => Promise.resolve(meta()) } }));
vi.mock('@/api/context', async (load) => ({ ...await load<typeof ContextApi>(), threadContextQuery: (id: string, detail: 'kept' | 'count', revision: number) => ({ queryKey: ['keyboard-context', id, detail, revision], queryFn: () => server.context() }) }));
vi.mock('@/features/composer/ModelEffort', () => ({ ModelEffort: () => <button>Model</button> }));
vi.mock('@/features/composer/ApprovalMode', () => ({ ApprovalModePicker: () => <button>Mode</button>, ConfinementPicker: () => <button>Sandbox</button> }));
vi.mock('@/features/usage/context/ContextRing', () => ({ ContextRing: () => <button>Context</button> }));
vi.mock('@/ui/CodeView', () => ({ CodeView: (props: CodeViewProps) => <textarea aria-label={props.label} defaultValue={props.text} readOnly={props.readOnly} onChange={(event) => { props.onEdit?.(true); props.onSettled?.(event.target.value); }} /> }));
vi.mock('@/lib/uploads', async (load) => ({ ...await load<typeof Uploads>(), releaseUpload: () => Promise.resolve(), UploadTask: class {
  readonly uploadId = null;
  constructor(readonly file: File) {}
  run(signal: AbortSignal, progress: (received: number) => void) { progress(0); return new Promise((_resolve, reject) => { signal.addEventListener('abort', () => reject(new DOMException('Upload cancelled', 'AbortError')), { once: true }); }); }
  cancel() { return server.cancelUpload(); }
} }));

const parameters: ThreadParameters = { approval_mode: 'on_request', approval_reviewer: null, approval_modes: ['ask', 'on_request', 'bypass'],
  model: { value: null, writable: true, options: [], source: { scope: 'default', node: null } },
  effort: { value: null, writable: true, supported: false, source: { scope: 'default', node: null } }, parameters: [] };
const settings: ResolvedSetting[] = [{ key: 'collab.default_delivery', value: 'interrupt', own: null, source: { scope: 'default', node: null }, locked_by: null, writable: true, parts: [] }];
let native: Thread;
let locale = currentLocale();
let theme: string | null = null;
let signedIn: string | null = null;
beforeEach(() => {
  locale = currentLocale(); theme = document.documentElement.getAttribute('data-theme'); signedIn = client.session.getState().userId;
  server.settingsRead.mockReset().mockImplementation(() => Promise.resolve(settings));
  delete parameters.composer;
  server.submit.mockReset(); server.live = true; server.limits = { reached: [], bypass_all: false };
  server.cancelUpload.mockReset().mockResolvedValue(undefined);
  server.context.mockReset(); server.context.mockResolvedValue({ harness_kind: null, harness: null, reports: { tokens: false, context: false, breakdown: false }, fill: null, turn_id: null, shortened: null, can_count: false });
  parameters.model.options = [];
  const world = new World(); const person = world.people.get('ada');
  const node = world.places.rowOf(ACME.mainThread);
  if (person === undefined || node === undefined) throw new Error('The keyboard test needs the native thread fixture');
  native = { ...world.threads.thread(person, node), state: 'running', current_turn_id: 'running-key-turn' };
  client.session.setState({ userId: person.me.id });
  useComposerStore.setState({ text: {}, parts: {}, sending: {}, refused: {}, cannotSteer: {}, passNext: {}, planning: {} });
  threadWriter.reset(native.id, { thread: native, turns: [turn('running-key-turn', { ordinal: 1, state: 'running', inputId: 'earlier' })] }, false);
});
afterEach(async () => {
  threadWriter.evict(native.id); client.session.setState({ userId: signedIn });
  useComposerStore.setState({ text: {}, parts: {}, sending: {}, refused: {}, cannotSteer: {}, passNext: {}, planning: {} });
  await setLocale(locale); if (theme === null) document.documentElement.removeAttribute('data-theme'); else document.documentElement.setAttribute('data-theme', theme);
});

it('waits for the pasted delivery hint before sending and uses the selected context fill', async () => {
  const modelEstimate = { token_estimate: { ascii_prose_x100: 400, ascii_structured_x100: 300, other_x100: 100, image_tokens: 1000, measured: false } };
  parameters.model.options = [{ model_name: 'fake-1', display_name: 'Example', parameters: {}, allowed: true, reason: null, ...modelEstimate }];
  const reply: { resolve?: (context: ThreadContext) => void } = {};
  server.context.mockImplementation(() => new Promise((resolve) => { reply.resolve = resolve; }));
  const now = new Date().toISOString();
  native = { ...native, state: 'idle', current_turn_id: null, model: 'fake-1', usage: { input_tokens: 0, cached_input_tokens: 0, output_tokens: 0, reasoning_tokens: 0, total_tokens: 0, context_used: 100, context_window: 1000, compact_at: null, updated_at: now } };
  threadWriter.reset(native.id, { thread: native, turns: [] }, false);
  const part: PastedTextPart = { type: 'pasted_text', blob: 'f'.repeat(64), name: 'pasted.txt', path: '.orch/attachments/pasted.txt', characters: 200, ascii_characters: 200, size: 200 };
  useComposerStore.setState({ parts: { [native.id]: [part] } });
  show(<Composer thread={native} />);
  await waitFor(() => expect(server.context).toHaveBeenCalledOnce());
  const box = screen.getByRole('textbox', { name: i18n.t('composer.label') });
  expect(screen.getByRole('button', { name: i18n.t('composer.send') })).toBeDisabled();
  expect(screen.queryByText(/^Sent in the message/)).not.toBeInTheDocument();
  fireEvent.keyDown(box, { key: 'Enter' }); expect(server.submit).not.toHaveBeenCalled();
  await act(async () => {
    reply.resolve?.({ harness_kind: 'codex', harness: harnessName('codex'), reports: { tokens: true, context: true, breakdown: false }, fill: { count: 'orch', used: 901, window: 1000, compact_at: null, scaled: false, at: now, parts: [{ kind: 'other', tokens: 901, count: 'orch', rows: [] }] }, turn_id: null, shortened: null, can_count: false });
    await Promise.resolve();
  });
  await waitFor(() => expect(screen.getByRole('button', { name: i18n.t('composer.send') })).toBeEnabled());
  expect(screen.getByText(/^Sent as a file/)).toBeVisible();
  fireEvent.keyDown(box, { key: 'Enter' });
  expect(server.submit).toHaveBeenCalledExactlyOnceWith(defaultThreadRead(native.id), [part], 'queue', undefined, undefined, false);
});

it('recomputes paste delivery from the selected model’s reported estimate', async () => {
  const estimate = { ascii_prose_x100: 400, ascii_structured_x100: 300, other_x100: 100, image_tokens: 1000, measured: false };
  const first = { token_estimate: estimate }, second = { token_estimate: { ...estimate, ascii_prose_x100: 200 } };
  parameters.model.options = [
    { model_name: 'model-a', display_name: 'A', parameters: {}, allowed: true, reason: null, ...first },
    { model_name: 'model-b', display_name: 'B', parameters: {}, allowed: true, reason: null, ...second },
  ];
  server.context.mockResolvedValue({ harness_kind: null, harness: null, reports: { tokens: true, context: true, breakdown: false }, fill: { count: 'orch', used: 900, window: 1000, compact_at: null, scaled: false, at: '2026-10-26T00:00:00Z', parts: [] }, turn_id: null, shortened: null, can_count: false });
  native = { ...native, state: 'idle', current_turn_id: null, model: 'model-a' };
  threadWriter.reset(native.id, { thread: native, turns: [] }, false);
  const part: PastedTextPart = { type: 'pasted_text', blob: 'e'.repeat(64), name: 'pasted.txt', path: '.orch/attachments/pasted.txt', characters: 100, ascii_characters: 100, size: 100 };
  useComposerStore.setState({ parts: { [native.id]: [part] } });
  const view = show(<Composer thread={native} />);
  expect(await screen.findByText(/^Sent in the message/)).toHaveTextContent('25');
  view.rerender(<Composer thread={{ ...native, model: 'model-b' }} />);
  expect(await screen.findByText(/^Sent as a file/)).toHaveTextContent('50');
  view.rerender(<Composer thread={{ ...native, model: 'missing' }} />);
  expect(screen.queryByText(/^Sent as a file/)).not.toBeInTheDocument();
  expect(screen.queryByText(/^Sent in the message/)).not.toBeInTheDocument();
});

for (const locale of ['en', 'zh-CN'] as const) it.each([true, false])('sends the explicit alternative to an automatic paste choice in '+locale+' (inline: %s)', async (inline) => {
  await setLocale(locale);
  parameters.model.options = [{ model_name: 'fake-1', display_name: 'Example', parameters: {}, allowed: true, reason: null,
    token_estimate: { ascii_prose_x100: 400, ascii_structured_x100: 300, other_x100: 100, image_tokens: 1000, measured: false } }];
  const now = '2026-10-26T00:00:00Z';
  server.context.mockResolvedValue({ harness_kind: null, harness: null, reports: { tokens: true, context: true, breakdown: false }, fill: { count: 'orch', used: inline ? 900 : 960, window: 1000, compact_at: null, scaled: false, at: now, parts: [] }, turn_id: null, shortened: null, can_count: false });
  native = { ...native, state: 'idle', current_turn_id: null, model: 'fake-1' };
  threadWriter.reset(native.id, { thread: native, turns: [] }, false);
  const part: PastedTextPart = { type: 'pasted_text', blob: 'b'.repeat(64), name: 'choice.txt', path: '.orch/attachments/choice.txt', characters: 100, ascii_characters: 100, size: 100 };
  useComposerStore.setState({ parts: { [native.id]: [part] } });
  show(<Composer thread={native} />);
  expect(await screen.findByText(i18n.t('uploads.pasteSuggested'), { exact: true })).toBeVisible();
  expect(screen.getByRole('radio', { name: i18n.t(inline ? 'uploads.inMessage' : 'uploads.asFile') })).toBeChecked();
  await userEvent.click(screen.getByRole('radio', { name: i18n.t(inline ? 'uploads.asFile' : 'uploads.inMessage') }));
  expect(screen.queryByText(i18n.t('uploads.pasteSuggested'), { exact: true })).not.toBeInTheDocument();
  fireEvent.keyDown(screen.getByRole('textbox', { name: i18n.t('composer.label') }), { key: 'Enter' });
  expect(server.submit).toHaveBeenCalledExactlyOnceWith(defaultThreadRead(native.id), [{ ...part, delivery: inline ? 'file' : 'inline' }], 'queue', undefined, undefined, false);
});

for (const language of ['en', 'zh-CN'] as const) for (const delivery of ['inline', 'file'] as const) it(`keeps an explicit ${delivery} choice without model estimates or a completed context read in ${language}`, async () => {
  await setLocale(language);
  server.context.mockImplementation(() => new Promise(() => undefined));
  native = { ...native, state: 'idle', current_turn_id: null, model: 'unreported-model' };
  threadWriter.reset(native.id, { thread: native, turns: [] }, false);
  const part: PastedTextPart = { type: 'pasted_text', blob: 'd'.repeat(64), name: 'chosen.txt', path: '.orch/attachments/chosen.txt', characters: 200, ascii_characters: 200, size: 200, delivery };
  useComposerStore.setState({ parts: { [native.id]: [part] } });
  show(<Composer thread={native} />);
  const label = i18n.t(delivery === 'inline' ? 'uploads.pasteInline' : 'uploads.pasteFile');
  expect(await screen.findByText(label, { exact: true })).toBeVisible();
  expect(screen.getByRole('radio', { name: i18n.t(delivery === 'inline' ? 'uploads.inMessage' : 'uploads.asFile') })).toBeChecked();
  expect(screen.queryByText(/≈/)).not.toBeInTheDocument();
  const destination = `→ ${part.path} · ${i18n.t('uploads.whenSent')}`;
  if (delivery === 'file') expect(screen.getByText(destination, { exact: true })).toBeVisible();
  else expect(screen.queryByText(destination, { exact: true })).not.toBeInTheDocument();
  await waitFor(() => expect(screen.getByRole('button', { name: i18n.t('composer.send') })).toBeEnabled());
  fireEvent.keyDown(screen.getByRole('textbox', { name: i18n.t('composer.label') }), { key: 'Enter' });
  expect(server.submit).toHaveBeenCalledExactlyOnceWith(defaultThreadRead(native.id), [part], 'queue', undefined, undefined, false);
});

it('can drop an edited paste after permission is lost without leaving sending blocked on return', async () => {
  let stopped: () => void = () => undefined;
  const cleanup = new Promise<void>((resolve) => { stopped = resolve; });
  server.cancelUpload.mockReturnValue(cleanup);
  try {
    native = { ...native, state: 'idle', current_turn_id: null };
    threadWriter.reset(native.id, { thread: native, turns: [] }, false);
    const part: PastedTextPart = { type: 'pasted_text', blob: 'e'.repeat(64), name: 'edited.txt', path: '.orch/attachments/edited.txt', characters: 200, ascii_characters: 200, size: 200 };
    rememberPaste(part.blob, 'x'.repeat(200));
    useComposerStore.setState({ parts: { [native.id]: [part] } });
    const view = show(<Composer thread={native} />);
    fireEvent.click(screen.getByRole('button', { name: 'Pasted text' }));
    const editor = await screen.findByRole('textbox', { name: 'Pasted text' });
    fireEvent.change(editor, { target: { value: 'Changed before permission was lost' } });
    view.rerender(<Composer thread={{ ...native, access: { ...native.access, permissions: ['thread.view'] } }} />);
    fireEvent.click(await screen.findByRole('button', { name: 'Cancel Pasted text' }));
    expect(useComposerStore.getState().parts[native.id]).toEqual([]);
    await waitFor(() => expect(server.cancelUpload).toHaveBeenCalledOnce());
    view.rerender(<Composer thread={native} />);
    const box = screen.getByRole('textbox', { name: i18n.t('composer.label') });
    fireEvent.change(box, { target: { value: 'A new message' } });
    expect(screen.getByRole('button', { name: i18n.t('composer.send') })).toBeDisabled();
    fireEvent.keyDown(box, { key: 'Enter' });
    expect(server.submit).not.toHaveBeenCalled();
    await act(async () => { stopped(); await cleanup; });
    expect(screen.getByRole('button', { name: i18n.t('composer.send') })).toBeEnabled();
    fireEvent.keyDown(box, { key: 'Enter' });
    expect(server.submit).toHaveBeenCalledExactlyOnceWith(defaultThreadRead(native.id), [{ type: 'text', text: 'A new message' }], 'queue', undefined, undefined, false);
  } finally { stopped(); }
});

describe.each([['en', 'light'], ['en', 'dark'], ['zh-CN', 'light'], ['zh-CN', 'dark']] as const)('running delivery keys %s / %s', (language, look) => {
  beforeEach(async () => { await setLocale(language); document.documentElement.setAttribute('data-theme', look); });
  it.each(['idle', 'running'] as const)('keeps an offline %s draft and explains why Enter cannot send it', (state) => {
    native = { ...native, state };
    if (state === 'idle') threadWriter.reset(native.id, { thread: native, turns: [] }, false);
    server.live = false;
    const view = show(<Composer thread={native} />);
    const box = screen.getByRole('textbox', { name: i18n.t('composer.label') });
    fireEvent.change(box, { target: { value: 'Keep this until the connection returns' } });
    fireEvent.keyDown(box, { key: 'Enter' });
    expect(server.submit).not.toHaveBeenCalled();
    expect(box).toHaveValue('Keep this until the connection returns');
    expect(screen.getByText(i18n.t('composer.reason.offline'), { exact: true })).toBeVisible();
    server.live = true;
    view.rerender(<Composer thread={native} />);
    expect(screen.queryByText(i18n.t('composer.reason.offline'), { exact: true })).not.toBeInTheDocument();
    fireEvent.keyDown(box, { key: 'Enter' });
    expect(server.submit).toHaveBeenCalledExactlyOnceWith(defaultThreadRead(native.id), [{ type: 'text', text: 'Keep this until the connection returns' }], 'queue', undefined, undefined, false);
  });
  it('queues with Enter, steers with Ctrl+Enter and interrupts only with the explicit modified key', async () => {
    show(<Composer thread={native} />);
    const hint = i18n.t(isApplePlatform() ? 'composer.runningHintMac' : 'composer.runningHint');
    expect(screen.getAllByText(hint, { exact: true })).toHaveLength(1);
    const box = screen.getByRole('textbox', { name: i18n.t('composer.label') });
    const user = userEvent.setup();
    await user.click(box); fireEvent.change(box, { target: { value: 'Check the redirect' } });
    fireEvent.compositionStart(box); fireEvent.keyDown(box, { key: 'Enter', isComposing: true }); fireEvent.compositionEnd(box);
    fireEvent.keyDown(box, { key: 'Enter', altKey: true }); fireEvent.keyDown(box, { key: 'Enter', shiftKey: true });
    expect(server.submit).not.toHaveBeenCalled();
    await user.keyboard('{Enter}');
    expect(server.submit).toHaveBeenCalledExactlyOnceWith(defaultThreadRead(native.id), [{ type: 'text', text: 'Check the redirect' }], 'queue', undefined, undefined, false);
    expect(box).toHaveValue('');
    await user.click(box); fireEvent.change(box, { target: { value: 'Keep the return URL' } }); await user.keyboard('{Control>}{Enter}{/Control}');
    expect(server.submit).toHaveBeenNthCalledWith(2, defaultThreadRead(native.id), [{ type: 'text', text: 'Keep the return URL' }], 'steer', undefined, undefined, false);
    await user.click(box); fireEvent.change(box, { target: { value: 'Stop and check auth' } }); await user.keyboard('{Control>}{Shift>}{Enter}{/Shift}{/Control}');
    expect(server.submit).toHaveBeenNthCalledWith(3, defaultThreadRead(native.id), [{ type: 'text', text: 'Stop and check auth' }], 'interrupt', undefined, undefined, false);
    expect(server.submit).toHaveBeenCalledTimes(3);
  });
  it('does not promise unsupported steering and removes the running hint when idle or offline', async () => {
    if (native.binding === null) throw new Error('The fixture must name its agent');
    const view = show(<Composer thread={native} />);
    act(() => composerWriter.noteCannotSteer(native.id, 'running-key-turn'));
    expect(screen.getByText(i18n.t('composer.runningQueueHint'), { exact: true })).toBeVisible();
    const box = screen.getByRole('textbox', { name: i18n.t('composer.label') });
    await userEvent.type(box, 'Still here{Control>}{Enter}{/Control}');
    expect(server.submit).not.toHaveBeenCalled(); expect(box).toHaveValue('Still here');
    server.live = false; view.rerender(<Composer thread={native} />);
    expect(document.querySelector('[data-delivery-hint]')).toBeNull();
    server.live = true;
    act(() => threadWriter.apply(native.id, { turns: [{ ...turn('running-key-turn', { ordinal: 1, state: 'completed', inputId: 'earlier' }), seq_updated: native.seq + 1 }] }));
    view.rerender(<Composer thread={{ ...native, state: 'idle' }} />);
    await waitFor(() => expect(document.querySelector('[data-delivery-hint]')).toBeNull());
  });
});

it('offers Plan only where the agent program plans, and sends with Plan on as a plan', async () => {
  if (native.binding === null) throw new Error('The fixture must name its agent');
  const view = show(<Composer thread={{ ...native, state: 'idle', binding: { ...native.binding, plan_mode: false } }} />);
  expect(screen.queryByRole('button', { name: i18n.t('composer.plan.label') })).toBeNull();
  const planning = { ...native, state: 'idle' as const, binding: { ...native.binding, plan_mode: true } };
  view.rerender(<Composer thread={planning} />);
  const plan = screen.getByRole('button', { name: i18n.t('composer.plan.label') });
  expect(plan).toHaveAttribute('aria-pressed', 'false');
  await userEvent.click(plan);
  expect(plan).toHaveAttribute('aria-pressed', 'true');
  const box = screen.getByRole('textbox', { name: i18n.t('composer.label') });
  await userEvent.type(box, 'Plan the redirect fix{Enter}');
  expect(server.submit).toHaveBeenCalledExactlyOnceWith(defaultThreadRead(native.id), [{ type: 'text', text: 'Plan the redirect fix' }], 'queue', undefined, undefined, true);
  // It stays on until the person turns it off, as the programs' own plan modes do.
  expect(plan).toHaveAttribute('aria-pressed', 'true');
});

it.each(['en', 'zh-CN'] as const)('forgets a learned steering refusal when the next turn starts in %s', async (locale) => {
  await setLocale(locale);
  composerWriter.noteCannotSteer(native.id, 'running-key-turn');
  show(<Composer thread={native} />);
  expect(screen.getByText(i18n.t('composer.runningQueueHint'), { exact: true })).toBeVisible();
  native = { ...native, current_turn_id: 'next-key-turn', seq: native.seq + 2 };
  act(() => threadWriter.apply(native.id, { thread: native, turns: [
    { ...turn('running-key-turn', { ordinal: 1, state: 'completed', inputId: 'earlier' }), seq_updated: native.seq + 1 },
    { ...turn('next-key-turn', { ordinal: 2, state: 'running', inputId: 'next' }), seq_updated: native.seq + 2 },
  ] }));
  const hint = i18n.t(isApplePlatform() ? 'composer.runningHintMac' : 'composer.runningHint');
  expect(await screen.findByText(hint, { exact: true })).toBeVisible();
  expect(screen.queryByText(i18n.t('composer.runningQueueHint'), { exact: true })).not.toBeInTheDocument();
  const box = screen.getByRole('textbox', { name: i18n.t('composer.label') });
  fireEvent.change(box, { target: { value: 'Steer this new turn' } }); fireEvent.keyDown(box, { key: 'Enter', ctrlKey: true });
  expect(server.submit).toHaveBeenCalledExactlyOnceWith(defaultThreadRead(native.id), [{ type: 'text', text: 'Steer this new turn' }], 'steer', undefined, undefined, false);
});

it('sends the original image through file delivery when the build has no native image prompts', async () => {
  const binding = native.binding;
  if (binding === null) throw new Error('The composer fixture needs a binding');
  const image = { type: 'image' as const, blob: 'c'.repeat(64), name: 'diagram.png', media_type: 'image/png' as const };
  const named = { harness: { kind: 'codex', name: 'Codex', names: {} } };
  native = { ...native, state: 'idle', current_turn_id: null, binding: { ...binding, agent: { ...binding.agent, ...named }, program: {
    kind: 'codex', version: '0.162.1', build: { kind: 'codex', version: '0.162.1', os: 'linux', arch: 'x86_64', url: 'https://build.test/program', sha256: 'a'.repeat(64), is_default: false, source: 'channel', status: 'failed', check: { at: '2026-10-26T00:00:00Z', machine: null, missing: [], off: ['input_images'], error: null, stage: 'complete', cases: [] } },
  } } };
  threadWriter.reset(native.id, { thread: native, turns: [] }, false);
  useComposerStore.setState({ parts: { [native.id]: [image] } });
  show(<Composer thread={native} />);
  const reason = i18n.t('programVersions.featureOff', { feature: i18n.t('programVersions.features.input_images'), name: 'Codex', version: '0.162.1' });
  expect(await screen.findByText(reason)).toBeVisible();
  expect(screen.getByRole('button', { name: i18n.t('composer.send') })).toBeEnabled();
  expect(screen.getByText(i18n.t('fileMedia.imageByPath'))).toBeVisible();
  expect(screen.queryByRole('button', { name: i18n.t('uploads.changeModel') })).not.toBeInTheDocument();
  fireEvent.keyDown(screen.getByRole('textbox', { name: i18n.t('composer.label') }), { key: 'Enter' });
  expect(server.submit).toHaveBeenCalledExactlyOnceWith(defaultThreadRead(native.id), [image], 'queue', undefined, undefined, false);
});

for (const language of ['en', 'zh-CN'] as const) it(`uses the reported program name for a disabled image (${language})`, async () => {
  await setLocale(language);
  const binding = native.binding;
  if (binding === null) throw new Error('The composer fixture needs a binding');
  const image = { type: 'image' as const, blob: 'c'.repeat(64), name: 'diagram.png', media_type: 'image/png' as const };
  const named = { harness: { kind: 'research-agent', name: 'Research assistant', names: { 'zh-CN': '研究助理' } } };
  native = { ...native, state: 'idle', current_turn_id: null, binding: { ...binding, agent: { ...binding.agent, ...named, harness_kind: 'research-agent' }, program: {
    kind: 'research-agent', version: '1.2.3', build: { kind: 'research-agent', version: '1.2.3', os: 'linux', arch: 'x86_64', url: 'https://build.test/program', sha256: 'a'.repeat(64), is_default: false, source: 'channel', status: 'failed', check: { at: '2026-10-26T00:00:00Z', machine: null, missing: [], off: ['input_images'], error: null, stage: 'complete', cases: [] } },
  } } };
  threadWriter.reset(native.id, { thread: native, turns: [] }, false);
  useComposerStore.setState({ parts: { [native.id]: [image] } });
  show(<Composer thread={native} />);
  const reason = i18n.t('programVersions.featureOff', { feature: i18n.t('programVersions.features.input_images'), name: language === 'zh-CN' ? '研究助理' : 'Research assistant', version: '1.2.3' });
  expect(await screen.findByText(reason)).toBeVisible();
  expect(screen.getByRole('button', { name: i18n.t('composer.send') })).toBeEnabled();
  expect(screen.getByText(i18n.t('fileMedia.imageByPath'))).toBeVisible();
  expect(document.body).not.toHaveTextContent('research-agent');
});


for (const language of ['en', 'zh-CN'] as const) for (const mode of [
  { width: 400, theme: 'light', delivery: 'queue' },
  { width: 400, theme: 'dark', delivery: 'steer' },
  { width: 1440, theme: 'light', delivery: 'interrupt' },
  { width: 1440, theme: 'dark', delivery: 'queue' },
] as const) it(`retains an image for a text-only model when sending ${mode.delivery} in ${language} at ${mode.width}px ${mode.theme}`, async () => {
  await setLocale(language);
  document.documentElement.dataset.theme = mode.theme;
  parameters.model.options = [{ model_name: 'opaque-text-model', display_name: 'Text model', parameters: { input_images: false }, allowed: true, reason: null, token_estimate: { ascii_prose_x100: 400, ascii_structured_x100: 300, other_x100: 100, image_tokens: 1000, measured: false } }];
  native = { ...native, model: 'opaque-text-model', state: mode.delivery === 'queue' ? 'idle' : 'running', current_turn_id: mode.delivery === 'queue' ? null : 'running-key-turn' };
  threadWriter.reset(native.id, { thread: native, turns: mode.delivery === 'queue' ? [] : [turn('running-key-turn', { ordinal: 1, state: 'running', inputId: 'earlier' })] }, false);
  const image = { type: 'image' as const, blob: 'd'.repeat(64), name: 'screenshot.png', media_type: 'image/png' as const };
  useComposerStore.setState({ parts: { [native.id]: [image] } });
  show(<div style={{ width: mode.width }}><Composer thread={native} /></div>);
  expect(await screen.findByText(i18n.t('fileMedia.imageByPath'))).toBeVisible();
  expect(screen.getByText('screenshot.png')).toBeVisible();
  fireEvent.keyDown(screen.getByRole('textbox', { name: i18n.t('composer.label') }), {
    key: 'Enter', ctrlKey: mode.delivery !== 'queue', shiftKey: mode.delivery === 'interrupt',
  });
  expect(server.submit).toHaveBeenCalledExactlyOnceWith(defaultThreadRead(native.id), [image], mode.delivery, undefined, undefined, false);
});

function cachedVendorLimit(): LimitReached {
  if (native.binding === null) throw new Error('Quota controls need the existing provider binding');
  return { cap: null, provider_limit: { provider: native.binding.provider, as_of: '2026-10-25T12:00:00Z', resets_at: null, window: 'weekly' }, can_bypass: false, passed_by: null, ask: [], ask_more: 0 };
}

for (const stalePass of [false, true, 'null_caps'] as const) it(`sends once without a bypass prompt for cached vendor quota (old pass=${stalePass})`, async () => {
  native = { ...native, state: 'idle', current_turn_id: null };
  threadWriter.reset(native.id, { thread: native, turns: [] }, false);
  server.limits = { reached: [cachedVendorLimit()], bypass_all: false };
  if (stalePass) useComposerStore.setState({ passNext: { [native.id]: stalePass === 'null_caps' ? { provider_limit: true, caps: null } : { provider_limit: true } } });
  show(<Composer thread={native} />);
  const user = userEvent.setup();
  const box = screen.getByRole('textbox');
  await user.type(box, 'Use available paid credits');
  await waitFor(() => expect(screen.getByRole('button', { name: i18n.t('composer.send') })).toBeEnabled());
  fireEvent.keyDown(box, { key: 'Enter' });
  expect(server.submit).toHaveBeenCalledExactlyOnceWith(defaultThreadRead(native.id), [{ type: 'text', text: 'Use available paid credits' }], 'queue', undefined, undefined, false);
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  fireEvent.keyDown(box, { key: 'Enter' });
  expect(server.submit).toHaveBeenCalledTimes(1);
});

it('still prompts for a configured provider cap alongside cached vendor quota', async () => {
  native = { ...native, state: 'idle', current_turn_id: null };
  threadWriter.reset(native.id, { thread: native, turns: [] }, false);
  const vendor = cachedVendorLimit();
  const cap = choiceCap({ state: 'reached' });
  cap.cap.provider = vendor.provider_limit!.provider;
  server.limits = { reached: [vendor, { ...vendor, cap, provider_limit: null, can_bypass: true }], bypass_all: false };
  show(<Composer thread={native} />);
  const user = userEvent.setup();
  const box = screen.getByRole('textbox');
  await user.type(box, 'Explicit cap approval');
  await waitFor(() => expect(screen.getByRole('button', { name: i18n.t('composer.send') })).toBeEnabled());
  fireEvent.keyDown(box, { key: 'Enter' });
  expect(server.submit).not.toHaveBeenCalled();
  const dialog = await screen.findByRole('dialog', { name: i18n.t('ui.limit.title') });
  expect(dialog).not.toHaveTextContent('weekly');
  await user.click(screen.getByRole('button', { name: i18n.t('ui.limit.proceed') }));
  expect(server.submit).toHaveBeenCalledExactlyOnceWith(defaultThreadRead(native.id), [{ type: 'text', text: 'Explicit cap approval' }], 'queue', { caps: [cap.cap.id] }, undefined, false);
});

it('marks the first-message Plan default and retires the mark after an explicit choice', async () => {
  if (native.binding === null) throw new Error('The fixture needs its reported program');
  native = { ...native, state: 'idle', current_turn_id: null, usage: null, binding: { ...native.binding, plan_mode: true } };
  threadWriter.reset(native.id, { thread: native, turns: [] }, false);
  settings.push({ key: 'thread.plan_first', value: true, own: null, source: { scope: 'topic', node: null }, locked_by: null, writable: false, parts: [] });
  try {
    show(<Composer thread={native} />);
    const plan = () => screen.getByRole('button', { name: i18n.t('composer.plan.label') });
    await waitFor(() => expect(plan()).toHaveAttribute('aria-pressed', 'true'));
    expect(within(plan()).getByText(i18n.t('composer.plan.placeDefault'))).toBeVisible();
    await userEvent.click(plan());
    expect(plan()).toHaveAttribute('aria-pressed', 'false');
    expect(within(plan()).queryByText(i18n.t('composer.plan.placeDefault'))).toBeNull();
    await userEvent.type(screen.getByRole('textbox', { name: i18n.t('composer.label') }), 'Work directly{Enter}');
    expect(server.submit).toHaveBeenCalledExactlyOnceWith(defaultThreadRead(native.id), [{ type: 'text', text: 'Work directly' }], 'queue', undefined, undefined, false);
  } finally { settings.pop(); }
});


it('marks the first-message Plan default from compact composer facts without a settings read', async () => {
  if (native.binding === null) throw new Error('The fixture needs its reported program');
  native = { ...native, state: 'idle', current_turn_id: null, usage: null, binding: { ...native.binding, plan_mode: true } };
  threadWriter.reset(native.id, { thread: native, turns: [] }, false);
  parameters.composer = { default_delivery: 'queue', paste_inline_context_share: 25, plan_first: true, owner_reads: false };
  show(<Composer thread={native} />);
  const plan = () => screen.getByRole('button', { name: i18n.t('composer.plan.label') });
  await waitFor(() => expect(plan()).toHaveAttribute('aria-pressed', 'true'));
  expect(within(plan()).getByText(i18n.t('composer.plan.placeDefault'))).toBeVisible();
  expect(server.settingsRead).not.toHaveBeenCalled();
  await userEvent.click(plan());
  expect(plan()).toHaveAttribute('aria-pressed', 'false');
  expect(within(plan()).queryByText(i18n.t('composer.plan.placeDefault'))).toBeNull();
});

it('waits for the first-message setting before marking its Plan default', async () => {
  if (native.binding === null) throw new Error('The fixture needs its reported program');
  native = { ...native, state: 'idle', current_turn_id: null, usage: null, binding: { ...native.binding, plan_mode: true } };
  threadWriter.reset(native.id, { thread: native, turns: [] }, false);
  let finish: ((rows: ResolvedSetting[]) => void) | undefined;
  const pending = new Promise<ResolvedSetting[]>((resolve) => { finish = resolve; });
  server.settingsRead.mockReturnValue(pending);
  show(<Composer thread={native} />);
  const plan = () => screen.getByRole('button', { name: i18n.t('composer.plan.label') });
  await waitFor(() => expect(server.settingsRead).toHaveBeenCalledOnce());
  expect(plan()).toHaveAttribute('aria-pressed', 'false');
  expect(within(plan()).queryByText(i18n.t('composer.plan.placeDefault'))).toBeNull();
  const resolve = finish;
  if (resolve === undefined) throw new Error('The pending setting read is required');
  await act(async () => {
    resolve([...settings, { key: 'thread.plan_first', value: true, own: null, source: { scope: 'topic', node: null }, locked_by: null, writable: false, parts: [] }]);
    await pending;
  });
  await waitFor(() => expect(plan()).toHaveAttribute('aria-pressed', 'true'));
  expect(within(plan()).getByText(i18n.t('composer.plan.placeDefault'))).toBeVisible();
});
