/** An image the agent looked at or made.
 *
 * An image the agent looked at or made: its name, what it shows in the words the harness gave, and
 * its type. The picture is kept in the thread's files and is opened from there.
 */
import { useTranslation } from 'react-i18next';

import type { Item } from '@/generated/api-types';
import { FileAttachment } from '@/features/files/FileChip';
import { blobSource } from '@/features/files/source';
import { useTimelineContext } from '@/features/timeline/context';

type ImageItem = Extract<Item, { kind: 'tool_call' }>;

export function ImageDisplay({ item }: { item: ImageItem }) {
  const { t } = useTranslation();
  const { threadId } = useTimelineContext();
  const tool = item.payload.tool;
  if (tool.kind !== 'image') return null;
  return <div className="flex min-w-0 flex-col gap-1 text-sm">
    <FileAttachment source={{ ...blobSource('thread', threadId, tool.blob, tool.name ?? t('timeline.step.imageUnnamed'), undefined, tool.media_type), owner: item.author ?? undefined, updatedAt: item.created_at }} />
    {tool.caption !== null && tool.caption !== '' && <span className="text-text-muted">{tool.caption}</span>}
  </div>;
}
