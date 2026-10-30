/** Count of chat messages quoting an item, opening that chat.
 *
 * A small speech bubble with the number, at the end of an item that messages of the thread's chat quote.
 * Pressing it brings the Chat view to the front; nothing of the chat itself is drawn in the conversation.
 */
import { MessageSquare } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { showChat } from '@/features/chat/discuss';
import { Tooltip } from '@/ui/Tooltip';

export function ChatRefs({ count }: { count: number }) {
  const { t } = useTranslation();
  if (count <= 0) return null;
  const said = t('timeline.chatRefs.label', { count });
  return (
    <div className="flex justify-end">
      <Tooltip content={said}>
        <button
          type="button"
          aria-label={said}
          onClick={showChat}
          className="inline-flex items-center gap-1 rounded-md px-2 py-1 text-sm text-text-muted hover:bg-bg-muted hover:text-text"
        >
          <MessageSquare size={16} aria-hidden />
          <span aria-hidden>{count}</span>
        </button>
      </Tooltip>
    </div>
  );
}
