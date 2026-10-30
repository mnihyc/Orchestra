/** Emoji reactions and who reacted.
 *
 * Under a message, each emoji with how many reacted, ringed when the reader is among them. Resting
 * on one lists who; pressing one adds or takes away the reader's own, shown at once. A reaction is
 * a button with a name that says what it does, never an emoji alone.
 */
import type { TFunction } from 'i18next';
import type { ReactNode } from 'react';
import { useTranslation } from 'react-i18next';

import { namesOf } from '@/features/chat/acts';
import type { Reaction } from '@/generated/api-types';
import { cx } from '@/ui/cx';
import { Tooltip } from '@/ui/Tooltip';

export interface ReactionsProps {
  reactions: readonly Reaction[];
  me: string | null;
  people: ReadonlyMap<string, { display_name: string }>;
  onToggle: (emoji: string) => void;
  /** Whether the reader may react at all. */
  canReact: boolean;
}

/** Who reacted with an emoji, in words: the names the page knows, and how many others. */
export function whoReacted(t: TFunction, reaction: Reaction, people: ReadonlyMap<string, { display_name: string }>): string {
  const { names, others } = namesOf(reaction.users, people);
  if (names.length === 0) return t('chat.reactions.others', { count: others });
  const list = names.join(', ');
  return others === 0 ? t('chat.reactions.who', { names: list }) : t('chat.reactions.whoAndOthers', { names: list, count: others });
}

export function Reactions({ reactions, me, people, onToggle, canReact }: ReactionsProps): ReactNode {
  const { t } = useTranslation();
  if (reactions.length === 0) return null;
  return (
    <div role="group" aria-label={t('chat.reactions.label')} className="mt-1 flex flex-wrap items-center gap-1">
      {reactions.map((reaction) => {
        const mine = me !== null && reaction.users.includes(me);
        const who = whoReacted(t, reaction, people);
        return (
          <Tooltip key={reaction.emoji} content={who}>
            <button
              type="button"
              aria-pressed={mine}
              aria-label={`${reaction.emoji} ${t('chat.reactions.count', { count: reaction.users.length })}`}
              disabled={!canReact}
              onClick={() => onToggle(reaction.emoji)}
              className={cx(
                'inline-flex h-6 items-center gap-1 rounded-full border px-2 text-sm tabular-nums transition-colors',
                mine ? 'border-accent bg-accent-subtle text-accent-text' : 'border-border bg-bg-subtle text-text-muted hover:border-border-strong',
                !canReact && 'cursor-default',
              )}
            >
              <span aria-hidden>{reaction.emoji}</span>
              <span aria-hidden>{reaction.users.length}</span>
            </button>
          </Tooltip>
        );
      })}
    </div>
  );
}
