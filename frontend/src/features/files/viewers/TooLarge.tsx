/** Text beyond the preview limit: its first part, read-only, with how much there is.
 *
 * A text longer than is read is shown to the point that was read. The code viewer draws it, read-only;
 * this is the line above it that says how much of the whole it is looking at.
 */
import { useTranslation } from 'react-i18next';

import { useFormatContext } from '@/lib/appearance';
import { formatBytes } from '@/lib/format';

export function TooLargeNote({ shown, total }: { shown: number; total: number }) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  return (
    <p className="border-b border-border bg-bg-subtle px-3 py-1 text-sm text-text-muted" data-testid="too-large">
      {t('files.code.shownTo', { shown: formatBytes(shown, locale), total: formatBytes(total, locale) })}
    </p>
  );
}
