/** The shared defaults permission grid at system scope, including its guarded change preview. */
import { useTranslation } from 'react-i18next';
import { Defaults } from '@/features/access/Defaults';

export function AccessDefaults() {
  const { t } = useTranslation();
  return <Defaults place={{ kind: 'system', id: 'system', name: t('access.place.system') }} />;
}
