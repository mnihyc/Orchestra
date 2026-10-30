/** The running composer's hint matches its actual native delivery keys, independent of the place's preferred delivery. */
import { act, fireEvent, screen, waitFor } from '@testing-library/react';
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
import { useComposerStore } from '@/features/composer/Draft';
import { client } from '@/api/client';
import type { ResolvedSetting, Thread, ThreadContext, ThreadParameters } from '@/generated/api-types';
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
import { show } from './support';

const server = vi.hoisted(() => ({ submit: vi.fn<typeof Draft.submitInput>(), context: vi.fn<() => Promise<ThreadContext>>(), cancelUpload: vi.fn<() => Promise<void>>(), live: true }));
vi.mock('@/features/composer/Draft', async (load) => ({ ...await load<typeof Draft>(), useDraftSync: () => 'idle', submitInput: server.submit }));
vi.mock('@/realtime/live', async (load) => ({ ...await load<typeof Live>(), useSocketStatus: () => ({ phase: server.live ? 'ready' : 'reconnecting' }) }));
vi.mock('@/api/threads', async (load) => ({ ...await load<typeof ThreadsApi>(),
  parametersQuery: (id: string) => ({ queryKey: ['keyboard-parameters', id], queryFn: () => Promise.resolve(parameters) }),
  limitsQuery: (id: string) => ({ queryKey: ['keyboard-limits', id], queryFn: () => Promise.resolve({ reached: [] }) }),
}));
vi.mock('@/api/settings', async (load) => ({ ...await load<typeof SettingsApi>(), settingsQuery: (id: string) => ({ queryKey: ['keyboard-settings', id], queryFn: () => Promise.resolve(settings) }) }));
vi.mock('@/api/auth', async (load) => ({ ...await load<typeof AuthApi>(), metaQuery: { queryKey: ['keyboard-meta'], queryFn: () => Promise.resolve(meta()) } }));
vi.mock('@/api/context', async (load) => ({ ...await load<typeof ContextApi>(), threadContextQuery: (id: string, detail: 'kept' | 'count', revision: number) => ({ queryKey: ['keyboard-context', id, detail, revision], queryFn: () => server.context() }) }));
vi.mock('@/features/composer/ModelEffort', () => ({ ModelEffort: () => <button>Model</button> }));
vi.mock('@/features/composer/ApprovalMode', () => ({ ApprovalModePicker: () => <button>Mode</button> }));
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
  server.submit.mockReset(); server.live = true;
  server.cancelUpload.mockReset().mockResolvedValue(undefined);
  server.context.mockReset(); server.context.mockResolvedValue({ harness_kind: null, harness: null, reports: { tokens: false, context: false, breakdown: false }, fill: null, turn_id: null, shortened: null, can_count: false });
  parameters.model.options = [];
  const world = new World(); const person = world.people.get('ada');
  const node = world.places.rowOf(ACME.mainThread);
  if (person === undefined || node === undefined) throw new Error('The keyboard test needs the native thread fixture');
  native = { ...world.threads.thread(person, node), state: 'running' };
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
  it('queues with Enter, steers with Ctrl+Enter and interrupts only with the explicit modified key', async () => {
    show(<Composer thread={native} />);
    const hint = i18n.t(isApplePlatform() ? 'composer.runningHintMac' : 'composer.runningHint');
    expect(screen.getAllByText(hint, { exact: true })).toHaveLength(1);
    const box = screen.getByRole('textbox', { name: i18n.t('composer.label') });
    const user = userEvent.setup();
    await user.type(box, 'Check the redirect');
    fireEvent.compositionStart(box); fireEvent.keyDown(box, { key: 'Enter', isComposing: true }); fireEvent.compositionEnd(box);
    fireEvent.keyDown(box, { key: 'Enter', altKey: true }); fireEvent.keyDown(box, { key: 'Enter', shiftKey: true });
    expect(server.submit).not.toHaveBeenCalled();
    await user.keyboard('{Enter}');
    expect(server.submit).toHaveBeenCalledExactlyOnceWith(defaultThreadRead(native.id), [{ type: 'text', text: 'Check the redirect' }], 'queue', undefined, undefined, false);
    expect(box).toHaveValue('');
    await user.type(box, 'Keep the return URL'); await user.keyboard('{Control>}{Enter}{/Control}');
    expect(server.submit).toHaveBeenNthCalledWith(2, defaultThreadRead(native.id), [{ type: 'text', text: 'Keep the return URL' }], 'steer', undefined, undefined, false);
    await user.type(box, 'Stop and check auth'); await user.keyboard('{Control>}{Shift>}{Enter}{/Shift}{/Control}');
    expect(server.submit).toHaveBeenNthCalledWith(3, defaultThreadRead(native.id), [{ type: 'text', text: 'Stop and check auth' }], 'interrupt', undefined, undefined, false);
    expect(server.submit).toHaveBeenCalledTimes(3);
  });
  it('does not promise unsupported steering and removes the running hint when idle or offline', async () => {
    if (native.binding === null) throw new Error('The fixture must name its agent');
    const agentId = native.binding.agent.id;
    const view = show(<Composer thread={native} />);
    act(() => useComposerStore.setState({ cannotSteer: { [agentId]: true } }));
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

it('keeps Send held for a build-disabled native image after the file and paste flows are combined', async () => {
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
  expect(screen.getByRole('button', { name: i18n.t('composer.send') })).toBeDisabled();
  expect(screen.queryByRole('button', { name: i18n.t('uploads.changeModel') })).not.toBeInTheDocument();
  fireEvent.keyDown(screen.getByRole('textbox', { name: i18n.t('composer.label') }), { key: 'Enter' });
  expect(server.submit).not.toHaveBeenCalled();
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
  expect(screen.getByRole('button', { name: i18n.t('composer.send') })).toBeDisabled();
  expect(document.body).not.toHaveTextContent('research-agent');
});
