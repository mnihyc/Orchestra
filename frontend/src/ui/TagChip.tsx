/** Shared TagChip component (status); primitives are wrapped here.
 *
 * A tag as a topic carries it: its name on one of the twelve colours of the identity palette. The
 * colour is a mark and never the meaning: the name is always written, and a chip that can be taken off
 * has a button named after what it takes off.
 */
import { X } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { identityGround } from '@/ui/Avatar';
import { cx } from '@/ui/cx';

export interface TagChipProps {
  name: string;
  /** Which colour of the identity palette, from 0. */
  color: number;
  /** Present where the tag can be taken off: the chip then ends in a button. */
  onRemove?: () => void;
  className?: string;
}

export function TagChip({ name, color, onRemove, className }: TagChipProps) {
  const { t } = useTranslation();
  return (
    <span
      className={cx(
        'inline-flex h-5 max-w-full shrink-0 items-center gap-1 rounded-full px-2 text-xs font-medium text-identity-text',
        identityGround(color),
        className,
      )}
    >
      <span className="truncate">{name}</span>
      {onRemove !== undefined && (
        <button
          type="button"
          aria-label={t('ui.tag.remove', { name })}
          className="-mr-1 inline-flex size-4 items-center justify-center rounded-full transition-colors hover:bg-black/20"
          onClick={onRemove}
        >
          <X size={12} aria-hidden />
        </button>
      )}
    </span>
  );
}
