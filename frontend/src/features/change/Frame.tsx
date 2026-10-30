/** The dialog every change stands in: its title, the four steps with the one it is at, and the acts at its foot.
 *
 * The frame every change stands in: a dialog whose title names the act and the thing, one line under it that says
 * what the act is, and the four views in their order, so that whoever has made one change has made them all. The
 * steps are told above the body, and the one that is under way is marked by more than its colour. The wizard can be
 * left while a job runs (it is the server's), and cannot while a request that locks is on its way.
 */
import { Check, X } from 'lucide-react';
import type { ReactNode } from 'react';
import { useTranslation } from 'react-i18next';

import { cx } from '@/ui/cx';
import { Dialog, type DialogSize } from '@/ui/Dialog';

/** The four views, in the order a change goes through them. */
export type View = 'plan' | 'confirm' | 'progress' | 'done';

const VIEWS: readonly View[] = ['plan', 'confirm', 'progress', 'done'];

export interface StepsProps {
  at: View;
  /** The last view shows that it did not end well. */
  failed?: boolean;
}

/** The four views as a row of steps; narrow screens show only the one that is under way. */
export function Steps({ at, failed = false }: StepsProps) {
  const { t } = useTranslation();
  const index = VIEWS.indexOf(at);
  return (
    <ol aria-label={t('change.steps.label')} className="flex flex-wrap items-center gap-x-2 gap-y-1 text-sm text-text-muted">
      {VIEWS.map((view, position) => {
        const done = position < index || (at === 'done' && !failed && position === index);
        const current = position === index;
        const bad = failed && current && at === 'done';
        return (
          <li key={view} aria-current={current ? 'step' : undefined} className={cx('items-center gap-1', current ? 'flex' : 'hidden sm:flex')}>
            <span
              aria-hidden
              className={cx(
                'flex size-5 items-center justify-center rounded-full border text-xs',
                bad ? 'border-danger bg-danger-subtle text-danger-text' : current ? 'border-accent bg-accent-subtle text-accent-text' : done ? 'border-border-strong bg-bg-muted text-text' : 'border-border text-text-subtle',
              )}
            >
              {bad ? <X className="size-4" /> : done ? <Check className="size-4" /> : position + 1}
            </span>
            <span className={cx(current ? 'font-medium text-text' : '')}>{t(`change.steps.${view}`)}</span>
            {position < VIEWS.length - 1 && <span aria-hidden className="ml-1 hidden text-text-subtle sm:inline">—</span>}
          </li>
        );
      })}
    </ol>
  );
}

export interface FrameProps {
  /** What stands at the end of the row of steps: the plan's Options. */
  aside?: ReactNode;
  title: ReactNode;
  /** One line under the title: what this act is. */
  about?: ReactNode;
  at: View;
  failed?: boolean;
  open: boolean;
  onClose: () => void;
  /** False while a request that cannot be taken back is on its way. */
  dismissible?: boolean;
  size?: DialogSize;
  /** An ordinary provider/profile switch is a picker and confirmation, without wizard chrome. */
  steps?: boolean;
  children: ReactNode;
  footer: ReactNode;
}

export function Frame({ aside, title, about, at, failed = false, open, onClose, dismissible = true, size = 'lg', steps = true, children, footer }: FrameProps) {
  return (
    <Dialog open={open} onOpenChange={(next) => !next && dismissible && onClose()} size={size} title={title} {...(about === undefined ? {} : { description: about })} dismissible={dismissible} footer={footer}>
      <div className="flex flex-col gap-4 py-2" data-change-view={at}>
        {(steps || aside !== undefined) && <div className="flex flex-wrap items-center justify-between gap-2">
          {steps && <Steps at={at} failed={failed} />}
          {aside}
        </div>}
        {children}
      </div>
    </Dialog>
  );
}
