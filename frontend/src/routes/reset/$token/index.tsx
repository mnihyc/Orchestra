/** Reset password. Loader, permission handling and search schema are pending.
 *
 * The link is its own proof, so the screen is open to anybody who holds it, signed in or not.
 */
import { createFileRoute } from '@tanstack/react-router';

import { ResetPassword } from '@/features/auth/ResetPassword';

export const Route = createFileRoute('/reset/$token/')({
  staticData: { needs: 'any', frame: 'plain' },
  component: ResetRoute,
});

function ResetRoute() {
  const { token } = Route.useParams();
  return <ResetPassword token={token} />;
}
