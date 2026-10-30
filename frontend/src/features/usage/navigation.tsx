/** Usage section links keep the current scope, dates and filters rather than reopening a default report. */
import { Link } from '@tanstack/react-router';
import type { TFunction } from 'i18next';
import { ChartColumn, Gauge, List } from 'lucide-react';
import { patchSearch, type UsageSearch } from '@/features/usage/search';
import type { SectionNavItem } from '@/ui/SectionNav';

export const USAGE_VIEWS = ['usage', 'performance', 'requests'] as const;
const ICONS = { usage: ChartColumn, performance: Gauge, requests: List };

export function usageSections(search: UsageSearch, t: TFunction): SectionNavItem[] {
  return USAGE_VIEWS.map((view) => {
    const Icon = ICONS[view];
    return { id: view, label: t(`usagePerf.viewNames.${view}`), icon: <Icon aria-hidden />,
      link: <Link to="/usage" search={patchSearch(search,{view,show:undefined})}
        activeOptions={{ exact: true, includeSearch: true, explicitUndefined: true }} /> };
  });
}
