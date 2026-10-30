/** What an invitation is for and until when, before anything is asked; one that is no longer good says so and no more.
 *
 * Whoever holds an invitation's link may read what it is: who asks, into what, as what, and until
 * when. Somebody signed in accepts it here. Somebody who is not chooses between making an account
 * with it and signing in first.
 */
import { useMutation, useQuery } from '@tanstack/react-query';
import { Link, useRouter } from '@tanstack/react-router';
import { MailCheck, MailX } from 'lucide-react';
import { useMemo } from 'react';
import { useStore } from 'zustand';
import { useTranslation } from 'react-i18next';

import { meQuery, signOut } from '@/api/auth';
import { ApiError, client } from '@/api/client';
import { acceptInvitation, invitationQuery } from '@/api/users';
import { AuthCard } from '@/features/auth/Frame';
import { acceptedDestination } from '@/features/auth/accepted';
import { usePlaceLabels } from '@/features/navigation/placeLabels';
import type { InvitationPreview } from '@/generated/api-types';
import { useAppearance } from '@/lib/appearance';
import { formatDateTime } from '@/lib/time';
import { Avatar } from '@/ui/Avatar';
import { Button } from '@/ui/Button';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { linkButtonClasses, linkClasses } from '@/ui/link';
import { Skeleton, SkeletonGroup } from '@/ui/Skeleton';

/** Whether a lookup failed because the invitation ended, was used up or was withdrawn: the answer says no more than that. */
export function isSpent(error: unknown): boolean {
  return error instanceof ApiError && (error.status === 410 || error.status === 404);
}

/** The org an invitation is into, named with its address where an org the reader is in already has its name. */
function useInvitedOrg(preview: InvitationPreview): string | null {
  const org = preview.org;
  const placed = useMemo(() => (org === null ? [] : [{ id: `invited:${org.slug}`, name: org.name, above: [], address: org.slug }]), [org]);
  const labels = usePlaceLabels(placed, { trees: false });
  return org === null ? null : (labels.get(`invited:${org.slug}`) ?? org.name);
}

/** The sentence that says what an invitation is, by what it invites to. */
function useInvitationSentence(): (preview: InvitationPreview, system: string, org: string | null) => string {
  const { t } = useTranslation();
  return (preview, system, org) => {
    const inviter = preview.inviter.display_name;
    const role = preview.role === null ? null : t(`roles.${preview.role}`);
    if (preview.what !== null) {
      const sentence = t(`auth.invite.to.${preview.what.kind}`, { inviter, name: preview.what.name });
      return role === null ? sentence : `${sentence} ${t('auth.invite.asRole', { role })}`;
    }
    if (org !== null) {
      return role === null ? t('auth.invite.toOrg', { inviter, org }) : t('auth.invite.toOrgAs', { inviter, org, role });
    }
    return t('auth.invite.toSystem', { inviter, system });
  };
}

/** What an invitation is, in a few lines: who asks, into what, until when, and whether it is bound to one address. */
export function InvitationSummary({ preview, system }: { preview: InvitationPreview; system: string }) {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  const timeZone = useAppearance((state) => state.timeZone);
  const sentence = useInvitationSentence();
  const org = useInvitedOrg(preview);
  return (
    <div className="flex items-start gap-3 rounded-lg border border-border bg-bg-subtle p-3">
      <Avatar
        text={preview.inviter.avatar.text}
        color={preview.inviter.avatar.color}
        kind={preview.inviter.kind === 'service' ? 'service' : 'human'}
        size={32}
      />
      <div className="flex min-w-0 flex-col gap-1 text-base">
        <p className="font-medium wrap-anywhere text-text">{sentence(preview, system, org)}</p>
        {preview.what !== null && org !== null && <p className="text-sm text-text-muted">{t('auth.invite.partOf', { org })}</p>}
        <p className="text-sm text-text-muted">
          {t('auth.invite.goodUntil', { time: formatDateTime(preview.expires_at, { locale, timeZone }) })}
        </p>
        {preview.email_bound && <p className="text-sm text-text-muted">{t('auth.invite.bound')}</p>}
      </div>
    </div>
  );
}

/** An invitation that ended, was used up or was withdrawn: it says so and no more. */
export function InvitationSpent() {
  const { t } = useTranslation();
  return <EmptyState compact icon={<MailX aria-hidden />} title={t('auth.invite.spent')} action={<Button asChild><Link to="/login">{t('auth.links.backToSignIn')}</Link></Button>} />;
}

export function InvitationSkeleton() {
  return (
    <SkeletonGroup className="flex items-start gap-3 rounded-lg border border-border bg-bg-subtle p-3">
      <Skeleton className="size-8 rounded-full" />
      <div className="flex flex-1 flex-col gap-2 py-1">
        <Skeleton className="h-4 w-4/5" />
        <Skeleton className="h-3 w-1/2" />
      </div>
    </SkeletonGroup>
  );
}

export function Invitation({ token, system }: { token: string; system: string }) {
  const { t } = useTranslation();
  const router = useRouter();
  const signedIn = useStore(client.session, (state) => state.status === 'signed_in');
  const preview = useQuery(invitationQuery(token));
  const me = useQuery({ ...meQuery, enabled: signedIn });

  const accept = useMutation({
    mutationFn: () => acceptInvitation(token),
    onSuccess: (reply) => router.navigate({...acceptedDestination(reply),replace:true}),
  });
  const leave = useMutation({ mutationFn: signOut });

  return (
    <AuthCard
      title={t('auth.invite.title')}
      foot={
        signedIn ? (
          <p>
            {t('auth.invite.signedInAs', { name: me.data?.display_name ?? '' })}{' '}
            <button type="button" className={linkButtonClasses} disabled={leave.isPending} onClick={() => leave.mutate()}>
              {t('auth.invite.notYou')}
            </button>
          </p>
        ) : undefined
      }
    >
      {preview.isPending ? (
        <InvitationSkeleton />
      ) : preview.isError ? (
        isSpent(preview.error) ? (
          <InvitationSpent />
        ) : (
          <ErrorState
            compact
            error={preview.error}
            retrying={preview.isRefetching}
            onRetry={() => void preview.refetch()}
          />
        )
      ) : (
        <div className="flex flex-col gap-4">
          <InvitationSummary preview={preview.data} system={system} />
          {accept.error !== null &&
            (isSpent(accept.error) ? <InvitationSpent /> : <ErrorAlert error={accept.error} />)}
          {isSpent(accept.error) ? null : signedIn ? (
            <Button
              variant="primary"
              block
              icon={<MailCheck aria-hidden />}
              pending={accept.isPending}
              onClick={() => accept.mutate()}
            >
              {t('auth.invite.accept')}
            </Button>
          ) : (
            <div className="flex flex-col gap-3">
              <Button variant="primary" block asChild>
                <Link to="/register" search={{ invitation: token }}>
                  {t('auth.invite.register')}
                </Link>
              </Button>
              <p className="text-center text-sm text-text-muted">
                {t('auth.invite.haveAccount')}{' '}
                <Link to="/login" search={{ redirect: `/invite/${encodeURIComponent(token)}` }} className={linkClasses}>
                  {t('auth.links.signIn')}
                </Link>
              </p>
            </div>
          )}
        </div>
      )}
    </AuthCard>
  );
}
