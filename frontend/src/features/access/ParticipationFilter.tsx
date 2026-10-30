/** An ordinary focus filter narrows the displayed work, without changing anyone's authority or subscriptions. */
import { useTranslation } from 'react-i18next';
import { Switch } from '@/ui/Switch';

export function useParticipationCopy() {
  const { t } = useTranslation();
  return { name: t('adminReach.focus.name'), description: t('adminReach.focus.description') };
}

export function ParticipationFilter({ checked, onCheckedChange }: { checked: boolean; onCheckedChange: (checked: boolean) => void }) {
  const { t } = useTranslation();
  return <Switch label={t(checked ? 'adminReach.focus.only' : 'adminReach.focus.all')} checked={checked} onCheckedChange={onCheckedChange} />;
}
