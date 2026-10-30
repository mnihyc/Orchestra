/** Shared AvatarStack component (people); primitives are wrapped here.
 *
 * Several people as one row of avatars that overlap, with the number of those that did not fit. The
 * names are said once, to a screen reader, for the row as a whole. Where the row stands alone, outside
 * anything that is pressed itself, a click on it (or on its "+3") opens the list of everybody in it,
 * each name opening its card: no avatar and no count is a dead end. Inside a button or a link the row is
 * part of what that says, and opens nothing of its own.
 */
import { PeopleCard, PeopleList } from '@/ui/PeopleList';
import { useLayoutEffect, useRef, useState, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';

import type { UserRef } from '@/generated/api-types';
import { Avatar, type AvatarSize } from '@/ui/Avatar';
import { cx } from '@/ui/cx';
import { standsAlone } from '@/ui/pressed';

export interface AvatarStackProps {
  users: readonly (Pick<UserRef, 'avatar' | 'display_name' | 'id' | 'kind'> & { username?: string; address?: string | null })[];
  /** How many people there are in all, where `users` is only the first of them. */
  total?: number;
  /** How many avatars are drawn before the rest are counted. */
  max?: number;
  size?: AvatarSize;
  className?: string;
}

/** Enough avatars to tell that there are several people, few enough to stay a glance. */
const DEFAULT_MAX = 5;

/** A count opens the same people card by pointer, keyboard or tap. */
export function PeopleCount({ users, total, children }: Pick<AvatarStackProps, 'users' | 'total'> & { children: ReactNode }) {
  const { t } = useTranslation();
  return <PeopleCard label={t('ui.avatarStack.list')} trigger={<button type="button" className="focus-inset rounded-sm text-sm text-text-muted underline decoration-dotted underline-offset-2">{children}</button>}>
    <PeopleList users={users} hidden={Math.max(0, (total ?? users.length) - users.length)} />
  </PeopleCard>;
}

export function AvatarStack({ users, total, max = DEFAULT_MAX, size = 24, className }: AvatarStackProps) {
  const { t } = useTranslation();
  const at = useRef<HTMLSpanElement>(null);
  const hasPeople = users.length > 0;
  // Whether the row stands outside anything that is pressed: known once it is drawn, before it is painted.
  const [alone, setAlone] = useState(false);
  useLayoutEffect(() => {
    setAlone(hasPeople && standsAlone(at.current));
  }, [hasPeople]);
  // Nobody to show is no picture at all; an image that has no name to say would be a fault of its own.
  if (!hasPeople) return null;
  const shown = users.slice(0, max);
  const more = (total ?? users.length) - shown.length;
  const names = users.map((user) => user.display_name).join(', ');
  const row = (
    <span role="img" aria-label={names} className={cx('inline-flex items-center', className)}>
      {shown.map((user, index) => (
        <Avatar
          key={user.id}
          text={user.avatar.text}
          color={user.avatar.color}
          kind={user.kind === 'service' ? 'service' : 'human'}
          size={size}
          // The ring is the ground the row lies on, so the avatars read as separate discs.
          className={cx('ring-2 ring-surface', index > 0 && '-ml-2')}
        />
      ))}
      {more > 0 && (
        <span
          aria-hidden
          className="-ml-2 inline-flex h-6 min-w-6 items-center justify-center rounded-full bg-bg-muted px-1 text-xs font-medium text-text-muted ring-2 ring-surface"
        >
          {t('ui.avatarStack.more', { count: more })}
        </span>
      )}
    </span>
  );
  const content = !alone ? row : <PeopleCard label={t('ui.avatarStack.list')} trigger={<button type="button" aria-label={t('ui.avatarStack.open', { names })} className="inline-flex max-w-full rounded-full">{row}</button>}>
    <PeopleList users={users} hidden={Math.max(0, (total ?? users.length) - users.length)} />
  </PeopleCard>;
  // Reappearing tabs inspect outside this row's own trigger, just as an individual chip does.
  return <span ref={at} className="inline-flex">{content}</span>;
}
