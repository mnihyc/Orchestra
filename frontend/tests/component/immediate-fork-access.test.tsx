/** A bounded suggestion is frozen after the preview and remains visible until confirmation. */
import { StrictMode } from 'react';
import { act, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { client } from '@/api/client';
import { useImmediateFork } from '@/features/thread/ImmediateFork';
import type { AgentAccessChoice, ForkRequest, Thread } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { ACME } from '../doubles/fixtures';
import { World } from '../doubles/world';
import { show } from './support';

const calls = vi.hoisted(() => ({ read: vi.fn<(request: ForkRequest) => void>(), confirm: vi.fn() }));
vi.mock('@/features/thread/forkNow', () => ({ useForkNow: () => ({ pending: false, start: calls.confirm }) }));
vi.mock('@/api/forks', async (load) => ({ ...await load<object>(), planFork: (_id: string, request: ForkRequest) => {
  calls.read(request);
  return Promise.resolve({
    access: { summary: { custom: false, from: null, view: [], interact: [], manage: [] } },
    agent_access: { preset: { where: 'topic', parts: { documents: 'edit' } }, choices: [
      { where: 'nothing', parts: {} }, { where: 'thread', parts: {} },
      { where: 'topic', parts: { documents: 'read' } }, { where: 'project', parts: {} },
    ] },
  });
} }));
let thread: Thread;
function Entry() {
  const fork = useImmediateFork(thread);
  return <><button onClick={() => fork.start(3)}>Open fork</button>{fork.confirmation}</>;
}
beforeEach(() => {
  vi.clearAllMocks();
  const world = new World(), person = world.people.get('ada'), row = world.places.rowOf(ACME.mainThread);
  if (person === undefined || row === undefined) throw new Error('The native fixture thread is required');
  thread = world.threads.thread(person, row);
  client.session.setState({ status: 'signed_in', userId: person.me.id });
});
afterEach(async () => { act(() => client.session.setState({ status: 'signed_out', userId: null })); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) it(`keeps the reduced effective choice through the fresh confirmation preview (${locale})`, async () => {
  await setLocale(locale);
  const errors = vi.spyOn(console, 'error');
  show(<StrictMode><Entry /></StrictMode>);
  await userEvent.click(screen.getByRole('button', { name: 'Open fork' }));
  await waitFor(() => expect(calls.read.mock.calls.some(([request]) => request.agent_access?.parts.documents === 'read')).toBe(true));
  expect(await screen.findByText(i18n.t('startImplicit.reducedTo'))).toBeVisible();
  const confirm = screen.getByRole('button', { name: i18n.t('change.menu.forkNow') });
  await waitFor(() => expect(confirm).toBeEnabled());
  expect(calls.confirm).not.toHaveBeenCalled();
  await userEvent.click(confirm);
  expect(calls.confirm).toHaveBeenCalledExactlyOnceWith(3, { where: 'topic', parts: { documents: 'read' } } satisfies AgentAccessChoice);
  expect(errors.mock.calls.some((args) => /while rendering|Cannot update a component/.test(args.join(' ')))).toBe(false);
});
