/** A skill's page: its versions, its draft and who may see, use and manage it, by tab. */
import { usePageParams, usePageSearch, usePageNavigate } from '@/features/workbench/pageContext';
import { createFileRoute } from '@tanstack/react-router';

import { SkillScreen, SKILL_TABS, type SkillTab } from '@/features/resources/skills/Skill';

export const Route = createFileRoute('/resources/skills/$id/')({
  staticData: { screen: 'page' },
  validateSearch: (search: Record<string, unknown>): { tab?: SkillTab } => {
    const tab = SKILL_TABS.find((candidate) => candidate === search.tab);
    return tab === undefined ? {} : { tab };
  },
  component: Screen,
});

function Screen() {
  const navigate = usePageNavigate(Route);
  return <SkillScreen id={usePageParams(Route).id} tab={usePageSearch(Route).tab ?? 'overview'} onTab={(tab) => { void navigate({ search: { tab }, replace: true }); }} />;
}
