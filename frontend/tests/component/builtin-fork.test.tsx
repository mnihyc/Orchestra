/** Built-in forks retain their real source identity, selected history and one creation intent. */
import { act, cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as AskApi from '@/api/ask';
import { client } from '@/api/client';
import { useImmediateFork } from '@/features/thread/ImmediateFork';
import type { AskKeepReply, Thread } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { ACME } from '../doubles/fixtures';
import { World } from '../doubles/world';
import { show } from './support';

const calls = vi.hoisted(() => ({ keep: vi.fn<typeof AskApi.keepAsk>(), plan: vi.fn(), native: vi.fn(), navigate: vi.fn() }));
vi.mock('@/api/ask', async (load) => ({ ...await load<typeof AskApi>(), keepAsk: calls.keep }));
vi.mock('@/api/forks', async (load) => ({ ...await load<object>(), planFork: calls.plan }));
vi.mock('@/features/thread/forkNow', () => ({ useForkNow: () => ({ pending: false, start: calls.native }) }));
vi.mock('@/features/settings/PlacePicker', () => ({ PlacePicker: () => null }));
vi.mock('@tanstack/react-router', async (load) => ({ ...await load<object>(), useNavigate: () => calls.navigate }));

let thread: Thread;
const reply: AskKeepReply = { conversation_id: ACME.mainThread, thread_id: ACME.repro, topic_id: ACME.fixLogin };
function Entry() {
  const fork = useImmediateFork(thread);
  return <><button onClick={() => fork.start(1)}>Open fork</button>{fork.confirmation}</>;
}
beforeEach(() => {
  vi.clearAllMocks(); calls.keep.mockReset().mockResolvedValue(reply);
  const world = new World(), person = world.people.get('ada'), row = world.places.rowOf(ACME.mainThread);
  if (person === undefined || row === undefined) throw new Error('The source needs a genuine thread');
  thread = { ...world.threads.thread(person, row), binding: null, environment: null, environment_permissions: [],
    program: { id: 'orch', root_org_id: ACME.org, read_only: true, requires_environment: false, requires_native_harness: false } };
  client.session.setState({ status: 'signed_in', userId: person.me.id, readerVersion: 0 });
});
afterEach(async () => { cleanup(); act(() => client.session.setState({ status: 'signed_out', userId: null })); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) it(`retries a Private selected-history fork with the same creation key (${locale})`, async () => {
  // EN: kept thread > first answer > Fork from here > review Private and History
  // through turn 1 > Fork > lost response > Fork again > open the one new thread.
  // 中文：保留的线程 > 首轮回答 > 从这里分叉 > 查看私有及历史范围 > 分叉 > 回答丢失 >
  // 再次分叉；只能打开同一个新线程，不要求智能体或环境。
  await setLocale(locale);
  calls.keep.mockRejectedValueOnce(new Error('The response was lost'));
  show(<Entry />);
  await userEvent.click(screen.getByRole('button', { name: 'Open fork' }));
  expect(await screen.findByText(i18n.t('ask.privateThread'))).toBeVisible();
  expect(screen.getByText(i18n.t('agentAccess.forkThrough', { turn: 1 }))).toBeVisible();
  const confirm = screen.getByRole('button', { name: i18n.t('change.menu.forkNow') });
  await userEvent.click(confirm);
  expect(await screen.findByRole('alert')).toBeVisible();
  await waitFor(() => expect(confirm).toBeEnabled());
  await userEvent.click(confirm);
  await waitFor(() => expect(calls.navigate).toHaveBeenCalledExactlyOnceWith({ to: '/thread/$threadId', params: { threadId: ACME.repro } }));
  expect(calls.keep).toHaveBeenCalledTimes(2);
  expect(calls.keep.mock.calls[0]).toEqual([thread.id, { topic_id: null, project_id: null, title: null }, { key: expect.any(String), toTurn: 1 }]);
  expect(calls.keep.mock.calls[1]).toEqual(calls.keep.mock.calls[0]);
  expect(calls.plan).not.toHaveBeenCalled(); expect(calls.native).not.toHaveBeenCalled();
});

it('does not navigate a replacement reader after the original fork finishes', async () => {
  let complete: ((value: AskKeepReply) => void) | undefined;
  calls.keep.mockImplementation(() => new Promise<AskKeepReply>((resolve) => { complete = resolve; }));
  show(<Entry />);
  await userEvent.click(screen.getByRole('button', { name: 'Open fork' }));
  await userEvent.click(screen.getByRole('button', { name: i18n.t('change.menu.forkNow') }));
  await waitFor(() => expect(calls.keep).toHaveBeenCalledOnce());
  act(() => client.session.setState({ readerVersion: 1 }));
  await act(async () => { complete?.(reply); await Promise.resolve(); });
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  expect(calls.navigate).not.toHaveBeenCalled();
});
