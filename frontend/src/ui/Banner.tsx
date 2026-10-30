/** Shared Banner component (states); primitives are wrapped here.
 *
 * One line across the top of the work that concerns everything under it: a notice of the system's
 * administrators, a connection that is gone, a new version. It does not move and takes no place of
 * the content away beyond its line.
 */
import { CircleAlert, Info, TriangleAlert, X } from 'lucide-react';
import type { ReactNode } from 'react';
import { useTranslation } from 'react-i18next';

import { cx } from '@/ui/cx';

export type BannerTone = 'info' | 'warning' | 'danger';

export interface BannerProps {
  tone?: BannerTone;
  children: ReactNode;
  /** What can be done about it, at the end of the line. */
  action?: ReactNode;
  /** Keep a long reason readable on phones by placing its action on the next line. */
  actionBelowOnMobile?: boolean;
  /** Present where the reader may put the banner away. */
  onDismiss?: () => void;
  className?: string;
}

const TONES: Record<BannerTone, { box: string; icon: string; Icon: typeof Info }> = {
  info: { box: 'border-info/30 bg-info-subtle', icon: 'text-info', Icon: Info },
  warning: { box: 'border-warning/40 bg-warning-subtle', icon: 'text-warning', Icon: TriangleAlert },
  danger: { box: 'border-danger/30 bg-danger-subtle', icon: 'text-danger', Icon: CircleAlert },
};

export function Banner({ tone = 'info', children, action, actionBelowOnMobile = false, onDismiss, className }: BannerProps) {
  const { t } = useTranslation();
  const { box, icon, Icon } = TONES[tone];
  return (
    <div
      role={tone === 'info' ? 'status' : 'alert'}
      className={cx('flex min-h-(--control-md) items-center gap-2 border-b px-4 py-1 text-sm text-text', actionBelowOnMobile && 'flex-wrap sm:flex-nowrap', box, className)}
    >
      <Icon className={icon} aria-hidden />
      <p className="min-w-0 flex-1 wrap-anywhere">{children}</p>
      {actionBelowOnMobile && action !== undefined && action !== null ? <div className="min-w-0 basis-full pl-6 sm:basis-auto sm:pl-0">{action}</div> : action}
      {onDismiss !== undefined && (
        <button
          type="button"
          aria-label={t('common.dismiss')}
          className="inline-flex size-(--control-sm) shrink-0 items-center justify-center rounded-sm text-text-muted transition-colors hover:bg-bg-muted hover:text-text"
          onClick={onDismiss}
        >
          <X aria-hidden />
        </button>
      )}
    </div>
  );
}
