/** The settings of a project and of a topic: the same sections an org has that apply to them, drawn from the same parts.
 *
 * A project has its general settings, the settings editor at its scope, its tags and where what
 * happens in it is sent; a topic has its general settings, the editor at its scope, which is what new
 * threads and new environments there start with, and the same destinations, in the same order: both open
 * on the first. Nothing here is a copy: each section is the one an org's settings use, given the project or the topic.
 */
import { useQuery } from '@tanstack/react-query';
import { Diamond, Folder, Settings } from 'lucide-react';
import { useEffect } from 'react';
import { useTranslation } from 'react-i18next';
import { useStore } from 'zustand';

import { client } from '@/api/client';
import { projectQuery } from '@/api/nodes';
import { Trail } from '@/features/trace/Trail';
import { scopeOf } from '@/features/trace/params';
import { OutsideOrch } from '@/features/settings/OutsideOrch';
import { SettingsFrame } from '@/features/settings/Frame';
import { placeSettingsSections } from '@/features/settings/navigation';
import { sectionLabel } from '@/features/settings/sectionLabels';
import { useSettingsTabTitle } from '@/features/workbench/settingsTabTitle';
import { SettingsEditor } from '@/features/settings/Editor';
import { General } from '@/features/settings/org/General';
import { Tags } from '@/features/settings/org/Tags';
import type { RowPlace } from '@/features/settings/SettingRow';
import type { Project, Topic } from '@/generated/api-types';
import { selectOrg } from '@/shell/org';
import { Page, PlaceMark, Section } from '@/shell/Page';
import { useScreenHeading } from '@/shell/screen';
import { ErrorState } from '@/ui/ErrorState';
import { Skeleton, SkeletonGroup } from '@/ui/Skeleton';

/** The sections of a project's and a topic's settings, with the checks their routes read from the address, are `sections.ts`'s. */
import { isProjectSection, isTopicSection, PROJECT_SECTIONS, type ProjectSection, TOPIC_SECTIONS, type TopicSection } from '@/features/settings/sections';
export { isProjectSection, isTopicSection, PROJECT_SECTIONS, type ProjectSection, TOPIC_SECTIONS, type TopicSection };

function placeOf(node: Project | Topic): RowPlace {
  return { name: node.name, owner: node.owner, access: node.access, kind: node.kind };
}

/** The editor at a node's scope, with the secrets of the place. */
export function ScopeEditor({ node }: { node: Project | Topic }) {
  const { t } = useTranslation();
  return (
    <Section id="policies" title={t('day100.names.defaults')}>
      <SettingsEditor target={{ kind: 'node', id: node.id }} scope={node.kind} place={placeOf(node)} secrets />
    </Section>
  );
}

export interface TopicScopeSettingsProps {
  topic: Topic;
  section: TopicSection;
  onSection: (section: TopicSection) => void;
}

/** The settings in a topic's tab: its general settings, the editor at topic scope, and where what happens in the topic is sent; the control above them says which is in view. */
export function TopicScopeSettings({ topic, section }: TopicScopeSettingsProps) {
  const { t } = useTranslation();
  const navigation = placeSettingsSections({ kind: 'topic', id: topic.id, current: section }, t);
  return (
    <SettingsFrame title={sectionLabel(section,t)} mark={<PlaceMark><Diamond aria-hidden /></PlaceMark>}
      adminAccess={topic.access.admin_access} private={topic.page === 'restricted'} opening={{ kind: topic.kind, id: topic.id }}
      label={t('settings.org.sections', { name: topic.name })} current={section} items={navigation}>
      {section === 'general' && <General key={topic.id} node={topic} />}
      {section === 'policies' && <ScopeEditor node={topic} />}
      {section === 'audit' && <Trail scope={scopeOf(topic)} />}
      {section === 'destinations' && <OutsideOrch node={topic} />}
    </SettingsFrame>
  );
}

export interface ProjectSettingsProps {
  projectId: string;
  section: ProjectSection;
}

export function ProjectSettings({ projectId, section }: ProjectSettingsProps) {
  const { t } = useTranslation();
  const userId = useStore(client.session, (state) => state.userId);
  const query = useQuery(projectQuery(projectId));
  const project = query.data;
  const readable = query.isError ? undefined : project;
  const tabTitle = useSettingsTabTitle(sectionLabel(section, t), readable);
  useEffect(() => {
    if (project !== undefined) selectOrg(userId, project.org_id);
  }, [project, userId]);
  useScreenHeading({ title: tabTitle, tabTitle, tabPending: query.isPending, icon: Settings, nodeId: readable?.id, path: readable?.path, leaf: true, trail: [{ label: t('screens.sections.settings'), to: `/project/${projectId}/settings` }] });

  if (query.isPending) {
    return (
      <Page>
        <SkeletonGroup className="flex flex-col gap-4">
          <Skeleton className="h-8 w-1/3" />
          <Skeleton className="h-[10rem] w-full" />
        </SkeletonGroup>
      </Page>
    );
  }
  if (query.isError) {
    return <ErrorState className="h-full" error={query.error} retrying={query.isRefetching} onRetry={() => void query.refetch()} />;
  }
  const data = query.data;
  const navigation = placeSettingsSections({ kind: 'project', id: projectId, current: section }, t);
  return (
    <SettingsFrame
        adminAccess={data.access.admin_access} private={data.page === 'restricted'} opening={{ kind: data.kind, id: data.id }}
        mark={
          <PlaceMark>
            <Folder aria-hidden />
          </PlaceMark>
        }
        title={sectionLabel(section,t)}
        label={t('settings.org.sections', { name: data.name })} current={section}
        items={navigation}>
            {section === 'general' && <General key={data.id} node={data} />}
            {section === 'policies' && <ScopeEditor node={data} />}
            {section === 'tags' && <Tags node={data} />}
            {section === 'audit' && <Trail scope={scopeOf(data)} />}
            {section === 'destinations' && <OutsideOrch node={data} />}
    </SettingsFrame>
  );
}
