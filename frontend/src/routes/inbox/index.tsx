/** Notification inbox. Loader, permission handling and search schema are pending.
 *
 * What the person was told: every kind of notification, read and unread, with filters. The filter that
 * is chosen is in the address (`filter`), so that a link reproduces it.
 */
import { usePageSearch, usePageNavigate } from '@/features/workbench/pageContext';
import { createFileRoute } from '@tanstack/react-router';

import { Inbox } from '@/features/inbox/Inbox';
import type { InboxFilter } from '@/features/inbox/model';

interface InboxSearch {
  filter?: Exclude<InboxFilter, 'all'>;
}

export const Route = createFileRoute('/inbox/')({
  staticData: { screen: 'page' },
  validateSearch: (search: Record<string, unknown>): InboxSearch =>
    search.filter === 'unread' || search.filter === 'mentions' ? { filter: search.filter } : {},
  component: InboxRoute,
});

function InboxRoute() {
  const { filter } = usePageSearch(Route);
  const navigate = usePageNavigate(Route);
  return <Inbox filter={filter ?? 'all'} onFilter={(next) => void navigate({ search: next === 'all' ? {} : { filter: next }, replace: true })} />;
}
