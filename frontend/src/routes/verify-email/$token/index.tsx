/** Verify email. Loader, permission handling and search schema are pending.
 *
 * Opening the link uses it. A link is good for one use, so it is used once for as long as this page
 * lives, however often the route is entered or loaded again; only a failure that was not the link's
 * own is tried anew.
 */
import { createFileRoute, useRouter } from '@tanstack/react-router';

import { confirmEmail } from '@/api/auth';
import { ApiError } from '@/api/client';
import { type Verification, VerifyEmail } from '@/features/auth/VerifyEmail';

const used = new Map<string, Promise<Verification>>();

function confirmOnce(token: string): Promise<Verification> {
  let outcome = used.get(token);
  if (outcome === undefined) {
    outcome = confirmEmail(token).then(
      (signedIn): Verification => ({ confirmed: true, signedIn }),
      (error: unknown): Verification => {
        // The API answered: that is the link's fate. Anything else may be tried again.
        if (!(error instanceof ApiError)) used.delete(token);
        return { confirmed: false, error };
      },
    );
    used.set(token, outcome);
  }
  return outcome;
}

export const Route = createFileRoute('/verify-email/$token/')({
  staticData: { needs: 'any', frame: 'plain' },
  loader: ({ params }) => confirmOnce(params.token),
  component: VerifyRoute,
});

function VerifyRoute() {
  const router = useRouter();
  const outcome = Route.useLoaderData();
  return <VerifyEmail outcome={outcome} onRetry={() => void router.invalidate()} />;
}
