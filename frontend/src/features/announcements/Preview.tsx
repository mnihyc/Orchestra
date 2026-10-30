/** Explain the effect of the current selection.
 */
import { useTranslation } from 'react-i18next';
import { InlineAlert } from '@/ui/InlineAlert';

export function AnnouncementPreview({ name }: { name: string }) {
  const { t } = useTranslation();
  return <InlineAlert tone="info">{t('placeAnnouncements.preview', { name })}</InlineAlert>;
}
