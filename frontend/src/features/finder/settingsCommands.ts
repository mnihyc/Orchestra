/** Address every actual settings section, with the same names and destinations as its frame. */
import type { TFunction } from 'i18next';
import { pathOf, type Destination } from '@/features/finder/destinations';
import { sectionLabel } from '@/features/settings/sectionLabels';
import { orgSections, PROJECT_SECTIONS, TOPIC_SECTIONS } from '@/features/settings/sections';

export interface SettingsPlace { id: string; name: string; kind: 'org' | 'project' | 'topic'; address?: string; flat?: boolean }
export function settingsDestinations(place: SettingsPlace, t: TFunction): { id: string; pattern: string; path: string; title: string }[] {
  const sections = place.kind === 'org' ? orgSections(place.flat) : place.kind === 'project' ? PROJECT_SECTIONS : TOPIC_SECTIONS;
  return sections.map((section) => {
    const label = sectionLabel(section, t);
    const destination: Destination = place.kind === 'org' ? { to: '/org/$slug/settings/$section', params: { slug: place.address ?? place.id, section } }
      : place.kind === 'project' ? { to: '/project/$projectId/settings', params: { projectId: place.id }, search: { section } }
        : { to: '/topic/$topicId', params: { topicId: place.id }, search: { tab: 'settings', section } };
    return { id: section, pattern: destination.to, path: pathOf(destination), title: t('commands.settingsSection', { scope: place.name, section: label }) };
  });
}
