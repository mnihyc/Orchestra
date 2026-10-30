/** Shared InlineAlert component (states); primitives are wrapped here.
 *
 * A few sentences beside the thing they are about: what went wrong with this form, what a choice
 * will do, that something was sent. Its colour is never the only thing that says which: every tone
 * has its own icon, and the words say the rest.
 */
import { CircleAlert, CircleCheck, Info, TriangleAlert } from 'lucide-react';
import type { ReactNode } from 'react';

import { cx } from '@/ui/cx';
import { StateLine } from '@/ui/StateLine';

export type AlertTone = 'info' | 'success' | 'warning' | 'danger';

export interface InlineAlertProps {
  tone?: AlertTone;
  /** Essential context, in the same line as the explanation. */
  title?: ReactNode;
  children?: ReactNode;
  /** The one next action, a compact button or link. */
  action?: ReactNode;
  className?: string;
}

const TONES: Record<AlertTone, { box: string; icon: string; Icon: typeof Info }> = {
  info: { box: 'border-info/30 bg-info-subtle', icon: 'text-info', Icon: Info },
  success: { box: 'border-success/30 bg-success-subtle', icon: 'text-success', Icon: CircleCheck },
  warning: { box: 'border-warning/40 bg-warning-subtle', icon: 'text-warning', Icon: TriangleAlert },
  danger: { box: 'border-danger/30 bg-danger-subtle', icon: 'text-danger', Icon: CircleAlert },
};

export function InlineAlert({ tone = 'info', title, children, action, className }: InlineAlertProps) {
  const { box, icon, Icon } = TONES[tone];
  return (
    <StateLine
      role={tone === 'danger' || tone === 'warning' ? 'alert' : 'status'}
      tone={tone === 'danger' || tone === 'warning' ? tone : 'neutral'}
      icon={<Icon className={icon} aria-hidden />} title={title} message={children} action={action} compact
      className={cx('rounded-md border px-3', box, className)}
    />
  );
}
