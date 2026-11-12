/** Visible org memberships keep their own directory role and remain names when that role cannot be read. */
import { useQuery } from '@tanstack/react-query';
import { useCallback, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { profileMembershipQuery } from '@/api/profile';
import { ApiError } from '@/api/client';
import { PlaceLink } from '@/features/navigation/places';
import type { NodeRef, UserRef } from '@/generated/api-types';
import { Skeleton } from '@/ui/Skeleton';
import { Button } from '@/ui/Button';

function Membership({ user, org }: { user: UserRef; org: NodeRef }) {
  const { t } = useTranslation();
  const [visible, setVisible] = useState(typeof IntersectionObserver === 'undefined');
  const observer = useRef<IntersectionObserver | null>(null);
  const attach = useCallback((element: HTMLLIElement | null) => {
    observer.current?.disconnect();
    if (element === null || typeof IntersectionObserver === 'undefined') return;
    observer.current = new IntersectionObserver((entries) => {
      if (!entries.some((entry) => entry.isIntersecting)) return;
      setVisible(true);
      observer.current?.disconnect();
    });
    observer.current.observe(element);
  }, []);
  const readable = !org.redacted && org.name !== null && user.username !== '';
  const query = useQuery({ ...profileMembershipQuery(user, org.id), enabled: visible && readable });
  const member = !readable || query.isError ? null : query.data;
  const unavailable = query.error instanceof ApiError && [401, 403, 404, 410].includes(query.error.status);
  return <li ref={attach} className="flex min-w-0 flex-wrap items-center gap-x-1" data-person-membership={org.id}>
    {org.redacted || org.name === null ? <span className="text-text-muted">{t('ui.nameUnknown')}</span>
      : <PlaceLink place={org} className="text-accent-text hover:underline">{org.name}</PlaceLink>}
    {member !== null && member !== undefined && <><span aria-hidden>·</span><span className="text-text-muted">{member.is_owner ? `${t('places.members.owner')} · ${t(`roles.${member.role}`)}` : t(`roles.${member.role}`)}</span></>}
    {readable && query.isFetching && (member === null || member === undefined) && <Skeleton className="h-3 w-12" />}
    {readable && query.isError && <span className="inline-flex min-w-0 flex-wrap items-center gap-1 text-sm text-text-muted" role="status">
      {t(unavailable ? 'implicitAvailability.roleUnavailable' : 'implicitAvailability.roleFailed')}
      {!unavailable && <Button size="sm" variant="ghost" pending={query.isFetching} onClick={() => void query.refetch()}>{t('common.tryAgain')}</Button>}
    </span>}
    {readable && query.isSuccess && member === null && <span className="text-sm text-text-muted">{t('implicitAvailability.roleUnreported')}</span>}
  </li>;
}

export function Memberships({ user, orgs }: { user: UserRef; orgs: readonly NodeRef[] }) {
  return orgs.length === 0 ? null : <ul className="flex min-w-0 flex-col gap-1 text-sm">{orgs.map((org) => <Membership key={org.id} user={user} org={org} />)}</ul>;
}
