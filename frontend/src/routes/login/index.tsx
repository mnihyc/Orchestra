/** Sign in. Loader, permission handling and search schema are pending.
 *
 * `redirect` is where to go on to afterwards, a path inside the application; `error` is the code a
 * sign-in through an identity provider came back with.
 */
import { createFileRoute } from '@tanstack/react-router';

import { Login } from '@/features/auth/Login';
import { callbackCode } from '@/features/auth/IdentityCallback';
import { safeRedirect } from '@/routes/-access';

interface LoginSearch {
  redirect?: string;
  error?: string;
  reason?: string;
}

export const Route = createFileRoute('/login/')({
  staticData: { needs: 'signed_out', frame: 'plain' },
  validateSearch: (search: Record<string, unknown>): LoginSearch => ({
    ...(typeof search.redirect === 'string' ? { redirect: search.redirect } : {}),
    ...(typeof search.error === 'string' ? { error: search.error } : {}),
    ...(typeof search.reason === 'string' ? { reason: search.reason } : {}),
  }),
  component: LoginRoute,
});

function LoginRoute() {
  const search = Route.useSearch();
  return <Login onward={safeRedirect(search.redirect)} callbackError={callbackCode(search.error, search.reason)} />;
}
