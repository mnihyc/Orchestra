/** Register. Loader, permission handling and search schema are pending.
 *
 * `invitation` is the secret of an invitation's link, when the visitor came by one.
 */
import { createFileRoute } from '@tanstack/react-router';

import { Register } from '@/features/auth/Register';

interface RegisterSearch {
  invitation?: string;
}

export const Route = createFileRoute('/register/')({
  staticData: { needs: 'signed_out', frame: 'plain' },
  validateSearch: (search: Record<string, unknown>): RegisterSearch =>
    typeof search.invitation === 'string' && search.invitation !== '' ? { invitation: search.invitation } : {},
  component: RegisterRoute,
});

function RegisterRoute() {
  const search = Route.useSearch();
  return <Register invitation={search.invitation ?? null} />;
}
