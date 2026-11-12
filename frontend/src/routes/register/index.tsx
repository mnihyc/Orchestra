/** Register. Loader, permission handling and search schema are pending.
 *
 * `invitation` is the secret of an invitation's link, when the visitor came by one.
 */
import { createFileRoute } from '@tanstack/react-router';

import { Register } from '@/features/auth/Register';

interface RegisterSearch {
  invitation?: string;
  accepted_terms_hash?: string;
}

export const Route = createFileRoute('/register/')({
  staticData: { needs: 'signed_out', frame: 'plain' },
  validateSearch: (search: Record<string, unknown>): RegisterSearch => ({
    ...(typeof search.invitation === 'string' && search.invitation !== '' ? { invitation: search.invitation } : {}),
    ...(typeof search.accepted_terms_hash === 'string' && /^[a-f0-9]{64}$/u.test(search.accepted_terms_hash) ? { accepted_terms_hash: search.accepted_terms_hash } : {}),
  }),
  component: RegisterRoute,
});

function RegisterRoute() {
  const search = Route.useSearch();
  return <Register invitation={search.invitation ?? null} acceptedTermsHash={search.accepted_terms_hash ?? null} />;
}
