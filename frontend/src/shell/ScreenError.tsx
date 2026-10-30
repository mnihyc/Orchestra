/** What stands in place of a screen that could not be shown: what happened, in words, and a way to ask again.
 *
 * A route whose loader was refused, or whose screen failed while it was drawn, shows this where its
 * screen would be, with everything round it as it was. "Try again" asks the route anew.
 */
import { type ErrorComponentProps, useRouter } from '@tanstack/react-router';
import { useTranslation } from 'react-i18next';

import { NoAccess } from '@/features/access/NoAccess';
import { ApiError } from '@/api/client';
import { useScreenHeading } from '@/shell/screen';
import { ErrorState } from '@/ui/ErrorState';

export function ScreenError({ error, reset }: ErrorComponentProps) {
  const { t } = useTranslation();
  const router = useRouter();
  const title = t('shell.screenFailed');
  useScreenHeading({ title });
  // A route that was refused shows what is missing and the way on, and not only that it failed.
  if (error instanceof ApiError && error.is('authz.denied')) return <NoAccess error={error} className="h-full" onRetry={reset} />;
  return (
    <ErrorState
      className="h-full"
      error={error}
      title={title}
      onRetry={() => {
        reset();
        void router.invalidate();
      }}
    />
  );
}
