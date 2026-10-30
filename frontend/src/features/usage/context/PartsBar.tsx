/** A bar of parts, each with its colour and pattern and its words, which the keyboard reaches part by part.
 *
 * The parts are laid side by side in proportion to their shares. Each is focusable and says what it is,
 * its tokens and its share to a screen reader and in its tooltip; the screen that draws a bar also writes
 * the same numbers as text beside it, so that nothing is told by colour or by the pointer alone.
 */
import { useTranslation } from 'react-i18next';

import { lookStyle, type Segment } from '@/features/usage/context/fill';
import { cx } from '@/ui/cx';
import { Tooltip } from '@/ui/Tooltip';

export interface PartsBarProps<Key extends string> {
  segments: readonly Segment<Key>[];
  /** What the bar is, for a screen reader. */
  label: string;
  /** What one part says: its name, tokens and share. */
  describe: (segment: Segment<Key>) => string;
  /** A thin bar for a footer, a tall one for a panel. */
  size?: 'thin' | 'tall';
  /** Whether each part takes the keyboard's focus (a bar inside a button does not). */
  focusable?: boolean;
  className?: string;
}

export function PartsBar<Key extends string>({ segments, label, describe, size = 'tall', focusable = true, className }: PartsBarProps<Key>) {
  const { t } = useTranslation();
  if (segments.length === 0) return null;
  return (
    <span
      role="group"
      aria-label={label}
      className={cx('flex w-full overflow-hidden rounded-full border border-border bg-bg-muted', size === 'thin' ? 'h-(--meter-bar)' : 'h-4', className)}
      data-parts-bar
    >
      {segments.map((segment) => {
        const words = describe(segment);
        const part = (
          <span
            key={segment.key}
            role="img"
            aria-label={words}
            tabIndex={focusable ? 0 : undefined}
            data-part={segment.key}
            // Each part grows by its share; the least width keeps a small part seen and reachable.
            className="h-full min-w-[3px] basis-0 outline-offset-1 focus-visible:outline-2 focus-visible:outline-accent"
            style={{ flexGrow: segment.share, ...lookStyle(segment.look) }}
          />
        );
        return focusable ? (
          <Tooltip key={segment.key} content={words}>
            {part}
          </Tooltip>
        ) : (
          part
        );
      })}
      <span className="sr-only">{t('contextUsage.bar.numbersBelow')}</span>
    </span>
  );
}

/** A small square of a part's look, beside its name in a legend. */
export function Swatch({ segment }: { segment: Pick<Segment, 'look'> }) {
  return <span aria-hidden className="inline-block size-3 shrink-0 rounded-sm border border-border" style={lookStyle(segment.look)} />;
}
