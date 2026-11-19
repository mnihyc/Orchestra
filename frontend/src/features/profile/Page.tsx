/** The page at a person's username: visible pins and work beside their own words. */
import { useQuery } from '@tanstack/react-query';
import { Link } from '@tanstack/react-router';
import { UserRound } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { ApiError } from '@/api/client';
import { profileQuery } from '@/api/profile';
import { AdminReachNotice } from '@/features/access/AdminReachNotice';
import { documentPath, PlaceLink } from '@/features/navigation/places';
import { Activity } from '@/features/profile/Activity';
import { Identity } from '@/features/profile/Identity';
import type { UserProfile } from '@/generated/api-types';
import { Page } from '@/shell/Page';
import { useScreenHeading } from '@/shell/screen';
import { ErrorState } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';
import { Button } from '@/ui/Button';
import { EmptyState } from '@/ui/EmptyState';

export function InspectionBand({ profile }: { profile: UserProfile }) {
  return <AdminReachNotice of={profile.inspected} private object={{ kind: 'user', id: profile.user.id }} />;
}

export function ProfileContents({ profile, preview = false }: { profile: UserProfile; preview?: boolean }) {
  const { t } = useTranslation();
  const PinsHeading = preview ? 'h3' : 'h2';
  return <div className="flex min-w-0 flex-col gap-6"><InspectionBand profile={profile} /><div className="grid min-w-0 gap-6 md:grid-cols-[minmax(12rem,16rem)_minmax(0,1fr)]"><aside aria-label={t(preview ? 'profile.page.previewAbout' : 'profile.page.about')}><Identity profile={profile} heading={!preview} /></aside><div className="flex min-w-0 flex-col gap-6"><section aria-label={t(preview ? 'profile.page.previewPinned' : 'profile.page.pinned')}><PinsHeading className="mb-3 text-lg font-semibold">{t(preview ? 'profile.page.previewPinned' : 'profile.page.pinned')}</PinsHeading>{profile.pins.length === 0 ? <EmptyState compact title={t('profile.page.noPins')} /> : <ul className="grid gap-3 sm:grid-cols-2">{profile.pins.map((pin) => {
    const object = pin.node ?? pin.document;
    if (object === null) return null;
    return <li key={object.id} className="rounded-lg border border-border bg-surface p-4">{pin.node !== null ? <><PlaceLink place={pin.node} className="font-medium text-accent-text hover:underline">{pin.node.name}</PlaceLink><p className="mt-1 text-sm text-text-muted">{t(`finder.kinds.${pin.node.kind}`)}</p>{pin.node.description !== '' && <p className="mt-2 line-clamp-2 text-sm text-text-muted">{pin.node.description}</p>}</> : pin.document !== null && <><a href={documentPath(pin.document.id)} className="font-medium text-accent-text hover:underline">{pin.document.title}</a><p className="mt-1 text-sm text-text-muted">{t('profile.page.document')} · {pin.document.node.name}</p></>}</li>;
  })}</ul>}</section>{profile.activity && !preview && <Activity username={profile.user.username} />}</div></div></div>;
}

export function ProfilePage({ username }: { username: string }) {
  const { t } = useTranslation();
  const query = useQuery(profileQuery(username));
  useScreenHeading({ title: query.data?.user.display_name ?? t('profile.page.title'), icon: UserRound, target: query.data === undefined ? undefined : { kind: 'user', id: query.data.user.id } });
  return <Page>{query.isPending ? <SkeletonRows rows={6} /> : query.isError ? query.error instanceof ApiError && query.error.status === 404 ? <EmptyState icon={<UserRound aria-hidden />} titleAs="h1" title={t('profile.page.missing')} action={<Button asChild><Link to="/">{t('actions.home.label')}</Link></Button>} /> : <ErrorState error={query.error} onRetry={() => void query.refetch()} /> : <ProfileContents profile={query.data} />}</Page>;
}
