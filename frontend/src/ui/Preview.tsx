/** A short preview expands in the page, keeping the page as the one place that scrolls. */
import { type ReactNode, useId, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { cx } from '@/ui/cx';
import { linkButtonClasses } from '@/ui/link';

/** Six lines or table rows give the value's shape without making an inspector inside the page. */
export const PREVIEW_LINES = 6;

export function Preview({ more, children }: { more: boolean; children: (expanded: boolean) => ReactNode }) {
  const { t } = useTranslation();
  const [expanded, setExpanded] = useState(false);
  const id = useId();
  return <div className="min-w-0" data-content-preview>
    <div id={id} className="min-w-0">{children(expanded)}</div>
    {more && <button type="button" aria-expanded={expanded} aria-controls={id} onClick={() => setExpanded(!expanded)} className={cx(linkButtonClasses, 'focus-inset mt-1 inline-flex min-h-6 items-center text-sm')}>
      {t(expanded ? 'ui.preview.showLess' : 'ui.preview.showAll')}
    </button>}
  </div>;
}

export function TextPreview({ value, className }: { value: string; className?: string }) {
  const lines = value.split('\n');
  return <Preview more={lines.length > PREVIEW_LINES}>{(expanded) => <pre className={cx('whitespace-pre-wrap wrap-anywhere', className)}>{expanded ? value : lines.slice(0, PREVIEW_LINES).join('\n')}</pre>}</Preview>;
}
