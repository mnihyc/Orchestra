import { useTranslation } from 'react-i18next';
import type { DeletedInspection as InspectionFacts } from '@/generated/api-types';
import { useFormatContext } from '@/lib/appearance';
import { InlineAlert } from '@/ui/InlineAlert';

/** The deletion promise is a fact of this read, not one reconstructed from the audit row. */
export function InspectionBanner({ facts }: { facts?: InspectionFacts | null }) {
  const { t } = useTranslation(); const format = useFormatContext(); const date = (value: string) => new Intl.DateTimeFormat(format.locale, { dateStyle: 'medium', timeZone: format.timeZone }).format(new Date(value));
  if (facts === undefined || facts === null) return null;
  return <InlineAlert tone="warning" title={facts.deleted_by ? t('trace.deletedBy', { name: facts.deleted_by.display_name, day: date(facts.deleted_at), until: date(facts.purge_at) }) : t('trace.deletedAnonymous', { day: date(facts.deleted_at), until: date(facts.purge_at) })}><p>{t('trace.readOnly')}</p></InlineAlert>;
}
