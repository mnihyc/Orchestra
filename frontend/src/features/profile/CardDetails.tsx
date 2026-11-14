/** A name card opens the same privacy-filtered page facts, retaining the current screen's actions. */
import { Link } from '@tanstack/react-router';
import { linkTo } from '@/lib/links';
import { copyText } from '@/ui/clipboard';
import { Button } from '@/ui/Button';
import { useQuery } from '@tanstack/react-query';
import type { ReactNode } from 'react';
import { useTranslation } from 'react-i18next';

import { ApiError } from '@/api/client';
import { profileQuery } from '@/api/profile';
import { Identity } from '@/features/profile/Identity';
import { InspectionBand } from '@/features/profile/Page';
import { ErrorState } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';

export function CardDetails({ username, actions, hideMessage = false, onNavigate }: { username: string; actions?: ReactNode; hideMessage?: boolean; onNavigate?: () => void }) {
  const { t } = useTranslation();
  const query = useQuery(profileQuery(username));
  return <div className="flex max-w-[min(24rem,calc(100vw-2rem))] flex-col gap-3 p-3 text-text">{query.isPending ? <SkeletonRows read={query} rows={3} /> : query.isError ? query.error instanceof ApiError && query.error.status === 404 ? <p className="text-sm text-text-muted">{t('profile.page.missing')}</p> : <ErrorState error={query.error} compact onRetry={() => void query.refetch()} /> : <><InspectionBand profile={query.data} /><Identity profile={query.data} compact hideMessage={hideMessage} onNavigate={onNavigate} /><Link to="/u/$username" params={{ username }} onClick={onNavigate} className="text-sm text-accent-text hover:underline">{t('profile.page.open')}</Link></>}<Button size="sm" variant="ghost" onClick={() => void copyText(linkTo({ kind: 'person', id: username }), t('common.linkCopied'))}>{t('actions.copyLink.label')}</Button>{actions}</div>;
}
