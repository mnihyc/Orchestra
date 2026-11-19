/** The item's retained storage facts, without its rejected source contents. */
import { useTranslation } from 'react-i18next';
import type { Item } from '@/generated/api-types';
import { FactDisclosure } from '@/ui/FactDisclosure';

export function ItemStorageFacts({ item }: { item: Item }) {
  const { t } = useTranslation();
  const count = item.text_replacements ?? 0;
  const gap = item.storage_gap;
  const incomplete = gap !== undefined && gap !== null;
  if (count === 0 && !incomplete) return null;
  return <FactDisclosure label={t('timeline.storage.details')}
    fragment={t(incomplete ? 'timeline.storage.incomplete' : 'timeline.storage.repaired')}
    groups={[{ key: 'storage', kind: 'recorded_outcome', facts: [
      ...(count > 0 ? [{ key: 'replacements', label: t('timeline.storage.replacements'), value: count.toLocaleString() }] : []),
      ...(!incomplete ? [] : [
        { key: 'reason', label: t('timeline.storage.reason'), value: gap.sqlstate },
        ...(gap.event_seq === null ? [] : [{ key: 'event', label: t('timeline.storage.event'), value: gap.event_seq.toLocaleString() }]),
      ]),
    ] }]} />;
}
