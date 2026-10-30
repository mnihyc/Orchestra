/** Reset with an email token.
 *
 * The link that was mailed, or that an administrator handed over, asks for a new password. Setting
 * it ends every session of that person, on every device.
 */
import { useMutation, useQuery } from '@tanstack/react-query';
import { Link } from '@tanstack/react-router';
import { CircleCheck, LinkIcon } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { confirmRecovery, metaQuery, PASSWORD_MIN_CHARS, recoveryQuery } from '@/api/auth';
import { ApiError, client } from '@/api/client';
import { AuthCard } from '@/features/auth/Frame';
import { useIdentityRules } from '@/features/auth/identityRules';
import { useForm } from '@/lib/forms';
import { Button } from '@/ui/Button';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { PasswordInput } from '@/ui/Input';
import { linkClasses } from '@/ui/link';
import { SkeletonRows } from '@/ui/Skeleton';

export function ResetPassword({ token }: { token: string }) {
  return <ResetForToken key={token} token={token} />;
}

function ResetForToken({ token }: { token: string }) {
  const { t } = useTranslation();
  const checked = useQuery(recoveryQuery(token));
  const system = useQuery(metaQuery);
  const meta = system.data;
  const rules = useIdentityRules();
  const form = useForm({ password: '' }, { password: (value) => rules.password(value, []) });

  const reset = useMutation({
    mutationFn: confirmRecovery,
    onSuccess: () => {
      // Whoever was signed in on this browser is signed out with every other device: find that out now.
      if (client.session.getState().status === 'signed_in') void client.renew().catch(() => undefined);
    },
    onError: (error) => form.refuse(error),
  });
  const back = <Link to="/login" className={linkClasses}>{t('auth.links.backToSignIn')}</Link>;

  if (reset.isSuccess) {
    return (
      <AuthCard title={t('auth.reset.title')}>
        <EmptyState
          compact
          icon={<CircleCheck aria-hidden />}
          title={t('auth.reset.done')}
          description={t('auth.reset.doneMore')}
          action={
            <Button variant="primary" asChild>
              <Link to="/login">{t('auth.links.signIn')}</Link>
            </Button>
          }
        />
      </AuthCard>
    );
  }

  const failed = reset.error ?? (!reset.isPending ? checked.error : null);
  const spent = failed instanceof ApiError && (failed.status === 410 || failed.status === 404);
  if (spent) {
    return (
      <AuthCard title={t('auth.reset.linkTitle')} foot={back}>
        <EmptyState
          compact
          icon={<LinkIcon aria-hidden />}
          title={t('auth.reset.spent')}
          description={meta?.mail === false ? t('auth.recover.noMail') : undefined}
          action={
            meta?.mail === true ? <Button asChild>
              <Link to="/recover">{t('auth.reset.askAgain')}</Link>
            </Button> : undefined
          }
        />
        {meta===undefined && (system.isError ? <ErrorState compact error={system.error} onRetry={()=>void system.refetch()} /> : <SkeletonRows rows={1} />)}
      </AuthCard>
    );
  }

  if (checked.isPending) {
    return <AuthCard title={t('auth.reset.linkTitle')} foot={back}><SkeletonRows rows={2} /></AuthCard>;
  }
  if (checked.isError && !reset.isPending) {
    return <AuthCard title={t('auth.reset.linkTitle')} foot={back}><ErrorState compact error={checked.error} retrying={checked.isRefetching} onRetry={() => void checked.refetch()} /></AuthCard>;
  }

  const password = form.field('password');
  return (
    <AuthCard
      title={t('auth.reset.title')}
      lead={t('auth.reset.lead')}
      foot={back}
    >
      <form
        noValidate
        className="flex flex-col gap-4"
        onSubmit={form.submit((values) => {
          if (!reset.isPending) reset.mutate({ token, password: values.password });
        })}
      >
        <PasswordInput
          label={t('auth.fields.newPassword')}
          hint={t('auth.fields.passwordHint', { min: PASSWORD_MIN_CHARS })}
          autoComplete="new-password"
          autoFocus
          {...password}
        />
        {reset.error !== null && password.error === undefined && <ErrorAlert error={reset.error} />}
        <Button type="submit" variant="primary" block pending={reset.isPending}>
          {t('auth.reset.submit')}
        </Button>
      </form>
    </AuthCard>
  );
}
