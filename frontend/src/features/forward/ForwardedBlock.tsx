/** Forwarded copies with their source, author and time, in a chat or a timeline.
 *
 * Inside the message of whoever forwarded, one block for each source: "Forwarded from" the chat or the
 * thread, then each original under its own author and time with the images and files it held. The
 * name of the source leads to the original for those who may open it and is plain text for the rest.
 * A copy does not change when its original does, and reading it needs no access to the original.
 */
import { useQuery } from '@tanstack/react-query';
import { CornerUpRight } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { orgsQuery } from '@/api/nodes';
import { type ForwardedPart } from '@/api/chats';
import { Body } from '@/features/chat/Body';
import { FileChip, ImageThumb } from '@/features/chat/Attachments';
import { isPlainClick, useOpenPath } from '@/features/chat/open';
import { placePath } from '@/features/navigation/places';
import { UserChip } from '@/ui/UserChip';
import { AgentChip } from '@/ui/AgentChip';
import { RelativeTime } from '@/ui/RelativeTime';

/** The key that says two copies come from the same place: consecutive ones share a block. */
function sourceKey(part: ForwardedPart): string {
  return part.from.kind === 'chat' ? `chat:${part.from.chat.id}` : `thread:${part.from.thread.id}`;
}

/** Groups the copies of a message by where they came from, keeping their order. */
export function groupBySource(parts: readonly ForwardedPart[]): ForwardedPart[][] {
  const groups: ForwardedPart[][] = [];
  for (const part of parts) {
    const last = groups.at(-1);
    const head = last?.[0];
    if (last !== undefined && head !== undefined && sourceKey(head) === sourceKey(part)) last.push(part);
    else groups.push([part]);
  }
  return groups;
}

function Source({ part }: { part: ForwardedPart }) {
  const { t } = useTranslation();
  const open = useOpenPath();
  const orgs = useQuery(orgsQuery).data ?? [];
  let label: string;
  let path: string | null = null;
  if (part.from.kind === 'thread') {
    const thread = part.from.thread;
    label = thread.name ?? t('chat.forwarded.someChat');
    if (thread.name !== null && !thread.redacted) path = `${placePath({ id: thread.id, kind: 'thread' })}?item=${part.source_id}`;
  } else {
    const chat = part.from.chat;
    if (chat.kind === 'node') {
      const node = chat.node;
      const named = node !== null && node.name !== null ? node : null;
      label = named === null ? t('chat.forwarded.someChat') : t('chat.title.node', { name: named.name, kind: t(`chat.kind.${named.kind}`) });
      if (named !== null && !named.redacted) {
        path = `${placePath({ id: named.id, kind: named.kind })}?panel=chat&chat=${chat.id}&message=${part.source_id}`;
      }
    } else {
      label = chat.kind === 'group' ? (chat.name ?? t('chat.forwarded.group')) : t('chat.forwarded.direct');
      const org = orgs.find((one) => one.id === chat.org_id);
      if (org !== undefined) path = `/org/${org.address}/messages?chat=${chat.id}&message=${part.source_id}`;
    }
  }
  const target = path;
  return (
    <p className="flex items-center gap-1 text-xs text-text-muted">
      <CornerUpRight size={16} aria-hidden className="shrink-0" />
      <span>{t('chat.forwarded.from')}</span>
      {target === null ? (
        <span className="truncate font-medium text-text">{label}</span>
      ) : (
        <a
          href={target}
          onClick={(event) => {
            if (!isPlainClick(event)) return;
            event.preventDefault();
            open(target);
          }}
          className="truncate font-medium text-accent-text hover:underline"
        >
          {label}
        </a>
      )}
    </p>
  );
}

export interface ForwardedBlocksProps {
  parts: readonly ForwardedPart[];
  /** The chat that carries the copies: the images and files of a copy are read from it. */
  chatId: string;
}

/** A block for each source among the copies of a message. */
export function ForwardedBlocks({ parts, chatId }: ForwardedBlocksProps) {
  return groupBySource(parts).map((group) => {
    const first = group[0];
    if (first === undefined) return null;
    return (
      <section key={`${sourceKey(first)}:${first.source_id}`} className="flex flex-col gap-2 border-l-2 border-border-strong pl-3">
        <Source part={first} />
        {group.map((copy) => (
          <div key={copy.source_id} className="flex flex-col gap-1">
            <div className="flex items-center gap-2 text-sm">
              {copy.author === null ? null : 'id' in copy.author ? <UserChip user={copy.author} size={24} /> : <AgentChip name={copy.author.agent} {...(copy.from.kind === 'thread' ? { thread: copy.from.thread } : {})} />}
              <RelativeTime at={copy.at} className="text-xs text-text-muted" />
            </div>
            {copy.text !== '' && <Body text={copy.text} />}
            {copy.images.map((image) => (
              <ImageThumb key={image.blob} chatId={chatId} blob={image.blob} name={image.name} mediaType={image.media_type} owner={copy.author !== null && !('agent' in copy.author) ? copy.author : undefined} updatedAt={copy.at} />
            ))}
            {copy.attachments.length > 0 && (
              <div className="flex flex-wrap gap-1">
                {copy.attachments.map((file) => (
                  <FileChip key={file.blob} chatId={chatId} blob={file.blob} name={file.name} size={file.size} mediaType={file.media_type} owner={copy.author !== null && !('agent' in copy.author) ? copy.author : undefined} updatedAt={copy.at} />
                ))}
              </div>
            )}
          </div>
        ))}
      </section>
    );
  });
}
