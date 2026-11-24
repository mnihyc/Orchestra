/** Every mount copy offers the same choices for data already at its destination. */
import { useTranslation } from 'react-i18next';
import type { CopyInto } from '@/generated/api-types';
import { Select } from '@/ui/Select';

export function CopyDestination({ mount, value, replace, onChange, disabled = false }: {
  mount: string; value: CopyInto | null; replace: boolean; onChange: (value: CopyInto) => void; disabled?: boolean;
}) {
  const { t } = useTranslation();
  return <Select<CopyInto | 'unanswered'> size="sm" hideLabel className="w-36" label={t('mountPlan.fates.into', { mount })} value={value ?? 'unanswered'} disabled={disabled}
    onValueChange={(into) => { if (into !== 'unanswered') onChange(into); }} options={[
      { value: 'unanswered', name: t('liveWork.choose'), disabled: true },
      { value: 'replace', name: t('mountPlan.fates.replace'), disabled: !replace, description: replace ? undefined : t('mountPlan.fates.replaceUnavailable') },
      { value: 'new_location', name: t('mountPlan.fates.keepBoth') },
    ]} />;
}
