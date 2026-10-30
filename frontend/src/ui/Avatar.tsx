/** Shared Avatar component (people); primitives are wrapped here.
 *
 * Who did something, at a glance: one character on the person's own colour. A person is a circle; a
 * service account a rounded square with a small mark at its corner; an agent a rounded square on a
 * quiet ground; an org a rounded square. The character is drawn at 45 % of the diameter, whatever
 * the size.
 */
import { Bot } from 'lucide-react';
import { type ReactNode, useLayoutEffect, useRef, useState } from 'react';
import type { UserRef } from '@/generated/api-types';

import { IDENTITY_COLORS } from '@/lib/grapheme';
import { cx } from '@/ui/cx';
import { PersonCard } from '@/ui/PersonCard';
import { standsAlone } from '@/ui/pressed';

/** The diameters there are, in pixels at the usual text size; `row` follows the density of lists. */
export type AvatarSize = 24 | 28 | 32 | 40 | 'row';

export interface AvatarProps {
  /** A known person whose card this standalone avatar opens; omitted for decorative avatars. */
  person?: UserRef;
  /** The one character shown. */
  text: string;
  /** Which colour of the identity palette, from 0. */
  color: number;
  kind?: 'human' | 'service' | 'agent' | 'org';
  size?: AvatarSize;
  /** The name, where the avatar stands alone; left out where the name is written beside it. */
  label?: string;
  /** What an agent shows in place of a character: the mark of its harness. */
  mark?: ReactNode;
  className?: string;
}

const SIZES: Record<AvatarSize, string> = {
  24: '[--avatar-d:var(--avatar-24)]',
  28: '[--avatar-d:var(--avatar-28)]',
  32: '[--avatar-d:var(--avatar-32)]',
  40: '[--avatar-d:var(--avatar-40)]',
  row: '[--avatar-d:var(--avatar-size)]',
};

/** One class for each colour of the palette, written out so that the stylesheet holds them all. */
const GROUNDS = [
  'bg-(--identity-0)',
  'bg-(--identity-1)',
  'bg-(--identity-2)',
  'bg-(--identity-3)',
  'bg-(--identity-4)',
  'bg-(--identity-5)',
  'bg-(--identity-6)',
  'bg-(--identity-7)',
  'bg-(--identity-8)',
  'bg-(--identity-9)',
  'bg-(--identity-10)',
  'bg-(--identity-11)',
] as const;

/** The class of the palette's colour at `index`, whatever number it is given. */
export function identityGround(index: number): string {
  const safe = Number.isInteger(index) ? ((index % IDENTITY_COLORS) + IDENTITY_COLORS) % IDENTITY_COLORS : 0;
  return GROUNDS[safe] ?? GROUNDS[0];
}

export function Avatar({ text, color, kind = 'human', size = 28, label, mark, className, person }: AvatarProps) {
  const at = useRef<HTMLSpanElement>(null);
  const [alone, setAlone] = useState(false);
  const personId = person?.id;
  useLayoutEffect(() => { setAlone(personId !== undefined && standsAlone(at.current)); }, [personId]);
  const agent = kind === 'agent';
  const picture = (
    <span
      ref={at}
      role={label === undefined ? undefined : 'img'}
      aria-label={label}
      aria-hidden={label === undefined ? true : undefined}
      className={cx(
        'avatar relative inline-flex shrink-0 items-center justify-center font-semibold select-none',
        SIZES[size],
        kind === 'human' ? 'rounded-full' : 'rounded-md',
        agent ? 'bg-bg-muted text-text-muted' : cx(identityGround(color), 'text-identity-text'),
        className,
      )}
    >
      {agent && mark !== undefined ? mark : text}
      {kind === 'service' && (
        <span className="absolute -right-0.5 -bottom-0.5 flex size-3 items-center justify-center rounded-full bg-surface text-text-muted">
          <Bot size={10} aria-hidden />
        </span>
      )}
    </span>
  );
  return person?.kind === 'human' && alone ? <PersonCard user={person}>{picture}</PersonCard> : picture;
}
