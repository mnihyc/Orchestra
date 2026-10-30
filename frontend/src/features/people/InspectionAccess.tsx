/** A creation-time standing access description from the current inspection row, without actual-read notices. */
import { useQuery } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';

import { pageQuery } from '@/api/access';
import { holderName } from '@/features/access/holders';
import { usePlaceLabel } from '@/features/navigation/placeLabels';

export function InspectionAccess({ orgId }: { orgId: string }) {
  const { t } = useTranslation();
  const label = usePlaceLabel();
  const read = useQuery({ ...pageQuery({ kind: 'org', id: orgId }), retry: false, staleTime: 0 });
  const row = read.data?.rows.find((row) => row.permission === 'directs.view');
  const holders = row === undefined ? null : [...row.entries.map((entry) => entry.holder), ...(row.inherited?.holders ?? [])];
  const names = holders === null ? null : [...new Set(holders.map((holder) => holderName(t, holder, '',false,label)))];
  return <p className="text-xs text-text-muted">{names === null ? t('people.readersUnknown') : names.length === 0 ? t('people.readersPeople') : t('people.readersInspection', { holders: names.join(', ') })}</p>;
}
