/** Search uses the same compact toolbar on pages and inside collection dialogs. */
import { useTranslation } from 'react-i18next';
import { FilterBar } from '@/ui/FilterBar';
import { Input } from '@/ui/Input';

export function CollectionSearch({ name, label, text, onText }: { name: string; label?: string | undefined; text: string; onText: (value: string) => void }) {
  const { t } = useTranslation();
  return <FilterBar search={<Input type="search" hideLabel label={label ?? t('ui.collection.find', { name })} placeholder={t('ui.collection.search')} value={text} onChange={(event) => onText(event.target.value)} autoComplete="off" />} />;
}
