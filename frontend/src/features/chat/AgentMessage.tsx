/** A message posted by an agent in a chat that was shared with it for writing, with the thread it came from.
 *
 * It stands in the list like anyone's, but the avatar is the agent's own square, never a person's
 * colour, the name carries an "agent" chip, and "from thread" leads to the thread for those who may
 * open it. Nothing of this is served until chats can be shared with agents; the message is drawn
 * as it will be when its author is one.
 */
import { Bot } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { isPlainClick, useOpenPath } from '@/features/chat/open';
import type { AuthorAgent } from '@/generated/api-types';
import { Avatar } from '@/ui/Avatar';
import { AgentChip } from '@/ui/AgentChip';

export function AgentAvatar({ size = 28 }: { size?: 24 | 28 | 32 | 40 | 'row' }) {
  return <Avatar text="" color={0} kind="agent" size={size} mark={<Bot size={16} aria-hidden />} />;
}

/** The name of an agent that wrote a message, its chip, and the thread it came from. */
export function AgentName({ author }: { author: AuthorAgent }) {
  const { t } = useTranslation();
  const open = useOpenPath();
  const path = `/thread/${author.thread.id}`;
  return (
    <span className="flex min-w-0 flex-wrap items-baseline gap-x-2">
      <AgentChip name={author.agent} thread={author.thread} />
      <span className="rounded-sm bg-bg-muted px-1 text-xs font-medium text-text-muted">{t('chat.agent.chip')}</span>
      {author.thread.name !== null && !author.thread.redacted && (
        <a
          href={path}
          onClick={(event) => {
            if (!isPlainClick(event)) return;
            event.preventDefault();
            open(path);
          }}
          className="truncate text-xs text-accent-text hover:underline"
        >
          {t('chat.agent.fromThread', { name: author.thread.name })}
        </a>
      )}
    </span>
  );
}
