/** Direct and group chats of an org. Loader, permission handling and search schema.
 *
 * The screen is the list of the caller's direct and group chats in a root org and the chat that is open
 * beside it. The address says which chat (`chat`), which message to go to (`message`), whether the
 * reader's switch for other people's chats is on (`show`) and for which org (`of`), so that a link
 * reproduces the screen.
 */
import { usePageParams, usePageSearch, usePageNavigate } from '@/features/workbench/pageContext';
import { createFileRoute } from '@tanstack/react-router';

import { Messages, type MessagesSearch } from '@/features/chat/Messages';

const text = (value: unknown): string | undefined => (typeof value === 'string' && value !== '' ? value : undefined);

export const Route = createFileRoute('/org/$slug/messages/')({
  staticData: { screen: 'page' },
  validateSearch: (search: Record<string, unknown>): MessagesSearch => {
    const chat = text(search.chat);
    const message = text(search.message);
    const of = text(search.of);
    return {
      ...(chat === undefined ? {} : { chat }),
      ...(chat !== undefined && message !== undefined ? { message } : {}),
      ...(of === undefined ? {} : { of }),
      ...(search.participating === true || search.participating === 'true' ? { participating: 'true' as const } : {}),
    };
  },
  component: MessagesRoute,
});

function MessagesRoute() {
  const { slug } = usePageParams(Route);
  const search = usePageSearch(Route);
  const navigate = usePageNavigate(Route);
  return (
    <Messages
      slug={slug}
      search={search}
      onSearch={(patch) =>
        void navigate({
          // What is set to nothing leaves the address.
          search: (before) => {
            const merged: Record<string, unknown> = { ...before, ...patch };
            return Object.fromEntries(Object.entries(merged).filter(([, value]) => value !== undefined));
          },
          replace: patch.chat === undefined,
        })
      }
    />
  );
}
