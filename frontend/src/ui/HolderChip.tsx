/** Shared HolderChip component (access): a role, a group or a place as a name and a count, with its people listed on request, a page at a time.
 *
 * A row of a page names a role or a group, never the people in it: a role may stand for hundreds. So
 * the chip reads its name and, where it is known, how many people that is; the people are asked for
 * when somebody opens the chip or rests the pointer on it, a page at a time, and a page of a large org
 * is as short as one of a small team. Where the people cannot be asked for, the chip is only a name.
 */
import { type InfiniteData, useInfiniteQuery, useQueryClient, type UseInfiniteQueryOptions } from '@tanstack/react-query';
import { type ReactNode, useRef } from 'react';
import { useTranslation } from 'react-i18next';

import type { UserRefPage } from '@/generated/api-types';
import { cx } from '@/ui/cx';
import { ErrorState } from '@/ui/ErrorState';
import { Pagination } from '@/ui/Pagination';
import { Popover } from '@/ui/Popover';
import { SkeletonRows } from '@/ui/Skeleton';
import { UserChip } from '@/ui/UserChip';

/** Where the people behind a chip are read: a query for pages of people, the cursor of the page before being what the next asks with. */
export type PeopleSource = UseInfiniteQueryOptions<UserRefPage, Error, InfiniteData<UserRefPage, string | null>, readonly unknown[], string | null>;

/** How long the pointer rests on a chip before its people are fetched: long enough not to ask for every chip it crosses. */
const REST_MS = 300;

export interface HolderChipProps {
  /** The name of the role, the group or the place. */
  name: string;
  /** How many people that is, where it is known. */
  count?: number | undefined;
  /** The mark of the kind of holder it is. */
  icon?: ReactNode;
  /** Where its people are asked for; the chip is only a name without it. */
  people?: PeopleSource | undefined;
  /** Drawn quietly: a holder that the page follows from a place and does not name itself. */
  inherited?: boolean;
  /** The act is under way: the chip is dimmed until it is answered. */
  pending?: boolean;
  /** Draws no frame of its own: the chip stands inside one that holds more than the chip, such as a cross to take it out. */
  bare?: boolean;
  /** What stands after the name inside the chip: where it comes from, until when. */
  note?: ReactNode;
  className?: string;
}

const CHIP = 'inline-flex h-(--control-sm) max-w-full min-w-0 items-center gap-1 rounded-full border px-2 text-sm';

function Look({ name, count, icon, note }: Pick<HolderChipProps, 'name' | 'count' | 'icon' | 'note'>) {
  return (
    <>
      {icon !== undefined && <span className="flex shrink-0 text-text-muted [&_svg]:size-[0.875rem]">{icon}</span>}
      <span className="min-w-0 truncate font-medium">{name}</span>
      {count !== undefined && <span className="shrink-0 text-text-muted tabular-nums">· {count}</span>}
      {note !== undefined && <span className="min-w-0 truncate text-text-muted">{note}</span>}
    </>
  );
}

/** The people behind a chip, read once the card is open. */
function People({ name, source }: { name: string; source: PeopleSource }) {
  const { t } = useTranslation();
  const list = useInfiniteQuery(source);
  const people = list.data?.pages.flatMap((page) => page.items) ?? [];
  if (list.isPending) return <SkeletonRows rows={3} />;
  if (list.isError && people.length === 0) {
    return <ErrorState compact error={list.error} title={t('access.holder.notLoaded')} retrying={list.isRefetching} onRetry={() => void list.refetch()} />;
  }
  return (
    <div className="flex flex-col gap-1">
      <p className="text-xs text-text-muted">
        {list.hasNextPage ? t('access.holder.peopleSoFar', { name, count: people.length }) : t('access.holder.people', { name, count: people.length })}
      </p>
      {people.length === 0 ? (
        <p className="text-sm text-text-muted">{t('access.holder.nobody')}</p>
      ) : (
        <ul className="flex max-h-[16rem] flex-col gap-1 overflow-y-auto">
          {people.map((person) => (
            <li key={person.id}>
              <UserChip user={person} size={24} showUsername />
            </li>
          ))}
        </ul>
      )}
      <Pagination
        hasMore={list.hasNextPage}
        loading={list.isFetchingNextPage}
        error={list.isFetchNextPageError ? list.error : undefined}
        onMore={() => void list.fetchNextPage()}
        className="py-1"
      />
    </div>
  );
}

export function HolderChip({ name, count, icon, people, inherited = false, pending = false, bare = false, note, className }: HolderChipProps) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const resting = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const look = bare
    ? cx('inline-flex h-full max-w-full min-w-0 items-center gap-1 rounded-full pr-1 pl-2 text-sm text-text', className)
    : cx(CHIP, inherited ? 'border-border bg-bg-subtle text-text' : 'border-border-strong bg-surface text-text', pending && 'opacity-60', className);

  if (people === undefined) {
    return (
      <span className={look}>
        <Look name={name} count={count} icon={icon} note={note} />
      </span>
    );
  }

  const prefetch = () => {
    resting.current = setTimeout(() => void queryClient.infiniteQuery(people as never).catch(() => undefined), REST_MS);
  };
  return (
    <Popover label={t('access.holder.card', { name })} className="w-(--menu-max-width) [--floating-width:var(--menu-max-width)]" trigger={
        <button
          type="button"
          aria-label={t('access.holder.open', { name })}
          className={cx(look, bare ? 'hover:bg-bg-muted' : 'hover:border-text-subtle')}
          onPointerEnter={prefetch}
          onPointerLeave={() => clearTimeout(resting.current)}
        >
          <Look name={name} count={count} icon={icon} note={note} />
        </button>
      }>
          <People name={name} source={people} />
    </Popover>
  );
}
