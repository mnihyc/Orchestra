import { useTranslation } from 'react-i18next';
import type { DeletedInspection as InspectionFacts } from '@/generated/api-types';
import { useFormatContext } from '@/lib/appearance';
import { InlineAlert } from '@/ui/InlineAlert';
import { RetentionFacts } from '@/features/project/RetentionFacts';

/** The deletion promise is a fact of this read, not one reconstructed from the audit row. */
export function InspectionBanner({ facts }: { facts?: InspectionFacts | null }) {
  const { t } = useTranslation(); const format = useFormatContext(); const date = (value: string) => new Intl.DateTimeFormat(format.locale, { dateStyle: 'medium', timeZone: format.timeZone }).format(new Date(value));
  if (facts === undefined || facts === null) return null;
  return <InlineAlert tone="warning" title={facts.deleted_by ? t('trace.deletedBy', { name: facts.deleted_by.display_name, day: date(facts.deleted_at), until: date(facts.purge_at) }) : t('trace.deletedAnonymous', { day: date(facts.deleted_at), until: date(facts.purge_at) })}><div className="flex flex-wrap items-center gap-2"><p>{t('trace.readOnly')}</p><RetentionFacts purgeAt={facts.purge_at} cleanup={facts.cleanup_hold} /></div></InlineAlert>;
}
