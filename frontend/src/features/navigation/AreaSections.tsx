/** Page-local Help and Usage section links, independent of global navigation. */
import { useRouterState } from '@tanstack/react-router';
import { useTranslation } from 'react-i18next';
import { helpSection, helpSections } from '@/features/help/navigation';
import { usageSections,USAGE_VIEWS } from '@/features/usage/navigation';
import { readSearch } from '@/features/usage/search';
import { useViewChoice,useRememberedView } from '@/lib/viewChoices';
import { ScrollArea } from '@/ui/ScrollArea';
import { SectionNav } from '@/ui/SectionNav';

export function AreaSections({ area }: { area: 'help' | 'usage' }) {
  const { t } = useTranslation();
  const location = useRouterState({ select: (state) => state.location });
  const search = location.search as Record<string, unknown>;
  const [savedHelp] = useViewChoice<string>('help', 'part', 'articles');
  const report = readSearch(search);
  const [usageView]=useRememberedView('usage','tab',area==='usage' ? report.view : undefined,'usage',USAGE_VIEWS);
  const current = area === 'help' ? helpSection(search.part) ?? helpSection(savedHelp) ?? 'articles' : usageView;
  const label = t(area === 'help' ? 'workbench.areaNavigation.help.sections' : 'usagePerf.views');
  const items = area === 'help' ? helpSections(t) : usageSections(report, t);
  return <ScrollArea className="h-full bg-bg-subtle p-3" data-area-sections={area}>
    <SectionNav presentation="sidebar" label={label} current={current} items={items} />
  </ScrollArea>;
}
