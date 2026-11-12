/** No-environment creation keeps one genuine native request across a lost reply. */
import { act, cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as ThreadsApi from '@/api/threads';
import { client } from '@/api/client';
import { ProgramThread } from '@/features/start/ProgramThread';
import { NO_PICKS } from '@/features/start/choices';
import type { AskModelsReply, Node, Thread } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { askModels } from '../unit/ask-fixtures';
import { ACME } from '../doubles/fixtures';
import { World } from '../doubles/world';
import { show } from './support';

const calls = vi.hoisted(() => ({ create: vi.fn<typeof ThreadsApi.createThread>(), navigate: vi.fn(), node: null as Node | null, models: null as AskModelsReply | null }));
vi.mock('@/api/threads', async (load) => ({ ...await load<typeof ThreadsApi>(), createThread: calls.create }));
vi.mock('@/api/nodes', async (load) => ({ ...await load<object>(), nodeQuery: (id: string) => ({ queryKey: ['program-create-node', id], queryFn: () => Promise.resolve(calls.node) }) }));
vi.mock('@/api/ask', async (load) => ({ ...await load<object>(), askModelsQuery: (root: string) => ({ queryKey: ['program-create-models', root], queryFn: () => Promise.resolve(calls.models ?? askModels(root)) }) }));
vi.mock('@/features/ask/ModelChoice', () => ({ AskModelChoice: () => null }));
vi.mock('@tanstack/react-router', async (load) => ({ ...await load<object>(), useNavigate: () => calls.navigate }));

let made: Thread;
const parent = { id: ACME.fixLogin, kind: 'topic' as const, name: 'Fix login', parentId: ACME.web, archived: false, status: 'open' as const, folder: false };
beforeEach(() => {
  vi.clearAllMocks(); calls.models = null;
  const world = new World(), person = world.people.get('ada'), row = world.places.rowOf(ACME.mainThread);
  if (person === undefined || row === undefined) throw new Error('A genuine thread is required');
  calls.node = world.places.handle(person, 'GET', `/nodes/${ACME.fixLogin}`, new URLSearchParams(), {})?.body as Node;
  made = { ...world.threads.thread(person, row), binding: null, environment: null, program_model: null,
    program: { id: 'orch', root_org_id: ACME.org, read_only: true, requires_environment: false, requires_native_harness: false } };
  calls.create.mockReset().mockResolvedValue(made);
  client.session.setState({ status: 'signed_in', userId: person.me.id, readerVersion: 0 });
});
afterEach(async () => { cleanup(); act(() => client.session.setState({ status: 'signed_out', userId: null })); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) it(`retries the same native program birth without profile or environment operands (${locale})`, async () => {
  // EN: topic > New thread > Read-only questions > first question > Create >
  // lost reply > Create again. One creation key and first-input identity remain.
  // 中文：话题 > 新会话 > 只读问答 > 首个问题 > 创建 > 回答丢失 > 再次创建；
  // 保持同一个创建键和首条输入标识。
  await setLocale(locale); calls.create.mockRejectedValueOnce(new Error('The reply was lost'));
  show(<ProgramThread parent={parent} open draft={{ picks: NO_PICKS, message: 'First question', generalAccess: 'private' }} onClose={() => undefined} onNative={() => undefined} />);
  const create = screen.getByRole('button', { name: i18n.t('start.create') });
  await waitFor(() => expect(create).toBeEnabled());
  await userEvent.click(create); await waitFor(() => expect(calls.create).toHaveBeenCalledOnce());
  await waitFor(() => expect(create).toBeEnabled()); await userEvent.click(create);
  await waitFor(() => expect(calls.navigate).toHaveBeenCalledOnce());
  expect(calls.create.mock.calls[1]).toEqual(calls.create.mock.calls[0]);
  const body = calls.create.mock.calls[0]?.[1];
  expect(body).toMatchObject({ program: { id: 'orch' }, access: 'private', first_input: { content: [{ type: 'text', text: 'First question' }] } });
  expect(body?.first_input?.client_id).toBeTruthy();
  for (const field of ['environment', 'binding', 'agent_access', 'model', 'effort', 'directory']) expect(body).not.toHaveProperty(field);
});

for (const locale of ['en', 'zh-CN'] as const) {
  it(`blocks a first question under an unset Ask list while permitting an empty draft thread (${locale})`, async () => {
    // EN: topic > New thread > Read-only questions > unset Ask list; the first
    // question cannot be created. Clear it to create an empty thread without a model pick.
    // 中文：话题 > 新会话 > 只读问答 > 未配置模型列表；首个问题不能创建。
    // 清空问题后可创建空会话，无需先选择模型。
    await setLocale(locale);
    calls.models = {root_org_id: ACME.org, models: [], default_model: null, blocked_reason: 'not_configured', writable: false};
    show(<ProgramThread parent={parent} open draft={{picks: NO_PICKS, message: 'Retained first question', generalAccess: 'private'}} onClose={() => undefined} onNative={() => undefined} />);
    await screen.findByText(i18n.t('ask.noModels'));
    const create = screen.getByRole('button', {name: i18n.t('start.create')});
    expect(create).toBeDisabled(); expect(calls.create).not.toHaveBeenCalled();
    expect(screen.queryByText(i18n.t('ask.noDefault'))).not.toBeInTheDocument();
    const message = screen.getByRole('textbox', {name: i18n.t('start.message.title')});
    expect(message).toHaveValue('Retained first question');
    await userEvent.clear(message);
    await waitFor(() => expect(create).toBeEnabled());
    await userEvent.click(create);
    await waitFor(() => expect(calls.create).toHaveBeenCalledOnce());
    expect(calls.create.mock.calls[0]?.[1]).toMatchObject({program: {id: 'orch'}});
    expect(calls.create.mock.calls[0]?.[1]).not.toHaveProperty('first_input');
  });

  it(`refuses a removed explicit model even for an empty new thread (${locale})`, async () => {
    // EN: reopen a new-thread draft whose saved model was removed > Create
    // stays disabled; stale preferences cannot bypass the configured Ask list.
    // 中文：重开新会话草稿，原模型已移除 > 创建保持禁用；旧偏好不能绕过配置列表。
    await setLocale(locale);
    const model = askModels(ACME.org).default_model;
    if (model === null) throw new Error('The prior preference needs a real fixture identity');
    calls.models = {root_org_id: ACME.org, models: [], default_model: null, blocked_reason: 'empty', writable: false};
    show(<ProgramThread parent={parent} open draft={{picks: NO_PICKS, message: '', generalAccess: 'private', program: {id: 'orch', model}}} onClose={() => undefined} onNative={() => undefined} />);
    await screen.findByText(i18n.t('ask.modelUnavailable'));
    expect(screen.getByRole('button', {name: i18n.t('start.create')})).toBeDisabled();
    expect(calls.create).not.toHaveBeenCalled();
  });
}
