/** Invitation. Loader, permission handling and search schema are pending.
 *
 * Open to anybody who holds the link: what it is for is shown before anything is asked.
 */
import { useQuery } from '@tanstack/react-query';
import { createFileRoute } from '@tanstack/react-router';

import { metaQuery } from '@/api/auth';
import { Invitation } from '@/features/auth/Invitation';

export const Route = createFileRoute('/invite/$token/')({
  staticData: { needs: 'any', frame: 'plain' },
  component: InviteRoute,
});

function InviteRoute() {
  const { token } = Route.useParams();
  const meta = useQuery(metaQuery).data;
  return <Invitation token={token} system={meta?.name ?? ''} />;
}
