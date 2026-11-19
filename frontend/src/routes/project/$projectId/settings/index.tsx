/** Project settings. Loader, permission handling and search schema are pending.
 *
 * The settings of a project: its general settings, the settings editor at its scope, its tags and where
 * what happens in it is sent. The section in view is in the address, so that a link can name it.
 */
import { usePageParams, usePageSearch } from '@/features/workbench/pageContext';
import { createFileRoute } from '@tanstack/react-router';

import { traceSearch, type TraceSearch } from '@/features/trace/params';
import { isProjectSection, type ProjectSection } from '@/features/settings/sections';
import { ProjectSettings } from '@/features/settings/ScopeSettings';

interface ProjectSettingsSearch extends TraceSearch {
  section?: ProjectSection;
  setting?: string;
}

export const Route = createFileRoute('/project/$projectId/settings/')({
  staticData: { screen: 'page' },
  validateSearch: (search: Record<string, unknown>): ProjectSettingsSearch =>
    ({ ...traceSearch(search), ...(isProjectSection(search.section) && search.section !== 'general' ? { section: search.section } : {}),...(typeof search.setting === 'string' && search.setting !== '' ? {setting:search.setting} : {}) }),
  component: Screen,
});

function Screen() {
  const { projectId } = usePageParams(Route);
  const { section,setting } = usePageSearch(Route);
  return <ProjectSettings projectId={projectId} section={section ?? (setting === undefined ? 'general' : 'policies')} />;
}
