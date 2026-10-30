/** Management opens the same main-area hierarchy with an explicit whole-system scope. */
import { useTranslation } from 'react-i18next';

import type { SectionProps } from '@/features/admin/sections';
import { Button } from '@/ui/Button';

export function Everything({ search }: SectionProps) {
  const { t } = useTranslation();
  const query = new URLSearchParams({ scope: 'system', ...(search.q === undefined ? {} : { q: search.q }) });
  return <Button asChild><a href={`/everything?${query.toString()}`}>{t('everything.openSystem')}</a></Button>;
}
