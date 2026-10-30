/** The twelve colours of the palette as a menu: the swatch, the name, and none. */
import type { ReactNode } from 'react';

import { identityGround } from '@/ui/Avatar';
import { i18n } from '@/i18n';
import { choiceOption } from '@/i18n/choices';
import type { MenuEntry } from '@/ui/Menu';
import { cx } from '@/ui/cx';

/** The colours of the palette, named in the reader's language. */
export const TINT_NAMES = ['red', 'orange', 'amber', 'lime', 'green', 'teal', 'cyan', 'blue', 'indigo', 'purple', 'pink', 'grey'] as const;

export function tintName(tint: number): string {
  return i18n.t(`documents.tint.${TINT_NAMES[tint] ?? 'grey'}` as 'documents.tint.red');
}

/** A small square of a colour. */
export function Swatch({ tint }: { tint: number | null }): ReactNode {
  return (
    <span
      aria-hidden
      className={cx('inline-block size-4 rounded-sm border border-border-strong', tint === null ? 'bg-bg' : identityGround(tint))}
    />
  );
}

/** The entries that choose one colour of the palette, or none. */
export function tintEntries(id: string, label: string, current: number | null, choose: (tint: number | null) => void): MenuEntry[] {
  return [
    {
      type: 'radio',
      id,
      label,
      value: current === null ? 'none' : String(current),
      onValueChange: (value) => choose(value === 'none' ? null : Number(value)),
      options: [
        { value: 'none', ...choiceOption(i18n.t, 'colour.none'), icon: <Swatch tint={null} /> },
        ...TINT_NAMES.map((_, tint) => ({ value: String(tint), ...choiceOption(i18n.t, 'colour.value', { name: tintName(tint) }), icon: <Swatch tint={tint} /> })),
      ],
    },
  ];
}
