/** Email recovery.
 *
 * Asks for the address and always answers the same, so that the page never says whether an address
 * is known here. Where the system sends no mail there is nothing to ask for, and the page says who
 * can help instead.
 */
import { useMutation, useQuery } from '@tanstack/react-query';
import { Link } from '@tanstack/react-router';
import { MailCheck } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { askRecovery, metaQuery } from '@/api/auth';
import { AuthCard } from '@/features/auth/Frame';
import { useIdentityRules } from '@/features/auth/identityRules';
import { useForm } from '@/lib/forms';
import { Button } from '@/ui/Button';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';
import { Input } from '@/ui/Input';
import { linkClasses } from '@/ui/link';
import { SkeletonRows } from '@/ui/Skeleton';

export function Recovery() {
  const { t } = useTranslation();
  const system = useQuery(metaQuery);
  const meta = system.data;
  const rules = useIdentityRules();
  const form = useForm({ email: '' }, { email: rules.email });
  const ask = useMutation({ mutationFn: askRecovery, onError: (error) => form.refuse(error) });

  const back = (
    <Link to="/login" className={linkClasses}>
      {t('auth.links.backToSignIn')}
    </Link>
  );

  if (meta === undefined) return <AuthCard title={t('auth.recover.title')} foot={back}>
    {system.isError ? <ErrorState compact error={system.error} onRetry={() => void system.refetch()} /> : <SkeletonRows rows={2} />}
  </AuthCard>;

  if (!meta.mail) {
    return (
      <AuthCard title={t('auth.recover.title')} foot={back}>
        <InlineAlert tone="info">{t('auth.recover.noMail')}</InlineAlert>
      </AuthCard>
    );
  }

  if (ask.isSuccess) {
    return (
      <AuthCard title={t('auth.recover.title')} foot={back}>
        <EmptyState
          compact
          icon={<MailCheck aria-hidden />}
          title={t('auth.recover.sent')}
          description={t('auth.recover.sentMore')}
        />
      </AuthCard>
    );
  }

  return (
    <AuthCard title={t('auth.recover.title')} lead={t('auth.recover.lead')} foot={back}>
      <form
        noValidate
        className="flex flex-col gap-4"
        onSubmit={form.submit((values) => {
          if (!ask.isPending) ask.mutate(values.email.trim());
        })}
      >
        <Input
          label={t('auth.fields.email')}
          type="email"
          inputMode="email"
          autoComplete="email"
          autoFocus
          {...form.field('email')}
        />
        {ask.error !== null && form.field('email').error === undefined && <ErrorAlert error={ask.error} />}
        <Button type="submit" variant="primary" block pending={ask.isPending}>
          {t('auth.recover.submit')}
        </Button>
      </form>
    </AuthCard>
  );
}
