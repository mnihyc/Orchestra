/** A compact setting with its supplied description, value and source. */
import type { ReactNode } from 'react';
import { cx } from '@/ui/cx';

export interface SettingRowProps {
  name: string;
  value: ReactNode;
  /** An explanation retained for both editable and read-only values. */
  description?: ReactNode;
  /** Supplementary information only: never repeat the visible description. */
  help?: ReactNode;
  /** The actual source; permission explanations belong once above a collection. */
  reason?: ReactNode;
  /** Native applicability, beside the source or opened directly in this row. */
  reach?: ReactNode;
  actions?: ReactNode;
  /** Pending/refusal/draft feedback; never the definition's ordinary description. */
  status?: ReactNode;
  /** Secondary metadata, rendered only after the person asks for it. */
  details?: ReactNode;
  /** A draft editor may retain its children while its disclosure is closed. */
  detailsVisible?: boolean;
  settingKey?: string;
  readOnly?: boolean;
  /** A page whose section already supplies the inset can align rows to that section. */
  inset?: boolean;
  as?: 'li' | 'div';
  className?: string;
}

export function SettingRow({ name, value, description, help, reason, reach, actions, status, details, detailsVisible = true, settingKey, readOnly = false, inset = true, as: Tag = 'li', className }: SettingRowProps) {
  return <Tag data-setting={settingKey} data-setting-row data-setting-readonly={readOnly || undefined} tabIndex={-1}
    className={cx('@container flex min-w-0 flex-wrap items-start gap-x-4 gap-y-1', inset && 'px-3', readOnly ? 'py-1.5' : 'py-2', className)}>
    <div className="min-w-0 flex-1 basis-[min(100%,16rem)]">
      <div className="flex min-h-(--control-md) min-w-0 items-start gap-1"><span className="font-medium leading-(--control-md) wrap-anywhere text-text">{name}</span>{help !== undefined && <span className="flex min-h-(--control-md) shrink-0 items-center">{help}</span>}{actions !== undefined && <span className="flex min-h-(--control-md) shrink-0 items-center gap-1">{actions}</span>}</div>
      {description !== undefined && <div data-setting-description data-overflow-text={typeof description === 'string' ? description : undefined}
        className="truncate text-sm text-text-muted">{description}</div>}
    </div>
    <div className="flex min-w-0 max-w-full flex-1 basis-[min(100%,14rem)] flex-wrap items-start justify-start gap-2 @min-[31rem]:justify-end">
      <div className={cx('flex min-w-0 max-w-full gap-1', readOnly ? 'flex-row flex-wrap items-baseline' : 'flex-col items-start')}><div className="min-w-0 max-w-full text-sm text-text">{value}</div>
        {reason !== undefined && <>{readOnly && <span aria-hidden className="text-text-muted">·</span>}<div className="flex min-w-0 flex-wrap items-center gap-1 text-xs text-text-muted">{reason}</div></>}{reach}</div>
    </div>
    {details !== undefined && <div hidden={!detailsVisible} className="min-w-0 basis-full">{details}</div>}
    {status !== undefined && <div className="min-w-0 basis-full">{status}</div>}
  </Tag>;
}
