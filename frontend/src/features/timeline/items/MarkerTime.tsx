/** A history line's exact moment, opened by a pointer, keyboard or touch. */
import { Clock } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { useFormatContext } from '@/lib/appearance';
import { formatExact } from '@/lib/time';
import { Popover } from '@/ui/Popover';

export function MarkerTime({ at }: { at: string }) {
  const { t } = useTranslation();
  const context = useFormatContext();
  const exact = formatExact(at, context);
  return <Popover label={t('timeline.exactTime')} align="center" className="px-2 py-1" trigger={
    <button type="button" aria-label={t('timeline.exactTime')} title={exact}
      className="focus-inset inline-flex shrink-0 items-center rounded-sm p-0.5 text-text-muted hover:bg-bg-muted">
      <Clock className="size-3" aria-hidden />
    </button>
  }><time dateTime={at} className="text-xs tabular-nums">{exact}</time></Popover>;
}
