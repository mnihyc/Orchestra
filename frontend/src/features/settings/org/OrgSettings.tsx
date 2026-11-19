/** The settings of an org at any level: its sections in a column beside the one in view, the same at every level.
 *
 * A root org and a department are the same thing here: each has its general settings, its members,
 * its departments, groups, roles, policies, tags and the rest, and a department's page is reached from
 * its parent's. The column names every section, and the section in view is in the address, so that a
 * link can name it. What a section needs of whoever opens it is said inside it, never by leaving it
 * out of the column: a reader who may not change a thing reads it, with what it would take.
 */
import { useQuery } from '@tanstack/react-query';
import { Settings } from 'lucide-react';
import { Activity, useEffect } from 'react';
import { useTranslation } from 'react-i18next';
import { useStore } from 'zustand';

import { client } from '@/api/client';
import { orgBySlugQuery } from '@/api/nodes';
import { useRetainedOrg } from '@/features/org/useRetainedOrg';
import { orgColor, orgInitial } from '@/features/navigation/OrgSwitcher';
import { Audit } from '@/features/settings/org/Audit';
import { Departments } from '@/features/settings/org/Departments';
import { OrgEverything } from '@/features/settings/org/Everything';
import { General } from '@/features/settings/org/General';
import { Groups } from '@/features/settings/org/Groups';
import { Members } from '@/features/settings/org/Members';
import { Policies } from '@/features/settings/org/Policies';
import { RolesAndDefaults } from '@/features/settings/org/RolesAndDefaults';
import { ServiceAccounts } from '@/features/settings/org/ServiceAccounts';
import { Tags } from '@/features/settings/org/Tags';
import { OutsideOrch } from '@/features/settings/OutsideOrch';
import { SettingsFrame } from '@/features/settings/Frame';
import { placeSettingsSections } from '@/features/settings/navigation';
import { sectionLabel } from '@/features/settings/sectionLabels';
import { useSettingsTabTitle } from '@/features/workbench/settingsTabTitle';
import type { Org } from '@/generated/api-types';
import { selectOrg } from '@/shell/org';
import { Page } from '@/shell/Page';
import { useScreenHeading } from '@/shell/screen';
import { Avatar } from '@/ui/Avatar';
import { ErrorState } from '@/ui/ErrorState';
import { Skeleton, SkeletonGroup } from '@/ui/Skeleton';

/** The sections of an org's settings, in the order a person looks for them. */
import { ORG_SECTIONS, orgSections } from '@/features/settings/sections';
export { ORG_SECTIONS } from '@/features/settings/sections';

export type OrgSection = (typeof ORG_SECTIONS)[number];

export function isOrgSection(value: string): value is OrgSection {
  return (ORG_SECTIONS as readonly string[]).includes(value);
}

/** The sections an org has: a department has all of them, but an org that holds no org inside it has no Departments. */
export function sectionsOf(org: Pick<Org, 'flat' | 'access'>): readonly OrgSection[] {
  // Everything shows each reader only what they may see.
  return orgSections(org.flat);
}

function SectionBody({ org, section }: { org: Org; section: OrgSection }) {
  switch (section) {
    case 'general':
      return <General node={org} />;
    case 'departments':
      return <Departments org={org} />;
    case 'members':
      return <Members org={org} />;
    case 'groups':
      return <Groups org={org} />;
    case 'roles':
      return <RolesAndDefaults org={org} />;
    case 'policies':
      return <Policies org={org} />;
    case 'tags':
      return <Tags node={org} />;
    case 'service-accounts':
      return <ServiceAccounts org={org} />;
    case 'audit':
      return <Audit org={org} />;
    case 'everything':
      return <OrgEverything org={org} />;
    case 'channels':
    case 'destinations':
      return <OutsideOrch node={org} />;
  }
}

export interface OrgSettingsProps {
  /** A root org's short name, or the address of an org inside one. */
  slug: string;
  section: string;
}

export function OrgSettings({ slug, section }: OrgSettingsProps) {
  const { t } = useTranslation();
  const userId = useStore(client.session, (state) => state.userId);
  const query = useQuery(orgBySlugQuery(slug));
  const org = query.isSuccess ? query.data : undefined;
  const data = useRetainedOrg(JSON.stringify([userId, slug]), org, query.isPending);
  const readable = org;
  const sections=data===undefined ? ORG_SECTIONS : sectionsOf(data);
  const current:OrgSection=section==='destinations' ? 'channels' : isOrgSection(section)&&sections.includes(section) ? section : 'general';

  const tabTitle = useSettingsTabTitle(sectionLabel(current, t), readable);

  // The root org of what is in view is the one whose navigation the left bar shows.
  useEffect(() => {
    if (org !== undefined) selectOrg(userId, org.org_id);
  }, [org, userId]);
  useScreenHeading({ title: tabTitle, tabTitle, tabPending: query.isPending, icon: Settings, nodeId: readable?.id, path: readable?.path, leaf: true, trail: [{ label: t('screens.sections.settings'), to: `/org/${slug}/settings/general` }] });

  const pending = <Page>
    <SkeletonGroup className="flex flex-col gap-4">
      <Skeleton className="h-8 w-1/3" />
      <Skeleton className="h-[10rem] w-full" />
    </SkeletonGroup>
  </Page>;
  const failure = query.isError ? <ErrorState className="h-full" error={query.error} retrying={query.isRefetching} onRetry={() => void query.refetch()} /> : null;
  if (data === undefined) return failure ?? pending;
  const own = data.org_kind === 'personal';
  const navigation = placeSettingsSections({ kind: 'org', address: data.address, current }, t, data.flat);
  return (
    <>
    {query.isPending && pending}
    <Activity key={JSON.stringify([userId, data.id])} mode={query.isSuccess ? 'visible' : 'hidden'}>
    <SettingsFrame
        adminAccess={data.access.admin_access} private={data.page === 'restricted'} opening={{ kind: data.kind, id: data.id }}
        mark={<Avatar text={orgInitial(data)} color={orgColor(data)} kind={own ? 'human' : 'org'} size={40} />}
        title={sectionLabel(current,t)}
        label={t('settings.org.sections', { name: data.name })} current={current}
        items={navigation}>
            <SectionBody key={`${data.id}-${current}`} org={data} section={current} />
    </SettingsFrame>
    </Activity>
    </>
  );
}
