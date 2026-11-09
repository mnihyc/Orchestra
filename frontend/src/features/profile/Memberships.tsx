/** Visible org memberships keep their own directory role and remain names when that role cannot be read. */
import { useQuery } from '@tanstack/react-query';
import { useCallback, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { profileMembershipQuery } from '@/api/profile';
import { PlaceLink } from '@/features/navigation/places';
import type { NodeRef, UserRef } from '@/generated/api-types';
import { Skeleton } from '@/ui/Skeleton';

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
  const query = useQuery({ ...profileMembershipQuery(user, org.id), enabled: visible && !org.redacted && org.name !== null && user.username !== '' });
  const member = query.isError ? null : query.data;
  return <li ref={attach} className="flex min-w-0 flex-wrap items-center gap-x-1" data-person-membership={org.id}>
    {org.redacted || org.name === null ? <span className="text-text-muted">{t('ui.nameUnknown')}</span>
      : <PlaceLink place={org} className="text-accent-text hover:underline">{org.name}</PlaceLink>}
    {member !== null && member !== undefined && <><span aria-hidden>·</span><span className="text-text-muted">{member.is_owner ? `${t('places.members.owner')} · ${t(`roles.${member.role}`)}` : t(`roles.${member.role}`)}</span></>}
    {query.isFetching && (member === null || member === undefined) && <Skeleton className="h-3 w-12" />}
  </li>;
}

export function Memberships({ user, orgs }: { user: UserRef; orgs: readonly NodeRef[] }) {
  return orgs.length === 0 ? null : <ul className="flex min-w-0 flex-col gap-1 text-sm">{orgs.map((org) => <Membership key={org.id} user={user} org={org} />)}</ul>;
}
