/** Page selection lives in the table header; its reviewed results survive clearing the selection. */
import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ReactNode } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { changeTopicStatus } from '@/api/day100';
import { Topics } from '@/features/project/Topics';
import type { Project, Topic } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { me, org, userRef } from '../doubles/fixtures';
import { show } from './support';

const root = org('org', 'acme', 'Acme', userRef(me(1, 'ada', 'Ada')));
const project: Project = { ...root, kind: 'project', id: 'project', parent_id: root.id, name: 'Release', topic_count: 2,
  access: { ...root.access, permissions: ['project.view', 'project.interact'] } };
const makeTopic = (id: string, name: string): Topic => ({ ...project, kind: 'topic', id, name, parent_id: project.id,
  status: 'open', done_at: null, thread_display_state: null, thread_count: 0, attention: { running: 0, waiting_approval: 0, blocked: 0 },
  work_attention: { uncommitted: 0, unpushed: 0, failed: 0 }, default_environment_id: null, environments: [], last_activity_at: null, seq: 0,
  access: { ...root.access, permissions: ['topic.view', 'topic.interact'] } });
const topics = [makeTopic('one', 'Ship the new release'), makeTopic('two', 'Review the complete long release checklist')];
vi.mock('@/api/nodes', async (original) => ({ ...await original<object>(), topicsQuery: (id: string) => ({
  queryKey: ['compact-selection', id], initialPageParam: null, getNextPageParam: () => undefined,
  queryFn: () => Promise.resolve({ items: topics, next_cursor: null }),
}) }));
vi.mock('@/api/day100', async (original) => ({ ...await original<object>(), changeTopicStatus: vi.fn() }));
vi.mock('@/features/navigation/NodeMenu', () => ({ useIsPending: () => false, NodeMenuButton: () => null }));
vi.mock('@/features/topic/TopicMenu', () => ({ TopicMenu: () => null, TopicMenuEntries: () => null }));
vi.mock('@/features/navigation/places', async (original) => ({ ...await original<object>(),
  PlaceLink: ({ place, children, className }: { place: { id: string }; children: ReactNode; className?: string }) => <a href={`/topic/${place.id}`} className={className}>{children}</a>,
}));
const appearance = useAppearance.getState();
beforeEach(() => { vi.clearAllMocks(); });
afterEach(async () => { useAppearance.setState(appearance); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  it(`selects one loaded page with a mixed state, then clears only that page (${locale}/${theme})`, async () => {
    await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme });
    show(<Topics project={project} search={{ view: 'list' }} onSearch={vi.fn()} />);
    const table = await screen.findByRole('table', { name: i18n.t('places.topics.title') });
    const page = within(table).getByRole('checkbox', { name: i18n.t('day100.batch.selectPage') });
    expect(page.closest('thead')).not.toBeNull();
    expect(screen.queryByRole('button', { name: i18n.t('day100.batch.selectPage') })).not.toBeInTheDocument();
    expect(screen.queryByRole('region', { name: i18n.t('day100.batch.selection') })).not.toBeInTheDocument();
    const first = within(table).getByRole('checkbox', { name: i18n.t('day100.batch.selectRow', { name: topics[0]!.name }) });
    const user = userEvent.setup();
    await user.click(first);
    expect(page).toHaveAttribute('aria-checked', 'mixed');
    await user.click(page);
    expect(page).toBeChecked();
    for (const topic of topics) expect(within(table).getByRole('checkbox', { name: i18n.t('day100.batch.selectRow', { name: topic.name }) })).toBeChecked();
    await user.click(page);
    expect(page).not.toBeChecked();
    expect(screen.queryByRole('region', { name: i18n.t('day100.batch.selection') })).not.toBeInTheDocument();
    expect(changeTopicStatus).not.toHaveBeenCalled();
  });

  it(`keeps confirmed results and guarded Undo open after the selected rows clear (${locale}/${theme})`, async () => {
    await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme });
    vi.mocked(changeTopicStatus).mockImplementation((id, status) => {
      const before = topics.find((topic) => topic.id === id);
      if (before === undefined) return Promise.reject(new Error('the batch must name a loaded topic'));
      return Promise.resolve({ ...before, status, revision: before.revision + 1 });
    });
    show(<Topics project={project} search={{ view: 'list' }} onSearch={vi.fn()} />);
    const user = userEvent.setup();
    await user.click(await screen.findByRole('checkbox', { name: i18n.t('day100.batch.selectPage') }));
    await user.click(screen.getByRole('button', { name: i18n.t('places.menu.markDone') }));
    const review = screen.getByRole('dialog', { name: i18n.t('day100.batch.review', { count: 2 }) });
    expect(within(review).getAllByRole('button', { name: i18n.t('common.close') })).toHaveLength(1);
    await user.click(within(review).getByRole('button', { name: i18n.t('day100.batch.markDone', { count: 2 }) }));
    const results = await screen.findByRole('dialog', { name: i18n.t('day100.batch.results', { count: 2 }) });
    await waitFor(() => expect(changeTopicStatus).toHaveBeenCalledTimes(2));
    for (const topic of topics) {
      expect(changeTopicStatus).toHaveBeenCalledWith(topic.id, 'done', topic.revision);
      expect(within(results).getByText(topic.name, { exact: true })).toBeVisible();
    }
    expect(within(results).getByRole('button', { name: i18n.t('day100.batch.undo') })).toBeEnabled();
    expect(within(results).getAllByRole('button', { name: i18n.t('common.close') })).toHaveLength(1);
    await user.click(within(results).getByRole('button', { name: i18n.t('common.close') }));
    expect(screen.queryByRole('region', { name: i18n.t('day100.batch.selection') })).not.toBeInTheDocument();
    expect(screen.getByRole('checkbox', { name: i18n.t('day100.batch.selectPage') })).not.toBeChecked();
  });
}
