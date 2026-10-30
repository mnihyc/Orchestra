/** Tree and Recent share one compact switch and the same Opened/Active choice at both widths. */
import { History, ListTree } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import type { NavigationArrangement, RecentOrder } from './recent';
import { Button } from '@/ui/Button';
import { IconButton } from '@/ui/IconButton';
import { Select } from '@/ui/Select';
import { Hint } from '@/features/help/Hint';

const ARRANGEMENTS = ['tree', 'recent'] as const;

export function ArrangementSwitch({ value, onChange, wide = false }: { value: NavigationArrangement; onChange: (value: NavigationArrangement) => void; wide?: boolean }) {
  const { t } = useTranslation();
  return <span className="inline-flex shrink-0 items-center gap-1"><span role="group" aria-label={t('navigation.arrangement.label')} className="inline-flex shrink-0 items-center gap-0.5 rounded-md bg-bg-muted p-0.5">
    {ARRANGEMENTS.map((choice) => {
      const icon = choice === 'tree' ? <ListTree aria-hidden /> : <History aria-hidden />;
      const selected = choice === value;
      return wide ? <Button key={choice} size="sm" variant="ghost" icon={icon} aria-pressed={selected} className={selected ? 'bg-surface shadow-sm' : undefined} onClick={() => onChange(choice)}>{t(`navigation.arrangement.${choice}`)}</Button>
        : <IconButton key={choice} size="sm" label={t(`navigation.arrangement.${choice}`)} icon={icon} pressed={selected} className={selected ? 'bg-surface shadow-sm' : undefined} onClick={() => onChange(choice)} />;
    })}
  </span>{wide && <Hint id="navigation-arrangements" popover />}</span>;
}

export function RecentOrderSelect({ value, onChange }: { value: RecentOrder; onChange: (value: RecentOrder) => void }) {
  const { t } = useTranslation();
  return <Select size="sm" hideLabel label={t('navigation.recentOrder.label')} value={value} onValueChange={onChange} fieldClassName="shrink-0" options={[
    { value: 'opened', name: t('navigation.recentOrder.opened'), description: t('navigation.recentOrder.openedDescription') },
    { value: 'active', name: t('navigation.recentOrder.active'), description: t('navigation.recentOrder.activeDescription') },
  ]} />;
}
