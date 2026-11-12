/** Notification choices stay in familiar areas; changing one area preserves every other choice. */
import { useTranslation } from 'react-i18next';

import { ALL_KINDS, KIND_AREA, KIND_AREAS, KIND_LOOK } from '@/features/inbox/kinds';
import type { NotificationKind } from '@/generated/api-types';
import { CheckboxList } from '@/ui/Checkbox';

export function NotificationKinds({ disabledKinds, onChange }: { disabledKinds: readonly NotificationKind[]; onChange: (disabled: NotificationKind[]) => void }) {
  const { t } = useTranslation();
  return <div className="grid min-w-0 gap-4 sm:grid-cols-2">
    {KIND_AREAS.map((area) => {
      const kinds = ALL_KINDS.filter((kind) => KIND_AREA[kind] === area);
      return <CheckboxList key={area} label={t(`notifications.kindAreas.${area}`)} presentation="compact"
        options={kinds.map((kind) => ({ value: kind, label: t(KIND_LOOK[kind].label) }))}
        value={kinds.filter((kind) => !disabledKinds.includes(kind))}
        onValueChange={(enabled) => onChange(ALL_KINDS.filter((kind) => KIND_AREA[kind] === area ? !enabled.includes(kind) : disabledKinds.includes(kind)))} />;
    })}
  </div>;
}
