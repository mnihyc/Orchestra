/** One live audience notice for the main thread and its side conversation. */
import { useQuery } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';
import { useStore } from 'zustand';
import type { TFunction } from 'i18next';
import { pageQuery, permissionsQuery, type AccessTarget } from '@/api/access';
import { client } from '@/api/client';
import { openAccess } from '@/features/access/AccessDialog';
import { holderName, peopleOf } from '@/features/access/holders';
import { indexCatalogue } from '@/features/access/model';
import { PersonThingProvider } from '@/features/people/personContext';
import type { Thread } from '@/generated/api-types';
import { formatNumber } from '@/lib/format';
import { useRunningClock } from '@/lib/time';
import { Banner } from '@/ui/Banner';
import { Button } from '@/ui/Button';
import { HolderChip } from '@/ui/HolderChip';
import { PeopleList } from '@/ui/PeopleList';
import { Popover } from '@/ui/Popover';
import { audienceExpires, threadAudience, type AudienceHolder } from './audience';

function words(holders: readonly AudienceHolder[], target: AccessTarget, t: TFunction, locale: string): string {
  const people = holders.filter(({ holder }) => holder.kind === 'user').length;
  const named = holders.filter(({ holder }) => holder.kind !== 'user').map(({ holder }) => holderName(t, holder, target.name, true));
  if (people > 0) named.unshift(t('thread.audience.people', { count: people, number: formatNumber(people, locale) }));
  return new Intl.ListFormat(locale, { style: 'short', type: 'conjunction' }).format(named);
}

export function AudienceBanner({ thread }: { thread: Pick<Thread, 'id' | 'name' | 'owner'> }) {
  const { t, i18n } = useTranslation();
  const reader = useStore(client.session, (state) => state.userId);
  const target: AccessTarget = { kind: 'thread', id: thread.id, name: thread.name };
  const page = useQuery({ ...pageQuery(target), retry: false });
  const definitions = useQuery(permissionsQuery);
  const now = useRunningClock(page.data !== undefined && audienceExpires(page.data));
  const share = <Button variant="ghost" size="sm" onClick={() => openAccess(target, { share: true })}>{t('thread.audience.sharing')}</Button>;
  if (page.isError || definitions.isError) {
    return <Banner tone="warning" action={<Button variant="ghost" size="sm" onClick={() => { void page.refetch(); void definitions.refetch(); }}>{t('common.tryAgain')}</Button>}
      className="shrink-0" >{t('thread.audience.unavailable')}</Banner>;
  }
  if (page.data === undefined || definitions.data === undefined || page.data.object.id !== thread.id) {
    return <Banner className="shrink-0">{t('thread.audience.loading')}</Banner>;
  }
  const catalogue = indexCatalogue(definitions.data);
  const owner = page.data.owner?.id ?? thread.owner.id;
  const holders = threadAudience(page.data, catalogue, owner, now);
  if (holders.length === 0) return null;
  const senders = holders.filter((holder) => holder.sends);
  const shared = t('thread.audience.shared', { audience: words(holders, target, t, i18n.language) });
  const summary = senders.length === 0 ? shared : `${shared} · ${t('thread.audience.senders', { audience: words(senders, target, t, i18n.language) })}`;
  return <Banner className="shrink-0" action={share}>
    <span className="flex min-w-0 flex-wrap items-center gap-x-1" data-thread-audience>
      <Popover label={t('thread.audience.title')} className="w-[min(20rem,calc(100vw-1rem))] p-2"
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
              {groups.map(({ key, holder }) => <HolderChip key={key} bare name={holderName(t, holder, target.name)} people={peopleOf(holder, target)} />)}
            </div>;
          })}
        </div></PersonThingProvider>
      </Popover>
      <span aria-hidden>·</span><span>{t(reader === owner ? 'thread.audience.personal' : 'thread.audience.owner')}</span>
    </span>
  </Banner>;
}
