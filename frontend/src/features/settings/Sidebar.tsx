/** Every settings scope uses Navigation for its sections, sharing native identity reads without an org tree. */
import { useTranslation } from 'react-i18next';
import { useQuery } from '@tanstack/react-query';
import { Link } from '@tanstack/react-router';
import { ShieldUser } from 'lucide-react';
import { adminModeQuery } from '@/api/adminMode';
import { nodeQuery, orgBySlugQuery, projectQuery, topicQuery } from '@/api/nodes';
import { isOrgIdAddress } from '@/features/navigation/orgAddress';

import { managementSections, personalSections, placeSettingsSections, type PlaceSettingsNavigation, type SettingsNavigation } from '@/features/settings/navigation';
import { SectionNav } from '@/ui/SectionNav';
import { ScrollArea } from '@/ui/ScrollArea';
import { SkeletonRows } from '@/ui/Skeleton';
import { ErrorState } from '@/ui/ErrorState';
import { useViewShown } from '@/features/workbench/viewOpening';

export function SettingsSidebar({ scope }: { scope: SettingsNavigation }) {
  const { t } = useTranslation();
  if (scope.kind === 'org' || scope.kind === 'project' || scope.kind === 'topic') return <PlaceSettingsSidebar scope={scope} />;
  if (scope.kind === 'system') return <SystemSettingsSidebar scope={scope} />;
  return <ScrollArea data-settings-sidebar className="h-full bg-bg-subtle p-3">
    <SectionNav presentation="sidebar" label={t('settings.personal.nav')}
      current={scope.current} items={personalSections(t)} />
  </ScrollArea>;
}

/** An administrator who has not enabled their reach is on the access gate, not Overview. */
function SystemSettingsSidebar({ scope }: { scope: Extract<SettingsNavigation, { kind: 'system' }> }) {
  const { t } = useTranslation();
  const mode = useQuery({ ...adminModeQuery, enabled: false }).data;
  if (mode === undefined) return <SkeletonRows rows={3} className="p-3" />;
  const items = mode.as_admin ? managementSections(t) : [{ id: 'access', label: t('workbench.areaNavigation.system.entry.name'),
    icon: <ShieldUser aria-hidden />, link: <Link to="/admin" aria-description={t('workbench.areaNavigation.system.entry.description')} activeOptions={{ exact: true, includeSearch: true }} /> }];
  return <ScrollArea data-settings-sidebar className="h-full bg-bg-subtle p-3">
    <SectionNav presentation="sidebar" label={t('admin.nav.label')} current={mode.as_admin ? scope.current : 'access'} items={items} />
  </ScrollArea>;
}

function PlaceSettingsSidebar({ scope }: { scope: PlaceSettingsNavigation }) {
  const { t } = useTranslation();
  const shown = useViewShown();
  const address = scope.kind === 'org' ? scope.address : '';
  const byId = isOrgIdAddress(address);
  // These are the page's canonical native reads. A shown cold drawer can own the same read.
  const idOrg = useQuery({ ...nodeQuery(byId ? address : ''), enabled: shown && byId });
  const org = useQuery({ ...orgBySlugQuery(address), enabled: shown && scope.kind === 'org' && !byId });
  const project = useQuery({ ...projectQuery(scope.kind === 'project' ? scope.id : ''), enabled: shown && scope.kind === 'project' });
  const topic = useQuery({ ...topicQuery(scope.kind === 'topic' ? scope.id : ''), enabled: shown && scope.kind === 'topic' });
  const query = scope.kind === 'org' ? byId ? idOrg : org : scope.kind === 'project' ? project : topic;
  if (query.isPending) return <SkeletonRows rows={4} className="p-3" />;
  if (query.isError) return <ErrorState compact error={query.error} onRetry={() => void query.refetch()} />;
  const node = query.data;
  if (scope.kind === 'org' && node.kind !== 'org') return null;
  const canonical = scope.kind === 'org' && node.kind === 'org' ? { ...scope, address: node.address } : scope;
  const items = placeSettingsSections(canonical, t, node.kind === 'org' && node.flat);
  const current = items.some((item) => item.id === scope.current) ? scope.current : 'general';
  return <ScrollArea data-settings-sidebar className="h-full bg-bg-subtle p-3">
    <SectionNav presentation="sidebar" label={t('settings.org.sections', { name: node.name })} current={current} items={items} />
  </ScrollArea>;
}
