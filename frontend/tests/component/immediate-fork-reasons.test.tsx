/** Fork confirmation explains an unavailable preview without starting the fork. */
import { act, cleanup, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import type * as Forks from '@/api/forks';
import { useImmediateFork } from '@/features/thread/ImmediateFork';
import type { Thread } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { ACME } from '../doubles/fixtures';
import { World } from '../doubles/world';
import { show } from './support';

const calls = vi.hoisted(() => ({ plan: vi.fn<typeof Forks.planFork>(), start: vi.fn() }));
vi.mock('@/api/forks', async (load) => ({ ...await load<typeof Forks>(), planFork: calls.plan }));
vi.mock('@/features/thread/forkNow', () => ({ useForkNow: () => ({ pending: false, start: calls.start }) }));
function Choice({ thread }: { thread: Thread }) {
  const fork = useImmediateFork(thread);
  return <><button onClick={() => fork.start()}>Open fork</button>{fork.confirmation}</>;
}
afterEach(async () => { cleanup(); vi.clearAllMocks(); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) it.each(['checking', 'failed'] as const)('explains the %s fork in '+locale, async (state) => {
  await setLocale(locale);
  calls.plan.mockImplementation(() => state === 'checking' ? new Promise(() => undefined) : Promise.reject(new Error('The preview failed')));
  const world = new World(), person = world.people.get('ada'), row = world.places.rowOf(ACME.mainThread);
  if (person === undefined || row === undefined) throw new Error('The fork fixture needs its native thread');
  show(<Choice thread={world.threads.thread(person, row)} />);
  await userEvent.click(screen.getByRole('button', { name: 'Open fork' }));
  if (state === 'failed') await screen.findByRole('alert');
  const confirm = screen.getByRole('button', { name: i18n.t('change.menu.forkNow') }), target = confirm.parentElement;
  if (target === null) throw new Error('The disabled fork reason must be focusable');
  expect(confirm).toBeDisabled(); act(() => target.focus());
  expect(await screen.findByRole('tooltip')).toHaveTextContent(i18n.t(state === 'checking' ? 'ui.unavailable.checkingFork' : 'ui.unavailable.forkUnavailable'));
  expect(calls.start).not.toHaveBeenCalled();
});
