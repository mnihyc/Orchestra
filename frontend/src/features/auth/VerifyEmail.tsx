/** Email verification.
 *
 * The link that confirms an address is used as soon as it is opened, once. What this screen shows is
 * how that went: confirmed, and where to go on; or a link that was used already or has ended, with
 * the way to ask for a new one.
 */
import { useMutation, useQuery } from '@tanstack/react-query';
import { Link } from '@tanstack/react-router';
import { CircleCheck, LinkIcon } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { metaQuery, resendConfirmation } from '@/api/auth';
import { ApiError } from '@/api/client';
import { AuthCard } from '@/features/auth/Frame';
import { useForm } from '@/lib/forms';
import { Button } from '@/ui/Button';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';
import { Input } from '@/ui/Input';
import { linkClasses } from '@/ui/link';
import { SkeletonRows } from '@/ui/Skeleton';

/** How using the link went. */
export type Verification =
  /** The address is confirmed; `signedIn` says whether the link also signed its person in. */
  | { confirmed: true; signedIn: boolean }
  | { confirmed: false; error: unknown };

export interface VerifyEmailProps {
  outcome: Verification;
  /** Uses the link again, after a failure that was not the link's. */
  onRetry: () => void;
}

export function VerifyEmail({ outcome, onRetry }: VerifyEmailProps) {
  const { t } = useTranslation();
  const system = useQuery(metaQuery);
  const meta = system.data;

  if (outcome.confirmed) {
    return (
      <AuthCard title={t('auth.verify.title')}>
        <EmptyState
          compact
          icon={<CircleCheck aria-hidden />}
          title={t('auth.verify.done')}
          description={outcome.signedIn ? undefined : t('auth.verify.doneSignIn')}
          action={
            <Button variant="primary" asChild>
              {outcome.signedIn ? <Link to="/">{t('common.continue')}</Link> : <Link to="/login">{t('auth.links.signIn')}</Link>}
            </Button>
          }
        />
      </AuthCard>
    );
  }

  const spent = outcome.error instanceof ApiError && (outcome.error.status === 410 || outcome.error.status === 404);
  if (!spent) {
    return (
      <AuthCard title={t('auth.verify.title')}>
        <ErrorState compact error={outcome.error} title={t('auth.verify.failed')} onRetry={onRetry} />
      </AuthCard>
    );
  }

  return (
    <AuthCard
      title={t('auth.verify.title')}
      foot={
        <Link to="/login" className={linkClasses}>
          {t('auth.links.backToSignIn')}
        </Link>
      }
    >
      <EmptyState compact icon={<LinkIcon aria-hidden />} title={t('auth.verify.spent')} />
      {meta?.mail === true ? <ResendForm /> : meta?.mail === false ? <p className="text-sm text-text-muted">{t('auth.verify.noMail')}</p> : null}
      {meta===undefined && (system.isError ? <ErrorState compact error={system.error} onRetry={()=>void system.refetch()} /> : <SkeletonRows rows={1} />)}
    </AuthCard>
  );
}

/** Asks for the confirmation link once more, by the name its person signs in with. */
function ResendForm() {
  const { t } = useTranslation();
  const form = useForm({ login: '' }, { login: (value) => (value.trim() === '' ? t('auth.rules.loginMissing') : null) });
  const again = useMutation({ mutationFn: resendConfirmation, onError: (error) => form.refuse(error) });

  if (again.isSuccess) return <InlineAlert tone="success">{t('auth.verify.resent')}</InlineAlert>;

  return (
    <form
      noValidate
      className="flex flex-col gap-4"
      onSubmit={form.submit((values) => {
        if (!again.isPending) again.mutate(values.login.trim());
      })}
    >
      <Input
        label={t('auth.fields.login')}
        autoComplete="username"
        autoCapitalize="none"
        autoCorrect="off"
        spellCheck={false}
        {...form.field('login')}
      />
      {again.error !== null && form.field('login').error === undefined && <ErrorAlert error={again.error} />}
      <Button type="submit" block pending={again.isPending}>
        {t('auth.verify.resend')}
      </Button>
    </form>
  );
}
