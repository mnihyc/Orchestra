/** A section of the left bar that folds: its heading with a marker, what in it needs a look while it is folded, what can be done to it at its end, and its rows.
 *
 * Folded, a section is its heading alone, and the heading then says only what waits inside: a dot
 * while something its reader follows is unread, and a number for what waits for them in person, the
 * decisions and the times they are mentioned. Every heading has the height of a row and every section
 * the same room above and below it, so that the bar has one rhythm from its top to its foot; the
 * marker stands where the arrows of the tree stand, and the heading's words where the icons of the
 * rows do. A section opens and closes over the short step of the motion tokens, and at once for
 * somebody who asked for less motion.
 *
 * The heading of the tree's section can be a path instead of a word: the way to the place the tree
 * begins at, which the caller draws; the marker then keeps the section's name for a screen reader.
 */
import { ChevronRight } from 'lucide-react';
import { type ReactNode, useEffect, useId, useState } from 'react';
import { useTranslation } from 'react-i18next';

import type { Needs } from '@/features/navigation/treeRows';
import { Badge } from '@/ui/Badge';
import { cx } from '@/ui/cx';
import { StatusDot } from '@/ui/StatusDot';

/**
 * How long a section that was folded keeps its rows drawn, so that they are seen to leave: the short
 * step of the motion tokens (`--motion-fast`, 120 ms) and a frame, after which nothing of it is left in the page.
 */
const FOLD_MS = 140;

export interface SectionProps {
  /** The name of the section: its heading, and what its marker is called. */
  title: string;
  folded: boolean;
  onFoldedChange: (folded: boolean) => void;
  /** What in it needs a look; shown only while the section is folded, since its rows say it otherwise. */
  needs?: Needs;
  /** What stands in the heading in place of the section's name: the way to where the tree begins. */
  path?: ReactNode;
  /** What can be done to the section, at the end of its heading. */
  action?: ReactNode;
  children: ReactNode;
}

export function Section({ title, folded, onFoldedChange, needs, path, action, children }: SectionProps) {
  const { t } = useTranslation();
  const body = useId();
  // The rows stay drawn while the section closes, and are gone once it has: a folded tree of thousands costs nothing.
  const [drawn, setDrawn] = useState(!folded);
  if (!folded && !drawn) setDrawn(true);
  // A section that was never folded in this page is simply there; one that is opened again is seen to open.
  const [wasFolded, setWasFolded] = useState(folded);
  if (folded && !wasFolded) setWasFolded(true);
  useEffect(() => {
    if (!folded) return;
    const timer = setTimeout(() => setDrawn(false), FOLD_MS);
    return () => clearTimeout(timer);
  }, [folded]);
  const shown = !folded || drawn;
  return (
    <section className="flex min-w-0 flex-col border-t border-border py-1">
      <div className="@container flex min-h-(--section-heading-height) items-center pr-2 pl-1">
        <h2 className={cx('flex min-w-0 self-stretch', path === undefined && 'flex-1')}>
          <button
            type="button"
            aria-expanded={!folded}
            aria-controls={body}
            data-navigation-section-label
            className="focus-inset flex min-w-0 flex-1 items-center gap-1 pl-1 text-left text-xs font-normal text-text-muted transition-colors hover:text-text"
            onClick={() => onFoldedChange(!folded)}
          >
            <span aria-hidden className="flex size-4 shrink-0 items-center justify-center">
              <ChevronRight className={cx('transition-transform duration-(--motion-fast)', !folded && 'rotate-90')} />
            </span>
            <span className={cx('truncate', path !== undefined && 'sr-only')}>{title}</span>
          </button>
        </h2>
        {path}
        {folded && needs !== undefined && (
          <span className="flex shrink-0 items-center gap-1 px-1">
            {/* A number of mentions says that something is unread already: the dot is for when there is none. */}
            {needs.unread && needs.mentions === 0 && <StatusDot tone="accent" label={t('navigation.unreadInside')} />}
            <Badge tone="accent" count={needs.mentions} label={t('navigation.mentionsInside', { count: needs.mentions })} />
            <Badge tone="warning" count={needs.waiting} label={t('places.tree.approvalsInside', { count: needs.waiting })} />
          </span>
        )}
        {action}
      </div>
      <div
        id={body}
        hidden={!shown}
        // The rows are out of reach from the moment the section closes, also while they are still seen to leave.
        inert={folded}
        className={cx(
          'grid transition-[grid-template-rows,opacity] duration-(--motion-fast) ease-(--motion-ease)',
          folded ? 'grid-rows-[0fr] opacity-0' : 'grid-rows-[1fr] opacity-100',
          !folded && wasFolded && 'starting:grid-rows-[0fr] starting:opacity-0',
        )}
      >
        {/* Clipped, and no scrolling box of its own: what stays at the top of the bar while it scrolls stays by the bar's own scrolling. */}
        <div className="flex min-h-0 min-w-0 flex-col overflow-clip">{shown && children}</div>
      </div>
    </section>
  );
}
