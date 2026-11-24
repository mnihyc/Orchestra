/** A cold recorded conversation keeps each checkpoint on its person's keyboard-reachable request. */
import { screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import type * as Source from '@/api/source';
import { NOT_PICKING, TimelineContextValue, type TimelineContext } from '@/features/timeline/context';
import { UserMessage } from '@/features/timeline/items/UserMessage';
import { ViewCompactContext } from '@/features/workbench/Narrow';
import { i18n, setLocale } from '@/i18n';
import { threadWriter } from '@/realtime/stores/thread';
import { ACME } from '../doubles/fixtures';
import { recordedConversation } from '../doubles/recorded-conversation';
import { World } from '../doubles/world';
import { show } from './support';

const reads = vi.hoisted(() => vi.fn());
vi.mock('@/api/source', async (load) => ({ ...await load<typeof Source>(), snapshotQuery: (id: string) => ({ queryKey: ['recorded-checkpoint', id], queryFn: reads }) }));
afterEach(async () => { threadWriter.evict(ACME.mainThread); reads.mockClear(); await setLocale('en'); document.documentElement.removeAttribute('data-theme'); });
for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [400, 1440]) {
  it(`keeps two recorded checkpoint controls on the person rows at ${locale}/${theme}/${width}`, async () => {
    await setLocale(locale); document.documentElement.dataset.theme = theme;
    const world = new World(), person = world.people.get('ada'), node = world.places.rowOf(ACME.mainThread);
    if (person === undefined || node === undefined) throw new Error('The recorded conversation needs its native reader and thread.');
    const thread = { ...world.threads.thread(person, node), environment_permissions: ['files.view', 'files.manage'] };
    const rows = recordedConversation(); threadWriter.reset(thread.id, { thread, ...rows }, false);
    const context: TimelineContext = { threadId: thread.id, threadName: thread.name, detail: 'normal', layout: 'one', setLayout: () => undefined,
      meId: person.me.id, canInteract: true, canManage: true, live: true, lit: null, selection: NOT_PICKING };
    const view = show(<TimelineContextValue.Provider value={context}><ViewCompactContext.Provider value={width === 400}>
      {rows.items.filter((item) => item.kind === 'user_message').map((item) => <UserMessage key={item.id} item={item} />)}
    </ViewCompactContext.Provider></TimelineContextValue.Provider>);
    expect(view.container.querySelectorAll('[data-restore-turn]')).toHaveLength(2);
    const user = userEvent.setup();
    for (const [id, turn] of [['request-one', 'recorded-one'], ['request-two', 'recorded-two']] as const) {
      const row = view.container.querySelector(`[data-item="${id}"]`);
      if (!(row instanceof HTMLElement)) throw new Error('The checkpoint must belong to its actual person message.');
      const restore = within(row).getByRole('button', { name: i18n.t('thread.turnActions.restoreBefore') });
      expect(restore).toHaveAttribute('data-restore-turn', turn); expect(restore).toBeEnabled();
      const copy = within(row).getByRole('button', { name: i18n.t('timeline.actions.copy') });
      await user.click(copy); await user.tab(); expect(restore).toHaveFocus();
    }
    expect(reads).not.toHaveBeenCalled(); expect(screen.getAllByRole('button', { name: i18n.t('thread.turnActions.restoreBefore') })).toHaveLength(2);
  });
}
