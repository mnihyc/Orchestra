/** A message on its way to the agent, drawn at the end of the conversation as the person's message, dimmed until the agent takes it.
 *
 * It stands where the message will stand once the agent has taken it: the same gap before it as a row of the conversation has, and the
 * same block, so that the message does not move when its row replaces this one.
 */
import { useTranslation } from 'react-i18next';

import { PERSON_BLOCK } from '@/features/timeline/items/UserMessage';
import type { SentMessage } from '@/features/thread/sentMessages';
import { wordsOf } from '@/lib/content';
import { cx } from '@/ui/cx';
import { Markdown } from '@/ui/Markdown';

export function SentMessageBlock({ message }: { message: SentMessage }) {
  const { t } = useTranslation();
  const text = wordsOf(message.content);
  const others = message.content.filter((part) => part.type !== 'text').length;
  return (
    <div className="mx-auto w-full max-w-(--reading-width) px-4" style={{ paddingTop: 'calc(var(--message-gap) / 2)', paddingBottom: 'calc(var(--message-gap) / 2)' }} data-sent-message={message.key}>
      <div className="mt-(--message-gap) flex min-w-0 flex-col items-end">
        <div className={cx(PERSON_BLOCK, 'opacity-60')}>
          <span className="sr-only">{t('thread.sent.waiting')}</span>
          {text.trim() !== '' && <Markdown text={text} breaks />}
          {others > 0 && <p className="text-xs text-text-subtle">{t('thread.sent.attachments', { count: others })}</p>}
        </div>
      </div>
    </div>
  );
}
