/** The identity column and the compact name card use the same visible page facts. */
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { useNavigate } from '@tanstack/react-router';
import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { choiceOption } from '@/i18n/choices';

import { createChat } from '@/api/chats';
import { nodeQuery } from '@/api/nodes';
import { PlaceLink } from '@/features/navigation/places';
import type { UserProfile } from '@/generated/api-types';
import { newActKey } from '@/lib/requests';
import { Avatar } from '@/ui/Avatar';
import { Button } from '@/ui/Button';
import { ErrorAlert } from '@/ui/ErrorState';
import { Markdown } from '@/ui/Markdown';
import { Select } from '@/ui/Select';

/** The displayed clock refreshes once a minute, the precision the page promises. */
const CLOCK_REFRESH_MS = 60_000;

function LocalTime({ zone }: { zone: string }) {
  const [now, setNow] = useState(() => new Date());
  const { i18n } = useTranslation();
  useEffect(() => { const timer = setInterval(() => setNow(new Date()), CLOCK_REFRESH_MS); return () => clearInterval(timer); }, []);
  return <time dateTime={now.toISOString()}>{new Intl.DateTimeFormat(i18n.language, { timeZone: zone, hour: '2-digit', minute: '2-digit', timeZoneName: 'shortOffset' }).format(now)}</time>;
}

export function ProfileMessage({ profile, onNavigate, compact = false }: { profile: UserProfile; onNavigate?: () => void; compact?: boolean }) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const [domainId, setDomainId] = useState(profile.member_of[0]?.id ?? '');
  const [choosing, setChoosing] = useState(false);
  const [attempt, setAttempt] = useState(newActKey);
  const send = useMutation({ mutationFn: async () => {
    if (domainId === '') throw new Error(t('profile.page.messageUnavailable'));
    const domain = await queryClient.query({ ...nodeQuery(domainId), staleTime: 'static' });
    const root = domain.id === domain.org_id ? domain : await queryClient.query({ ...nodeQuery(domain.org_id), staleTime: 'static' });
    if (root.kind !== 'org') throw new Error(t('profile.page.messageUnavailable'));
    const chat = await createChat(root.id, { user_ids: [profile.user.id] }, attempt);
    return { chat, address: root.address };
  }, onSuccess: ({ chat, address }) => { onNavigate?.(); void navigate({ to: '/org/$slug/messages', params: { slug: address }, search: { chat: chat.id } }); } });
  if (compact && profile.member_of.length > 1 && !choosing) return <Button size="sm" onClick={() => setChoosing(true)}>{t('profile.page.message')}</Button>;
  return <>{profile.member_of.length > 1 && <Select label={t('profile.page.messageIn')} value={domainId} disabled={send.isPending} onValueChange={(next) => { setDomainId(next); setAttempt(newActKey()); }} options={profile.member_of.map((org) => ({ value: org.id, ...choiceOption(t, 'dynamic.place', { name: org.name ?? '' }) }))} />}<Button size={compact ? 'sm' : 'md'} pending={send.isPending} disabled={domainId === ''} onClick={() => send.mutate()}>{t('profile.page.message')}</Button>{send.error !== null && <ErrorAlert error={send.error} />}</>;
}

export function Identity({ profile, compact = false, heading = false, hideMessage = false, onNavigate }: { profile: UserProfile; compact?: boolean; heading?: boolean; hideMessage?: boolean; onNavigate?: () => void }) {
  const { t, i18n } = useTranslation();
  const Name = heading ? 'h1' : 'p';
  return <div className="flex min-w-0 flex-col gap-3">
    <div className="flex items-center gap-3"><Avatar text={profile.user.avatar.text} color={profile.user.avatar.color} size={40} /><div className="min-w-0"><Name className="text-lg font-semibold wrap-anywhere">{profile.user.display_name}</Name><p className="flex flex-wrap gap-x-3 gap-y-1 text-sm wrap-anywhere text-text-muted"><span>@{profile.user.username}</span>{profile.pronouns !== '' && <span>{profile.pronouns}</span>}</p></div></div>
    {profile.bio !== '' && <Markdown text={profile.bio} compact={compact} />}
    {profile.local_time !== null && <p className="text-sm text-text-muted"><LocalTime zone={profile.local_time.timezone} /></p>}
    {profile.links.length > 0 && <ul className="flex flex-col gap-1 text-sm">{profile.links.map((link, index) => <li key={`${index}:${link.url}`}><a className="text-accent-text hover:underline" href={link.url} target="_blank" rel="noopener noreferrer">{link.label}</a></li>)}</ul>}
    {profile.member_of.length > 0 && <ul className="flex flex-wrap gap-2 text-sm">{profile.member_of.map((node) => <li key={node.id}><PlaceLink place={node} className="text-accent-text hover:underline">{node.name}</PlaceLink></li>)}</ul>}
    {profile.personal_org !== null && <PlaceLink place={profile.personal_org} className="text-sm text-accent-text hover:underline">{t('profile.page.personalOrganization', { name: profile.personal_org.name })}</PlaceLink>}
    <p className="text-sm text-text-muted">{t('profile.page.joined', { date: new Intl.DateTimeFormat(i18n.language, { year: 'numeric', month: 'long' }).format(new Date(profile.joined_at)) })}</p>
    {profile.is_me ? <Button asChild><a href="/settings/your-page" onClick={onNavigate}>{t('profile.page.edit')}</a></Button> : !hideMessage && profile.can_message && <ProfileMessage profile={profile} onNavigate={onNavigate} />}
  </div>;
}
