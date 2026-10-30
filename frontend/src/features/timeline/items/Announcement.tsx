/** What a thread was told: an announcement, or a milestone raised to the topic, with its origin and severity; or a message another thread sent to this one, with its files.
 *
 * What a thread was told: an announcement from a person or from another thread, with its title and
 * text; a milestone another thread raised to the topic, with a flag in place of the megaphone; or a
 * message another thread sent to this one alone, shown as the message it is. One that matters is in
 * the warning style.
 */
import { Flag, Megaphone, Send } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import type { Item } from '@/generated/api-types';
import { AgentReferences } from '@/features/people/AgentReferences';
import { useTimelineContext } from '@/features/timeline/context';
import { ItemFrame } from '@/features/timeline/ItemActions';
import { cx } from '@/ui/cx';
import { Markdown } from '@/ui/Markdown';
import { TimeOfDay } from '@/ui/TimeOfDay';

type AnnouncementItem = Extract<Item, { kind: 'announcement' }>;

export function AnnouncementCard({ item }: { item: AnnouncementItem }) {
  const { t } = useTranslation();
  const { threadId } = useTimelineContext();
  const { from, title, body, severity, category, to_thread: toThread } = item.payload;
  const sender = 'thread' in from ? from.thread.name : from.user.display_name;
  const Icon = toThread ? Send : category === 'milestone' ? Flag : Megaphone;
  const lead = toThread ? t('timeline.announcement.messageFrom', { name: sender }) : t('timeline.announcement.from', { name: sender });
  return (
    <ItemFrame item={item} label={t('timeline.announcement.menu')}>
      <section
        aria-label={toThread ? t('timeline.announcement.message') : t('timeline.announcement.title')}
        className={cx('flex flex-col gap-1 rounded-lg border px-3 py-2', severity === 'important' ? 'border-warning/40 bg-warning-subtle' : 'border-border bg-surface')}
      >
        <p className="flex items-center gap-2 text-sm text-text-muted">
          <Icon aria-hidden />
          <span>{lead}</span>
          <TimeOfDay at={item.created_at} className="text-xs text-text-subtle" />
          {severity === 'important' && <span className="text-xs font-medium text-text">· {t('timeline.announcement.important')}</span>}
        </p>
        {!toThread && title !== '' && <p className="text-base font-semibold text-text">{title}</p>}
        {body.trim() !== '' && (
          // A message from another thread was written for this one: its references are this thread's to resolve.
          <AgentReferences threadId={threadId}>
            <Markdown text={body} compact />
          </AgentReferences>
        )}
      </section>
    </ItemFrame>
  );
}
