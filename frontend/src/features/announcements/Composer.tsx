/** Scope, category, target activity, delivery, fallback and wake choices.
 */
import { useInfiniteQuery, useMutation, useQueryClient } from '@tanstack/react-query';
import { Megaphone } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { EmptyState } from '@/ui/EmptyState';
import { announce, announcementsQuery, applyAnnouncements } from '@/api/announcements';
import { useHelpOpener } from '@/features/help/open';
import { Hint } from '@/features/help/Hint';
import type { AnnouncementCreateBody, AnnouncementDelivery, AnnouncementFallback, Org, Project, Topic } from '@/generated/api-types';
import { holds } from '@/lib/permissions';
import { Button } from '@/ui/Button';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { Input } from '@/ui/Input';
import { IconButton } from '@/ui/IconButton';
import { RelativeTime } from '@/ui/RelativeTime';
import { Select } from '@/ui/Select';
import { SkeletonRows } from '@/ui/Skeleton';
import { Textarea } from '@/ui/Textarea';
import { AnnouncementPreview } from './Preview';
import { AnnouncementResult, DeliveryCounts } from './Result';

type Place = Org | Project | Topic;
type AnnouncementChoice = 'notice' | 'milestone' | 'info' | 'important' | 'all' | 'active' | 'default' | 'next_turn' | 'steer' | 'interrupt' | 'yes' | 'no';
/** The announcement contract bounds a title to one short line and its body to a brief notice. */
const TITLE_MAX_CHARS = 120;
const BODY_MAX_CHARS = 1500;

export function PlaceAnnouncements({ place, compact = false }: { place: Place; compact?: boolean }) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  return <>{compact ? <IconButton size="sm" label={t('placeAnnouncements.title')} icon={<Megaphone aria-hidden />} onClick={() => setOpen(true)} />
    : <Button variant="ghost" icon={<Megaphone aria-hidden />} onClick={() => setOpen(true)}>{t('placeAnnouncements.title')}</Button>}
    {open && <Dialog open onOpenChange={setOpen} title={t('placeAnnouncements.at', { name: place.name })}><Contents place={place} /></Dialog>}</>;
}

function Contents({ place }: { place: Place }) {
  const { t } = useTranslation();
  const openGuide = useHelpOpener();
  const client = useQueryClient();
  const [title, setTitle] = useState('');
  const [body, setBody] = useState('');
  const [category, setCategory] = useState<'notice' | 'milestone'>('notice');
  const [severity, setSeverity] = useState<'info' | 'important'>('info');
  const [targets, setTargets] = useState<'all' | 'active'>('all');
  const [delivery, setDelivery] = useState<AnnouncementDelivery | 'default'>('default');
  const [fallback, setFallback] = useState<AnnouncementFallback | 'default'>('default');
  const [wake, setWake] = useState<'default' | 'yes' | 'no'>('default');
  const list = useInfiniteQuery(announcementsQuery(place.id));
  const send = useMutation({ mutationFn: (request: AnnouncementCreateBody) => announce(place.id, request), onSuccess: async (reply) => {
    await applyAnnouncements(client, place.id, reply);
    setTitle(''); setBody('');
  } });
  const canSend = !place.archived && holds(place.access, `${place.kind}.interact`);
  const rows = list.data?.pages.flatMap((page) => page.items) ?? [];
  const choice = <Value extends AnnouncementChoice>(value: Value) => ({ value,
    name: t(`inventoryChoices.announcements.${value}.name`), description: t(`inventoryChoices.announcements.${value}.description`),
    ...(value === 'default' ? { help: { label: t('inventoryChoices.help.open', { name: t('inventoryChoices.announcements.default.name') }), onOpen: () => openGuide('settings-and-usage', 'inherited-settings') } }
      : value === 'notice' || value === 'milestone' ? { help: { label: t('inventoryChoices.help.open', { name: t(`inventoryChoices.announcements.${value}.name`) }), onOpen: () => openGuide('notices-and-announcements', 'send-an-announcement') } } : {}),
  });
  const useDefault = choice('default');
  return <div className="flex flex-col gap-4">
    <Hint id="announcements" popover className="self-start" />
    <AnnouncementPreview name={place.name} />
    {canSend && <form className="flex flex-col gap-3" onSubmit={(event) => { event.preventDefault(); send.mutate({ title: title.trim(), body, category, severity, targets,
      ...(delivery === 'default' ? {} : { delivery }), ...(fallback === 'default' ? {} : { fallback }), ...(wake === 'default' ? {} : { wake_idle: wake === 'yes' }) }); }}>
      <Input label={t('placeAnnouncements.heading')} value={title} maxLength={TITLE_MAX_CHARS} required disabled={send.isPending} onChange={(event) => setTitle(event.target.value)} />
      <Textarea label={t('placeAnnouncements.body')} value={body} maxLength={BODY_MAX_CHARS} disabled={send.isPending} onChange={(event) => setBody(event.target.value)} />
      <Select label={t('placeAnnouncements.category')} value={category} onValueChange={setCategory} options={[choice('notice'), choice('milestone')]} />
      <Select label={t('placeAnnouncements.severity')} value={severity} onValueChange={setSeverity} options={[choice('info'), choice('important')]} />
      <Select label={t('placeAnnouncements.targets')} value={targets} onValueChange={setTargets} options={[choice('all'), choice('active')]} />
      <Select label={t('placeAnnouncements.delivery')} value={delivery} onValueChange={setDelivery} options={[useDefault, choice('next_turn'), choice('steer'), choice('interrupt')]} />
      <Select label={t('placeAnnouncements.fallback')} value={fallback} onValueChange={setFallback} options={[useDefault, choice('next_turn'), choice('interrupt')]} />
      <Select label={t('placeAnnouncements.wake')} value={wake} onValueChange={setWake} options={[useDefault, choice('yes'), choice('no')]} />
      {send.error !== null && <ErrorAlert error={send.error} />}
      <Button type="submit" variant="primary" pending={send.isPending} disabled={title.trim() === ''}>{t('placeAnnouncements.send')}</Button>
    </form>}
    {send.data !== undefined && <AnnouncementResult reply={send.data} />}
    <h3 className="font-medium">{t('placeAnnouncements.history')}</h3>
    {list.isPending ? <SkeletonRows rows={2} /> : list.isError ? <ErrorState compact error={list.error} onRetry={() => void list.refetch()} /> : rows.length === 0 ? <EmptyState compact title={t('placeAnnouncements.empty')} /> : <ul className="flex flex-col gap-3">{rows.map((row) => <li key={row.id} className="flex flex-col gap-1 border-t border-border pt-3"><h4 className="font-medium">{row.title}</h4><p className="whitespace-pre-wrap wrap-anywhere">{row.body}</p><DeliveryCounts counts={row.delivered} /><RelativeTime at={row.created_at} /></li>)}</ul>}
    {list.hasNextPage && <Button pending={list.isFetchingNextPage} onClick={() => void list.fetchNextPage()}>{t('placeAnnouncements.more')}</Button>}
  </div>;
}

/** A page menu opens this same lazy reader/composer without another header trigger. */
export { Contents as PlaceAnnouncementContents };
