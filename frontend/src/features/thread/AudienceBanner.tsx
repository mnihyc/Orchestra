/** One live audience notice for the main thread and its side conversation. */
import { useQuery } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';
import { useStore } from 'zustand';
import type { TFunction } from 'i18next';
import { pageQuery, permissionsQuery, type AccessTarget } from '@/api/access';
import { client } from '@/api/client';
import { settingsQuery } from '@/api/settings';
import { openAccess } from '@/features/access/AccessDialog';
import { holderName, peopleOf } from '@/features/access/holders';
import { indexCatalogue } from '@/features/access/model';
import { PersonThingProvider } from '@/features/people/personContext';
import type { Thread } from '@/generated/api-types';
import { formatNumber } from '@/lib/format';
import { useRunningClock } from '@/lib/time';
import { useViewChoice } from '@/lib/viewChoices';
import { Banner } from '@/ui/Banner';
import { Button } from '@/ui/Button';
import { HolderChip } from '@/ui/HolderChip';
import { PeopleList } from '@/ui/PeopleList';
import { Popover } from '@/ui/Popover';
import { audienceDismissal, audienceExpires, audienceGrew, threadAudience, type AudienceHolder } from './audience';

function words(holders: readonly AudienceHolder[], t: TFunction, locale: string): string {
  const kinds = [...new Set(holders.map(({ holder }) => holder.kind))];
  const named = kinds.map((kind) => {
    const group = holders.filter(({ holder }) => holder.kind === kind);
    const first = group[0]?.holder;
    const count = group.length, number = formatNumber(count, locale);
    if (kind === 'user') return t('thread.audience.people', { count, number });
    if (first?.kind === 'role' && count === 1) return t(first.role === 'viewer' && first.at === null ? 'access.holder.everyoneHere' : `access.holder.roles.${first.role}`);
    if (first?.kind === 'group' && count === 1) return first.group.name;
    if (kind === 'everyone') return t('access.holder.everyone');
    return t(`thread.audience.kinds.${kind}`, { count, number });
  });
  return new Intl.ListFormat(locale, { style: 'short', type: 'conjunction' }).format(named);
}

export function AudienceBanner({ thread }: { thread: Pick<Thread, 'id' | 'name' | 'owner'> }) {
  const { t, i18n } = useTranslation();
  const reader = useStore(client.session, (state) => state.userId);
  const target: AccessTarget = { kind: 'thread', id: thread.id, name: thread.name };
  // Main, inline and panel surfaces observe the same page. A new surface does not
  // repeat a completed read; current page updates and explicit invalidation still apply.
  const page = useQuery({ ...pageQuery(target), retry: false, refetchOnMount: false });
  const definitions = useQuery({ ...permissionsQuery, refetchOnMount: false });
  const now = useRunningClock(page.data !== undefined && audienceExpires(page.data));
  const known = page.data !== undefined && definitions.data !== undefined && page.data.object.id === thread.id;
  const owner = page.data?.owner?.id ?? thread.owner.id;
  const holders = page.data !== undefined && definitions.data !== undefined && page.data.object.id === thread.id
    ? threadAudience(page.data, indexCatalogue(definitions.data), owner, now) : [];
  const settings = useQuery({ ...settingsQuery(thread.id), enabled: known && holders.length > 0 && !page.isError && !definitions.isError, retry: false, refetchOnMount: false });
  const [dismissed, dismiss] = useViewChoice<string | null>('thread', 'audience-dismissed', null, thread.id);
  const readsOn = !settings.isError && settings.data?.find((row) => row.key === 'orch_tools.owner_reads')?.value === true;
  const share = <Button variant="ghost" size="sm" className="shrink-0" onClick={() => openAccess(target, { share: true })}>{t('thread.audience.sharing')}</Button>;
  if (page.isError || definitions.isError) {
    return <Banner tone="warning" action={<Button variant="ghost" size="sm" onClick={() => { void page.refetch(); void definitions.refetch(); }}>{t('common.tryAgain')}</Button>}
      className="shrink-0 [&>p]:truncate" >{t('thread.audience.unavailable')}</Banner>;
  }
  if (!known || holders.length === 0 || !audienceGrew(holders, dismissed)) return null;
  const senders = holders.filter((holder) => holder.sends);
  const shared = t('thread.audience.shared', { audience: words(holders, t, i18n.language) });
  const summary = senders.length === 0 ? shared : `${shared} · ${t('thread.audience.senders', { audience: words(senders, t, i18n.language) })}`;
  const personal = t(reader === owner ? 'thread.audience.personal' : 'thread.audience.owner');
  return <Banner className="shrink-0" action={share} onDismiss={() => dismiss(audienceDismissal(holders))}>
    <span className="flex min-w-0 items-center gap-x-1 overflow-hidden whitespace-nowrap" data-thread-audience>
      {/* The card takes focus itself, so that opening it does not focus its first person and open that person's own card. */}
      <Popover label={t('thread.audience.title')} className="w-[min(20rem,calc(100vw-1rem))] p-2"
        onOpenAutoFocus={(event) => { event.preventDefault(); if (event.currentTarget instanceof HTMLElement) event.currentTarget.focus(); }}
        trigger={<button type="button" className="focus-inset max-w-full truncate rounded-sm font-medium underline decoration-dotted underline-offset-2">{summary}</button>}>
        <PersonThingProvider thing={{ kind: 'thread', id: thread.id }}><div className="flex min-w-0 flex-col gap-2">
          {[false, true].map((sends) => {
            const group = holders.filter((one) => one.sends === sends);
            const users = group.flatMap(({ holder }) => holder.kind === 'user' ? [holder.user] : []);
            const groups = group.filter((one) => one.holder.kind !== 'user');
            if (users.length === 0 && groups.length === 0) return null;
            return <div key={String(sends)} className="flex min-w-0 flex-col gap-1">
              <span className="text-xs text-text-muted">{t(sends ? 'thread.audience.canSend' : 'thread.audience.canView')}</span>
              {users.length > 0 && <PeopleList users={users} />}
              {groups.map(({ key, holder }) => <HolderChip key={key} bare wrap name={holderName(t, holder, target.name)} people={peopleOf(holder, target)} />)}
            </div>;
          })}
        </div></PersonThingProvider>
        {readsOn && <p className="mt-2 text-xs text-text-muted">{personal}</p>}
      </Popover>
      {readsOn && <><span aria-hidden>·</span><span className="min-w-0 truncate">{personal}</span></>}
    </span>
  </Banner>;
}
