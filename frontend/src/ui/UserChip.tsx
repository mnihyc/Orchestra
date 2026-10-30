/** Shared UserChip component (people); primitives are wrapped here.
 *
 * A person in a line of text: their avatar and their name, and under or beside it the name they sign
 * in with. A service identity is drawn as one. The name opens the person's card on a click and shows it
 * under the pointer and the keyboard's focus, so that a name cut short is never a dead end. Inside
 * something that is pressed itself (a button, a link, a choice) the chip is part of what that says and
 * opens nothing of its own: there its whole name is under the pointer. A screen that puts the chip in a
 * card of its own, with what it may do with the person, says `card={false}`.
 */
import { useLayoutEffect, useRef, useState } from 'react';

import type { UserRef } from '@/generated/api-types';
import { Avatar, type AvatarSize } from '@/ui/Avatar';
import { cx } from '@/ui/cx';
import { PersonCard } from '@/ui/PersonCard';
import { standsAlone } from '@/ui/pressed';
import type { PersonThing } from '@/features/people/personContext';

export interface UserChipProps {
  user: Pick<UserRef, 'avatar' | 'display_name' | 'kind' | 'username'> & { id?: string; address?: string | null };
  size?: AvatarSize;
  /** Writes the username after the name, quietly. */
  showUsername?: boolean;
  /** Whether the name opens the person's card; a screen that wraps the chip in a card of its own says no. */
  card?: boolean;
  /** A mention keeps the written @ in its name. */
  mention?: boolean;
  /** A nearby thing enables its relevant quick actions. */
  thing?: PersonThing | undefined;
  nameClassName?: string | undefined;
  className?: string;
}

export function UserChip({ user, size = 24, showUsername = false, card = true, mention = false, thing, nameClassName, className }: UserChipProps) {
  const at = useRef<HTMLSpanElement>(null);
  // Whether the chip stands outside anything that is pressed: known once it is drawn, before it is painted.
  const [alone, setAlone] = useState(false);
  useLayoutEffect(() => {
    setAlone(standsAlone(at.current));
  }, []);
  // The person it opens, where it opens one.
  const id = card && alone ? user.id : undefined;
  const chip = (
    <>
      <Avatar text={user.avatar.text} color={user.avatar.color} kind={user.kind === 'service' ? 'service' : 'human'} size={size} />
      <span className={cx('min-w-0 truncate font-medium text-text', nameClassName)} {...(id === undefined ? { title: user.display_name } : {})}>
        {mention ? `@${user.display_name}` : user.display_name}
      </span>
      {showUsername && <span className="min-w-0 truncate text-text-subtle">{`@${user.username}`}</span>}
    </>
  );
  // Tab restoration reruns layout effects; inspect outside our own card's button each time.
  return <span ref={at} className={cx('inline-flex min-w-0 items-center gap-2', className)}>
    {id === undefined ? chip : <PersonCard user={{ ...user, id }} thing={thing} label={mention ? `@${user.display_name}` : undefined}>{chip}</PersonCard>}
  </span>;
}
