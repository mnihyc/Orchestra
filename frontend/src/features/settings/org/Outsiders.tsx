/** From outside: the people who are no members of this tree and reach something of the org now, each way with who gave it and until when, and Take out everywhere here by a plan read whole.
 *
 * For whoever manages the org's members. Each person shows the things they reach, by name or through
 * a group, an org's members or a role of another place, and when they were last seen here; nothing of
 * where else they sign in or work, and no other member of a group. Taking somebody out first reads the
 * server's complete plan: the rows that name them go, and where a group or a role still lets them in,
 * the existing suspension holds them out of this org instead. It removes no other group member and
 * nothing outside this org. A plan that changed, ran out or that the reader may not carry out does
 * nothing, and says what to do.
 */
import { useInfiniteQuery, useMutation, useQuery } from '@tanstack/react-query';
import { DoorOpen, Search, UserX } from 'lucide-react';
import type { TFunction } from 'i18next';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { ApiError } from '@/api/client';
import { outsiderSources, outsidersQuery, removeOutsider, wholeOutsiderRemoval } from '@/api/directory';
import type { Org, OutsidePerson, OutsideRemovalPlan, OutsideSource } from '@/generated/api-types';
import { useDebounced } from '@/lib/debounce';
import { permissionLabel } from '@/lib/permissions';
import { boxClasses, Section } from '@/shell/Page';
import { Badge } from '@/ui/Badge';
import { Button } from '@/ui/Button';
import { Dialog } from '@/ui/Dialog';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorAlert, ErrorState, MissingAccessList } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';
import { Input } from '@/ui/Input';
import { Pagination } from '@/ui/Pagination';
import { RelativeTime } from '@/ui/RelativeTime';
import { SkeletonRows } from '@/ui/Skeleton';
import { Textarea } from '@/ui/Textarea';
import { toast } from '@/ui/Toast';

/** What a holder of a way is, read from the row as the server named it. */
interface HolderShown {
  kind?: string;
  name?: string | null;
  role?: string | null;
  org?: { name?: string | null } | null;
  at?: { name?: string | null } | null;
}

/** Unknown role words remain the server's own; they never acquire a different role's label. */
function roleWord(value: string, t: TFunction): string {
  return value === 'viewer' || value === 'interactor' || value === 'developer' || value === 'admin' ? t(`roles.${value}`) : value;
}

/** The row of a way in words: a permission of a page, or a role given at a place. */
function useRow() {
  const { t } = useTranslation();
  return (source: OutsideSource) =>
    source.row.startsWith('role:') ? t('settings.outside.role', { role: roleWord(source.row.slice('role:'.length), t) }) : permissionLabel(source.row);
}

/** How a way names the person: by name, or through what they belong to. */
function useThrough() {
  const { t } = useTranslation();
  return (source: OutsideSource) => {
    const holder = (source.holder ?? {}) as HolderShown;
    switch (holder.kind) {
      case 'group':
        return t('settings.outside.throughGroup', { name: holder.name ?? '' });
      case 'members':
        return t('settings.outside.throughMembers', { org: holder.org?.name ?? '' });
      case 'role':
        return t('settings.outside.throughRole', { role: roleWord(holder.role ?? '', t), place: holder.at?.name ?? '' });
      default:
        return t('settings.outside.byName');
    }
  };
}

/** One way a person from outside reaches a thing here, with who gave it and until when. */
function SourceLine({ source, removing }: { source: OutsideSource; removing?: boolean }) {
  const { t } = useTranslation();
  const row = useRow();
  const through = useThrough();
  return (
    <li className="flex flex-col gap-1 py-2 text-sm">
      <span className="flex flex-wrap items-center gap-x-2 gap-y-1">
        <span className="font-medium wrap-anywhere text-text">{source.object.name}</span>
        <span className="text-text-muted">{t(`settings.outside.kinds.${source.object.kind}`, { defaultValue: source.object.kind })}</span>
        <span>{row(source)}</span>
        {removing === true && (source.direct ? <Badge tone="danger">{t('settings.outside.goes')}</Badge> : <Badge tone="warning">{t('settings.outside.stays')}</Badge>)}
      </span>
      <span className="flex flex-wrap gap-x-3 text-text-muted">
        <span>{through(source)}</span>
        <span>{source.by === null ? t('settings.outside.givenSeeded') : t('settings.outside.givenBy', { name: source.by.display_name })}</span>
        <span>{source.until === null ? t('settings.outside.noEnd') : t('settings.outside.until')}</span>
        {source.until !== null && <RelativeTime at={source.until} />}
      </span>
    </li>
  );
}

/** The ways of one person, with the rest of them asked for on Show more. */
function Sources({ org, person }: { org: Org; person: OutsidePerson }) {
  const { t } = useTranslation();
  const [more, setMore] = useState<OutsideSource[]>([]);
  const [cursor, setCursor] = useState<string | null>(person.sources_cursor);
  const next = useMutation({
    mutationFn: (from: string) => outsiderSources(org.id, person.user.id, from),
    onSuccess: (page) => {
      setMore((shown) => [...shown, ...page.items]);
      setCursor(page.next_cursor);
    },
  });
  return (
    <div className="flex flex-col">
      <ul className="divide-y divide-border">
        {[...person.sources, ...more].map((source, index) => (
          // A way is a row of the server's own list: given whole and in its order.
          <SourceLine key={index} source={source} />
        ))}
      </ul>
      {cursor !== null && (
        <div>
          <Button size="sm" variant="ghost" pending={next.isPending} onClick={() => next.mutate(cursor)}>
            {t('settings.outside.showMore')}
          </Button>
        </div>
      )}
      {next.error !== null && <ErrorAlert error={next.error} />}
    </div>
  );
}

/** Take out everywhere here: the complete plan first, then one confirmation with a reason. */
function TakeOut({ org, person, onClose }: { org: Org; person: OutsidePerson; onClose: () => void }) {
  const { t } = useTranslation();
  const [reason, setReason] = useState('');
  const name = person.user.display_name;
  // The plan is read whole when the dialog opens, and again on Review again; it is never kept for another opening.
  const read = useQuery({
    queryKey: ['outside-removal', org.id, person.user.id] as const,
    queryFn: () => wholeOutsiderRemoval(org.id, person.user.id),
    staleTime: Infinity,
    gcTime: 0,
    retry: false,
  });
  const plan = read.data ?? null;
  const take = useMutation({
    mutationFn: (whole: OutsideRemovalPlan) => removeOutsider(org.id, person.user.id, whole, reason),
    onSuccess: (done) => {
      toast({
        tone: 'success',
        title: done.suspension === null ? t('settings.outside.takenOut', { name }) : t('settings.outside.takenOutHeld', { name }),
      });
      onClose();
    },
  });
  const stale = take.error instanceof ApiError && (take.error.is('conflict.stale_revision') || take.error.is('preflight.expired'));
  const complete = plan !== null && plan.reviewed_fingerprint !== null && plan.next_cursor === null;
  const blocked = !complete || plan.missing.length > 0 || stale;
  return (
    <Dialog
      open
      onOpenChange={(next) => !next && !take.isPending && onClose()}
      title={t('settings.outside.takeOutTitle', { name, org: org.name })}
      description={t('settings.outside.takeOutLead', { name, org: org.name })}
      dismissible={!take.isPending}
      footer={
        <>
          <Button disabled={take.isPending} onClick={onClose}>
            {t('common.cancel')}
          </Button>
          <Button variant="danger" pending={take.isPending} blocked={blocked} onClick={() => plan !== null && take.mutate(plan)}>
            {t('settings.outside.takeOutConfirm', { name })}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-4 py-2">
        {read.isPending ? (
          <SkeletonRows rows={3} />
        ) : read.isError ? (
          <ErrorState compact error={read.error} retrying={read.isRefetching} onRetry={() => void read.refetch()} />
        ) : plan !== null ? (
          <>
            <ul className={`${boxClasses} divide-y divide-border px-3`}>
              {plan.removes.map((source, index) => (
                <SourceLine key={index} source={source} removing />
              ))}
            </ul>
            {plan.suspension_needed && <InlineAlert tone="warning">{t('settings.outside.suspensionNeeded', { name, org: org.name })}</InlineAlert>}
            <p className="text-sm text-text-muted">{t('settings.outside.nothingElse')}</p>
            {plan.missing.length > 0 && (
              <InlineAlert tone="danger" title={t('settings.outside.missing')}>
                <MissingAccessList missing={plan.missing} />
              </InlineAlert>
            )}
            <Textarea label={t('settings.outside.reason')} hint={t('settings.outside.reasonHint')} rows={2} value={reason} onChange={(event) => setReason(event.target.value)} />
          </>
        ) : null}
        {take.error !== null &&
          (stale ? (
            <InlineAlert
              tone="warning"
              action={
                <Button
                  size="sm"
                  pending={read.isRefetching}
                  onClick={() => {
                    take.reset();
                    void read.refetch();
                  }}
                >
                  {t('settings.outside.reviewAgain')}
                </Button>
              }
            >
              {t('settings.outside.stale')}
            </InlineAlert>
          ) : (
            <ErrorAlert error={take.error} />
          ))}
      </div>
    </Dialog>
  );
}

export function Outsiders({ org }: { org: Org }) {
  const { t } = useTranslation();
  const [q, setQ] = useState('');
  const settled = useDebounced(q);
  const [taking, setTaking] = useState<OutsidePerson | null>(null);
  const list = useInfiniteQuery(outsidersQuery(org.id, settled));
  const people = list.data?.pages.flatMap((page) => page.items) ?? [];
  return (
    <Section id="outsiders" title={t('settings.outside.title')} lead={t('settings.outside.lead', { org: org.name })}>
      <Input
        label={t('settings.outside.search')}
        hideLabel
        type="search"
        placeholder={t('settings.outside.searchPlaceholder')}
        value={q}
        onChange={(event) => setQ(event.target.value)}
        fieldClassName="min-w-0 sm:max-w-(--container-form)"
        autoComplete="off"
      />
      {list.isPending ? (
        <SkeletonRows rows={3} />
      ) : list.isError && people.length === 0 ? (
        <ErrorState compact error={list.error} retrying={list.isRefetching} onRetry={() => void list.refetch()} />
      ) : people.length === 0 ? (
        <EmptyState compact icon={settled === '' ? <DoorOpen aria-hidden /> : <Search aria-hidden />} title={settled === '' ? t('settings.outside.none') : t('settings.outside.noMatch', { q: settled })} />
      ) : (
        <div className="flex flex-col gap-2">
          <ul className={`${boxClasses} divide-y divide-border`} aria-label={t('settings.outside.title')}>
            {people.map((person) => (
              <li key={person.user.id} className="flex flex-col gap-2 px-4 py-3">
                <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
                  <span className="font-medium text-text">{person.user.display_name}</span>
                  {person.suspended && <Badge tone="warning">{t('places.members.suspended')}</Badge>}
                  <span className="text-sm text-text-muted">
                    {t('settings.outside.lastSeen')}{' '}
                    {person.last_seen_at === null ? t('settings.members.lastSeenUnknown') : <RelativeTime at={person.last_seen_at} />}
                  </span>
                  <span className="flex-1" />
                  <Button size="sm" variant="danger-ghost" icon={<UserX aria-hidden />} onClick={() => setTaking(person)}>
                    {t('settings.outside.takeOut')}
                  </Button>
                </div>
                <Sources org={org} person={person} />
              </li>
            ))}
          </ul>
          <Pagination hasMore={list.hasNextPage} loading={list.isFetchingNextPage} error={list.isFetchNextPageError ? list.error : undefined} onMore={() => void list.fetchNextPage()} />
        </div>
      )}
      {taking !== null && <TakeOut org={org} person={taking} onClose={() => setTaking(null)} />}
    </Section>
  );
}
