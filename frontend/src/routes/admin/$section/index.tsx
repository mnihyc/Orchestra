/** System management sections. Loader, permission handling and search schema are pending.
 *
 * The screen is Management: one section of the system's own screens, named in the address, with the
 * search or the filter of a list in the address too (`q`, `status`, `span`, ...), so that a link
 * reproduces what was in view. An address that names no section is no screen. Who may be here is
 * decided by the screen: anybody else reads the refusal, and never meets the entry.
 */
import { usePageParams, usePageSearch, usePageNavigate } from '@/features/workbench/pageContext';
import { createFileRoute, notFound } from '@tanstack/react-router';

import { Management } from '@/features/admin/Management';
import { readManagementRouteSearch, readSearch, withSearch } from '@/features/admin/search';
import { isSection } from '@/features/admin/sections';

export const Route = createFileRoute('/admin/$section/')({
  staticData: { screen: 'page' },
  validateSearch: readManagementRouteSearch,
  beforeLoad: ({ params }) => {
    if (!isSection(params.section)) throw notFound();
  },
  component: AdminRoute,
});

function AdminRoute() {
  const { section } = usePageParams(Route);
  // The shared URL also carries Audit filters. Other sections retain their own narrow rules;
  // the embedded trail reads its addressed URL through traceSearch instead of these props.
  const search = readSearch({ ...usePageSearch(Route) });
  const navigate = usePageNavigate(Route);
  return (
    <Management
      section={section}
      search={search}
      onSearch={(patch, options) => void navigate({ search: withSearch(search, patch), replace: options?.replace ?? true })}
      onSection={(next) => void navigate({ to: '/admin/$section', params: { section: next }, search: {} })}
    />
  );
}
