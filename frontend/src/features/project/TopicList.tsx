/** Topics with tags, state and activity.
 *
 * A project's topics as rows of a table: the name with its description and its tags, what needs a look
 * in it, the state of the environments its threads run in, when something last happened, and the topic's
 * own menu. What a narrow place leaves out of the columns is said under the name instead, so the same
 * rows are a list of cards on a small screen. A topic that is archived is drawn quietly, and one that is
 * done says so.
 */
import { Ellipsis } from 'lucide-react';
import { useRef } from 'react';
import { useTranslation } from 'react-i18next';

import { EnvironmentDots } from '@/features/topic/Environments';
import { AttentionChips } from '@/features/org/ProjectCards';
import { useIsPending } from '@/features/navigation/NodeMenu';
import { PlaceLink } from '@/features/navigation/places';
import { AsAdminMark, adminAccessOf } from '@/ui/AsAdminMark';
import { TopicMenu, TopicMenuEntries } from '@/features/topic/TopicMenu';
import { TopicPath } from '@/features/project/TopicPath';
import type { Topic } from '@/generated/api-types';
import { cx } from '@/ui/cx';
import { Checkbox } from '@/ui/Checkbox';
import { RelativeTime } from '@/ui/RelativeTime';
import { StateIcon, type WorkState } from '@/ui/StateChip';
import { NodeIcon } from '@/ui/NodeIcon';
import { isThreadUrgency } from '@/ui/ThreadState';
import { type Column, Table } from '@/ui/Table';
import { TagChip } from '@/ui/TagChip';

/** A topic's work follows its reader's contained-thread state; archive remains a lifecycle mark. */
export function topicState(topic: Pick<Topic, 'status' | 'archived' | 'attention' | 'thread_display_state'>): WorkState {
  if (topic.archived) return 'archived';
  return isThreadUrgency(topic.thread_display_state) ? topic.thread_display_state : 'idle';
}

/** The tags of a topic, as chips. */
export function TopicTags({ topic, className }: { topic: Pick<Topic, 'tags'>; className?: string }) {
  if (topic.tags.length === 0) return null;
  return (
    <span className={cx('flex flex-wrap items-center gap-1', className)}>
      {topic.tags.map((tag) => (
        <TagChip key={tag.id} name={tag.name} color={tag.color} />
      ))}
    </span>
  );
}

/** What needs a look in a topic: its threads that run, wait for an approval or cannot go on. */
export function topicAttention(topic: Pick<Topic, 'attention'>) {
  return { running: topic.attention.running, waiting: topic.attention.waiting_approval, approvals: topic.attention.waiting_approval, blocked: topic.attention.blocked };
}

/** When something last happened in a topic, as far as anything is known: its last activity, else when it was last changed. */
export const lastActivityOf = (topic: Pick<Topic, 'last_activity_at' | 'updated_at'>): string => topic.last_activity_at ?? topic.updated_at;

/** The header names its loaded page once and reports a partial selection as mixed. */
function pageSelection(chosen: number, count: number): boolean | 'indeterminate' {
  return chosen === 0 ? false : chosen === count ? true : 'indeterminate';
}

export function TopicList({ topics, showPaths = false, selection }: { topics: readonly Topic[]; showPaths?: boolean; selection?: { selected: ReadonlySet<string>; toggle: (id: string, on: boolean, shift: boolean) => void; onPage?: (on: boolean) => void } }) {
  const { t } = useTranslation();
  const shift = useRef(false);
  const selectedOnPage = selection === undefined ? 0 : topics.filter((topic) => selection.selected.has(topic.id)).length;

  const columns: Column<Topic>[] = [
    ...(selection === undefined ? [] : [{ id: 'select', header: selection.onPage === undefined ? t('day100.batch.select')
      : <Checkbox label={<span className="sr-only">{t('day100.batch.selectPage')}</span>}
          checked={pageSelection(selectedOnPage, topics.length)}
          onCheckedChange={selection.onPage} />, className: 'w-10', cell: (topic: Topic) => <span onClickCapture={(event) => { shift.current = event.shiftKey; }}><Checkbox label={<span className="sr-only">{t('day100.batch.selectRow', { name: topic.name })}</span>} checked={selection.selected.has(topic.id)} onCheckedChange={(on) => { selection.toggle(topic.id, on, shift.current); shift.current = false; }} /></span> }]),
    {
      id: 'name',
      header: t('places.topics.name'),
      cell: (topic, shown) => {
        // What a column that is left out of a narrow place would have said is said here instead.
        const withoutAttention = !shown.has('attention');
        const withoutActivity = !shown.has('activity');
        return (
        <div className="flex min-w-0 items-start gap-3">
          <span className="mt-0.5">
            <NodeIcon kind="topic" color={topic.color} own={topic.custom_color} done={topic.status === 'done'} />
          </span>
          <div className="flex min-w-0 flex-1 flex-col gap-1">
            <span className="flex min-w-0 items-start gap-2"><PlaceLink place={topic} className="wrap-anywhere font-medium text-text hover:underline">
              {topic.name}
            </PlaceLink><AsAdminMark of={adminAccessOf(topic)?.place} responsive />{isThreadUrgency(topic.thread_display_state) && <StateIcon state={topic.thread_display_state} label={t(`navigation.holds.${topic.thread_display_state}`)} />}</span>
            {topic.description !== '' && <span className="line-clamp-1 text-sm wrap-anywhere text-text-muted">{topic.description}</span>}
            <TopicTags topic={topic} />
            {showPaths && <TopicPath topic={topic} />}
            {withoutAttention && <AttentionChips stats={topicAttention(topic)} />}
            {withoutActivity && (
              <span className="text-sm text-text-muted">
                <RelativeTime at={lastActivityOf(topic)} />
              </span>
            )}
          </div>
        </div>
        );
      },
    },
    {
      id: 'attention',
      header: t('day100.views.needsAttention'),
      from: 'md',
      cell: (topic) => <AttentionChips stats={topicAttention(topic)} />,
    },
    {
      id: 'environments',
      header: t('places.topics.environments'),
      from: 'lg',
      cell: (topic) => <EnvironmentDots environments={topic.environments} />,
    },
    {
      id: 'activity',
      header: t('places.topics.activity'),
      from: 'sm',
      className: 'whitespace-nowrap text-text-muted',
      cell: (topic) => <RelativeTime at={lastActivityOf(topic)} />,
    },
    {
      id: 'menu',
      header: t('places.topics.actions'),
      hideHeader: true,
      align: 'end',
      className: 'w-10',
      cell: (topic) => <TopicRowMenu topic={topic} />,
    },
  ];

  return <Table label={t('places.topics.title')} columns={columns} rows={topics} rowKey={(topic) => topic.id} rowClassName={(topic) => (topic.archived ? 'opacity-60' : undefined)} rowMenu={{ label: (topic) => t('places.menu.of', { name: topic.name }), content: (topic) => <TopicMenuEntries topic={topic} context /> }} />;
}

function TopicRowMenu({ topic }: { topic: Topic }) {
  const pending = useIsPending(topic.id);
  return (
    <span className="inline-flex items-center gap-1">
      {pending && <Ellipsis className="animate-skeleton text-text-subtle" aria-hidden />}
      <TopicMenu topic={topic} />
    </span>
  );
}
