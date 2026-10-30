/** A pointer shown unasked the first time something is used, which the reader can close for good.
 *
 * It says one thing in a sentence and has one button that closes it. What was closed follows the person
 * to every device. It is not a hint: a hint is the "?" the reader asks, and stays; a tip is shown to them,
 * and goes once it has been read.
 */
import { Lightbulb, X } from 'lucide-react';
import type { ReactNode } from 'react';
import { useTranslation } from 'react-i18next';

import { useDismissed } from '@/features/help/Dismissed';
import { cx } from '@/ui/cx';

/** Every tip there is. */
export type TipId = 'finder';

export function Tip({ id, children, className }: { id: TipId; children: ReactNode; className?: string }) {
  const { t } = useTranslation();
  const dismissed = useDismissed();
  if (dismissed.isClosed(id)) return null;
  return (
    <div role="note" className={cx('flex items-start gap-2 rounded-lg border border-border bg-bg-subtle px-3 py-2 text-sm text-text', className)}>
      <Lightbulb aria-hidden className="mt-0.5 text-text-muted" />
      <div className="min-w-0 flex-1">{children}</div>
      <button
        type="button"
        aria-label={t('help.tip.close')}
        className="inline-flex size-5 shrink-0 items-center justify-center rounded-md text-text-muted hover:bg-bg-muted hover:text-text"
        onClick={() => dismissed.close(id)}
      >
        <X size={16} aria-hidden />
      </button>
    </div>
  );
}
