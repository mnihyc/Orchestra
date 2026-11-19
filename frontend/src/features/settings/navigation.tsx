/** One set of real destinations for the personal/system frame and its workbench sidebar. */
import { useViewLocation } from '@/features/workbench/pageContext';
import { Link } from '@tanstack/react-router';
import type { TFunction } from 'i18next';
import { ChartColumn } from 'lucide-react';

import { LISTED, type SectionId } from '@/features/admin/sections';
import { ACTIONS } from '@/ui/actions';
import type { SectionNavItem } from '@/ui/SectionNav';
import { ORG_SECTIONS, PROJECT_SECTIONS, TOPIC_SECTIONS, orgSections } from '@/features/settings/sections';
import { sectionLabel } from '@/features/settings/sectionLabels';

export const PERSONAL_PAGES = [
  { to: '/settings/profile', key: 'profile', Icon: ACTIONS.profile.Icon },
  { to: '/settings/your-page', key: 'yourPage', Icon: ACTIONS.profile.Icon },
  { to: '/settings/security', key: 'security', Icon: ACTIONS.security.Icon },
  { to: '/settings/notifications', key: 'notifications', Icon: ACTIONS.notifications.Icon },
  { to: '/usage', key: 'settingsUsage', Icon: ChartColumn },
  { to: '/settings/tokens', key: 'tokens', Icon: ACTIONS.tokens.Icon },
  { to: '/settings/exports', key: 'exports', Icon: ACTIONS.yourExports.Icon },
  { to: '/settings/imports', key: 'imports', Icon: ACTIONS.import.Icon },
  { to: '/settings/interface', key: 'interface', Icon: ACTIONS.interface.Icon },
] as const;
export type PersonalPage = (typeof PERSONAL_PAGES)[number]['key'];

export function personalTitle(page: PersonalPage, t: TFunction): string {
  return page === 'yourPage' ? t('profile.page.settingsTitle') : t(`screens.${page}.title`);
}
export function personalSections(t: TFunction): SectionNavItem[] {
  return PERSONAL_PAGES.map((entry) => ({ id: entry.key, label: personalTitle(entry.key, t), icon: <entry.Icon className="size-4 shrink-0" aria-hidden />, link: <Link to={entry.to} activeOptions={{ exact: true, includeSearch: true }} /> }));
}
export function managementSections(t: TFunction): SectionNavItem[] {
  return LISTED.map((entry) => ({ id: entry.id, label: t(`admin.sections.${entry.id}.title`), icon: <entry.icon aria-hidden className="size-4 shrink-0" />,
    ...(entry.group === null ? {} : { group: entry.group }), link: <Link to="/admin/$section" params={{ section: entry.id }} activeOptions={{ exact: true, includeSearch: true }} /> }));
}
export type PlaceSettingsNavigation = { kind: 'org'; address: string; current: (typeof ORG_SECTIONS)[number] }
  | { kind: 'project'; id: string; current: (typeof PROJECT_SECTIONS)[number] }
  | { kind: 'topic'; id: string; current: (typeof TOPIC_SECTIONS)[number] };
export type SettingsNavigation = { kind: 'personal'; current: PersonalPage } | { kind: 'system'; current: SectionId } | PlaceSettingsNavigation;

/** Settings is an explicit destination; a plain topic address returns to the topic's content. */
export function settingsNavigation(pathname: string, search: Record<string, unknown> = {}): SettingsNavigation | null {
  const personal = PERSONAL_PAGES.find((entry) => entry.to.startsWith('/settings/') && (entry.to === pathname.replace(/\/$/, '') || pathname.startsWith(`${entry.to}/`)));
  if (personal !== undefined) return { kind: 'personal', current: personal.key };
  if (pathname === '/settings/usage' || pathname === '/settings') return { kind: 'personal', current: pathname.endsWith('/usage') ? 'settingsUsage' : 'profile' };
  if (pathname.startsWith('/settings/')) return { kind: 'personal', current: 'profile' };
  if (pathname === '/admin' || pathname.startsWith('/admin/')) {
    const section = pathname.split('/')[2];
    return { kind: 'system', current: LISTED.find((entry) => entry.id === section)?.id ?? 'overview' };
  }
  const org = /^\/org\/([^/]+)\/settings(?:\/([^/]+))?\/?$/.exec(pathname);
  const project = /^\/project\/([^/]+)\/settings\/?$/.exec(pathname);
  const topic = /^\/topic\/([^/]+)\/?$/.exec(pathname);
  try {
    if (org?.[1] !== undefined) {
      const current = ORG_SECTIONS.find((section) => section === org[2]) ?? 'general';
      return { kind: 'org', address: decodeURIComponent(org[1]), current: current === 'destinations' ? 'channels' : current };
    }
    if (project?.[1] !== undefined) return { kind: 'project', id: decodeURIComponent(project[1]), current: PROJECT_SECTIONS.find((section) => section === search.section) ?? (typeof search.setting === 'string' && search.setting !== '' ? 'policies' : 'general') };
    if (topic?.[1] !== undefined && search.tab === 'settings') return { kind: 'topic', id: decodeURIComponent(topic[1]), current: TOPIC_SECTIONS.find((section) => section === search.section) ?? (typeof search.setting === 'string' ? 'policies' : 'general') };
  } catch { /* A malformed address offers no place navigation. */ }
  return null;
}

export function useSettingsNavigation(): SettingsNavigation | null {
  const location = useViewLocation();
  return settingsNavigation(location.pathname, location.search);
}

/** The page and sidebar share these native links, including the legacy org destination alias. */
export function placeSettingsSections(scope: PlaceSettingsNavigation, t: TFunction, flat = false): SectionNavItem[] {
  if (scope.kind === 'org') return orgSections(flat).map((section) => ({ id: section, label: sectionLabel(section, t), link: <Link to="/org/$slug/settings/$section" params={{ slug: scope.address, section }} activeOptions={{ exact: true, includeSearch: true }} /> }));
  if (scope.kind === 'project') return PROJECT_SECTIONS.map((section) => ({ id: section, label: sectionLabel(section, t), link: <Link to="/project/$projectId/settings" params={{ projectId: scope.id }} search={{ section }} activeOptions={{ exact: true, includeSearch: true }} /> }));
  return TOPIC_SECTIONS.map((section) => ({ id: section, label: sectionLabel(section, t), link: <Link to="/topic/$topicId" params={{ topicId: scope.id }} search={{ tab: 'settings', section }} activeOptions={{ exact: true, includeSearch: true }} /> }));
}
