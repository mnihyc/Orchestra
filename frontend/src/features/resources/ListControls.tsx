/** The list keeps search in the bar and puts its optional criteria and order behind two compact acts. */
import { ArrowDownWideNarrow, ListFilter } from 'lucide-react';
import { type ReactNode, useId } from 'react';
import { useTranslation } from 'react-i18next';

import { ParticipationFilter } from '@/features/access/ParticipationFilter';
import { Button } from '@/ui/Button';
import { compactControls } from '@/ui/FilterBar';
import { Popover } from '@/ui/Popover';

export function ResourceListControls({ filters, sorting, participating, onParticipation, filtered, sorted }: {
  filters?: ReactNode;
  sorting?: ReactNode;
  participating: boolean;
  onParticipation: (value: boolean) => void;
  filtered: boolean;
  sorted: boolean;
}) {
  const { t } = useTranslation();
  const filterDescription = useId(), sortDescription = useId();
  const panel = `flex flex-col gap-3 ${compactControls}`;
  return <>
    <Popover label={t('resources.controls.filter')} align="end" className="w-(--container-form) max-w-[calc(100vw-2rem)]"
      trigger={<Button size="sm" icon={<ListFilter aria-hidden />} aria-label={t('resources.controls.filter')}
        aria-describedby={filtered ? filterDescription : undefined} title={t('resources.controls.filter')}>
        <span className="max-[360px]:sr-only">{t('resources.controls.filter')}</span>{filtered && <><span aria-hidden className="size-1.5 shrink-0 rounded-full bg-accent-text" /><span id={filterDescription} className="sr-only">{t('resources.controls.filtered')}</span></>}
      </Button>}>
      <div data-resource-filter-panel className={panel}>
        <ParticipationFilter checked={participating} onCheckedChange={onParticipation} />
        {filters}
      </div>
    </Popover>
    {sorting !== undefined && <Popover label={t('resources.controls.sort')} align="end" className="w-(--container-form) max-w-[calc(100vw-2rem)]"
      trigger={<Button size="sm" icon={<ArrowDownWideNarrow aria-hidden />} aria-label={t('resources.controls.sort')}
        aria-describedby={sorted ? sortDescription : undefined} title={t('resources.controls.sort')}>
        <span className="max-[360px]:sr-only">{t('resources.controls.sort')}</span>{sorted && <><span aria-hidden className="size-1.5 shrink-0 rounded-full bg-accent-text" /><span id={sortDescription} className="sr-only">{t('resources.controls.sorted')}</span></>}
      </Button>}>
      <div data-resource-sort-panel className={panel}>{sorting}</div>
    </Popover>}
  </>;
}
