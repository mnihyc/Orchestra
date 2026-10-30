/** A holder of a row as a chip: a person with their avatar, a role, a group or a place by its name, where it comes from, and the cross that takes it out.
 *
 * Names follow the words people use: "Everyone in Fix login", "Developers", "Developers of Globex",
 * "The work in Web". A role, a group or a place never lists its people: it reads its name and, where
 * the page knows it, how many, and opens its people on request. Where the people cannot be asked
 * for, it is only a name.
 */
import { Bot, Globe, Network, ShieldCheck, Users, X } from 'lucide-react';
import type { TFunction } from 'i18next';
import type { ReactNode } from 'react';
import { useTranslation } from 'react-i18next';

import { type AccessRowTarget, holderParam, peopleQuery } from '@/api/access';
import { choiceOption } from '@/i18n/choices';
import { usePlaceLabel } from '@/features/navigation/placeLabels';
import type { Entry, Holder, NodeOrSystem } from '@/generated/api-types';
import { Avatar } from '@/ui/Avatar';
import { cx } from '@/ui/cx';
import { HolderChip, type PeopleSource } from '@/ui/HolderChip';
import { Tooltip } from '@/ui/Tooltip';

/** What a place is called where the page names it: the system above all orgs has no name of its own. */
export function placeName(t: TFunction, place: NodeOrSystem | null): string {
  if (place === null) return t('access.place.none');
  if (place === 'system') return t('access.place.system');
  return place.name ?? t('access.place.hidden');
}

/** The name of a holder in the words of the page. `thing` is what the page is of, for a role held there. */
export function holderName(t: TFunction, holder: Holder, thing: string, compact = false, label?: ReturnType<typeof usePlaceLabel>): string {
  switch (holder.kind) {
    case 'role': {
      const place = label?.(holder.at) ?? holder.place ?? (holder.at === null ? thing : (holder.at.name ?? t('access.place.hidden')));
      if (holder.role === 'viewer') return compact && holder.at === null ? t('access.holder.everyoneHere') : t('access.holder.everyoneIn', { place });
      const roles = t(`access.holder.roles.${holder.role}`);
      return t('access.holder.roleOf', { roles, place });
    }
    case 'members':
      return t('access.holder.membersOf', { place: label?.(holder.node) ?? holder.node.name ?? t('access.place.hidden') });
    case 'group':
      return `${holder.group.name}, ${label?.({id:holder.group.org_id,name:holder.group.place ?? null}) ?? holder.group.place ?? t('access.place.hidden')}`;
    case 'user':
      return holder.user.display_name;
    case 'node':
      return t('access.holder.workIn', { place: label?.(holder.node) ?? holder.node.name ?? t('access.place.hidden') });
    case 'agents':
      return t('access.holder.agentsOf', { place: label?.(holder.node) ?? holder.node.name ?? t('access.place.hidden') });
    case 'everyone':
      return t('access.holder.everyone');
  }
}

const MARKS = { role: ShieldCheck, group: Users, members: Users, node: Network, agents: Bot, everyone: Globe } as const;

/** A day as people write it in a few words: "3 May", its parts held together by a space that does not break, so that a narrow cell never splits a date. */
export function shortDate(locale: string, iso: string): string {
  return new Intl.DateTimeFormat(locale, { day: 'numeric', month: 'short' }).format(new Date(iso)).replace(/ /g, '\u00a0');
}

/**
 * Who gave an entry and when, in a few words ("by Bo, 3 May"), or at length for the title of a chip; `null`
 * where nobody is named as having given it, as with what the system shipped.
 */
export function gaveText(t: TFunction, locale: string, entry: Pick<Entry, 'created_by' | 'created_at'>, long = false): string | null {
  if (entry.created_by === null) return null;
  return t(long ? 'access.holder.gaveLong' : 'access.holder.gave', { name: entry.created_by.display_name, date: shortDate(locale, entry.created_at) });
}

/** The people behind a holder, asked for when the chip is opened. */
export function peopleOf(holder: Holder, target: AccessRowTarget): PeopleSource | undefined {
  // A role on system defaults applies separately in each following place, not to one system membership.
  if (target.kind === 'system') return undefined;
  const param = holderParam(holder);
  return param === null ? undefined : (peopleQuery(target, param) as unknown as PeopleSource);
}

/** The cross at the end of a chip. */
export function RemoveButton({ label, onRemove, disabled = false, tip }: { label: string; onRemove: () => void; disabled?: boolean; tip?: string | undefined }) {
  const button = (
    <button
      type="button"
      aria-label={label}
      aria-disabled={disabled || undefined}
      className={cx(
        'mr-1 inline-flex size-6 shrink-0 items-center justify-center rounded-full transition-colors',
        disabled ? 'cursor-not-allowed text-text-disabled' : 'text-text-muted hover:bg-bg-muted hover:text-text',
      )}
      onClick={(event) => {
        if (disabled) {
          event.preventDefault();
          return;
        }
        onRemove();
      }}
    >
      <X size={16} aria-hidden />
    </button>
  );
  return tip === undefined ? button : <Tooltip content={tip}>{button}</Tooltip>;
}

export interface HolderViewProps {
  holder: Holder;
  /** What the page is of. */
  target: AccessRowTarget;
  /** Where it comes from, until when: after the name, quietly. */
  note?: ReactNode;
  /** Followed from a place and not named on the page: drawn quietly, with no cross. */
  inherited?: boolean;
  /** Beyond the org the page lies in. */
  outside?: boolean;
  /** The act on it is under way. */
  pending?: boolean;
  /** Says "Everyone here" for the role that is everybody, where the column or the line already says whose page it is. */
  compact?: boolean;
  /** The cross: what taking this holder out is called, and what it does. */
  remove?: { label: string; onRemove: () => void; disabled?: boolean; tip?: string } | undefined;
  className?: string;
}

export function HolderView({ holder, target, note, inherited = false, outside = false, pending = false, compact = false, remove, className }: HolderViewProps) {
  const { t } = useTranslation();
  const label = usePlaceLabel();
  const localSystemRole = target.kind === 'system' && holder.kind === 'role' && holder.at === null;
  const name = localSystemRole ? t('adminDefaults.roleName', { role: choiceOption(t, `accessRole.${holder.role}`).name })
    : holderName(t, holder, label({id:target.id,name:target.name}) ?? target.name, compact, label);
  const outsideMark = outside ? <span className="ml-1 rounded-sm bg-bg-muted px-1 text-xs font-medium text-text-muted">{t('access.holder.outside')}</span> : null;
  // One frame holds the chip and the cross that takes it out, so that the two read as one thing.
  const frame = cx(
    'inline-flex min-h-(--control-sm) max-w-full min-w-0 items-center rounded-full border py-0.5 text-sm text-text',
    inherited ? 'border-border bg-bg-subtle' : 'border-border-strong bg-surface',
    pending && 'opacity-60',
    className,
  );
  if (holder.kind === 'user') {
    return (
      <span className={cx(frame, 'gap-1 pl-1', remove === undefined && 'pr-2')}>
        <Avatar text={holder.user.avatar.text} color={holder.user.avatar.color} kind={holder.user.kind === 'service' ? 'service' : 'human'} size={24} />
        <span className="min-w-0 font-medium wrap-anywhere">{holder.user.display_name}</span>
        {outsideMark}
        {note !== undefined && <span className="min-w-0 text-text-muted wrap-anywhere">{note}</span>}
        {remove !== undefined && <RemoveButton {...remove} />}
      </span>
    );
  }
  const Mark = MARKS[holder.kind];
  return (
    <span className={frame}>
      <HolderChip
        bare
        name={name}
        count={localSystemRole ? undefined : holder.kind === 'group' ? holder.group.member_count : holder.kind === 'role' ? holder.count : undefined}
        icon={<Mark aria-hidden />}
        people={peopleOf(holder, target)}
        inherited={inherited}
        note={
          outside || note !== undefined ? (
            <>
              {outsideMark}
              {note}
            </>
          ) : undefined
        }
        className={remove === undefined ? 'pr-2' : undefined}
      />
      {remove !== undefined && <RemoveButton {...remove} />}
    </span>
  );
}
