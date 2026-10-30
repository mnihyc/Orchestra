/** The frame of Management: who may be here, the list of its sections, and the section in view with its name and what it is for.
 *
 * Management is for a system admin. Anybody else never meets its entry, and a direct address shows
 * the refusal, in words, with what it needs. For an admin the sections stand in a list at the side
 * (the same destinations use a select below 600px); each is its own chunk, drawn when it is first
 * opened, and a section that fails to arrive or to draw says so in its own place, with a way to try
 * again, while the list and everything else go on.
 */
import { useQuery } from '@tanstack/react-query';
import { Link } from '@tanstack/react-router';
import { ShieldUser } from 'lucide-react';
import { type ComponentType, lazy, type LazyExoticComponent, Suspense } from 'react';
import { useTranslation } from 'react-i18next';

import { meQuery } from '@/api/auth';
import { AdminModeEnableAction, useAdminMode } from '@/features/access/AdminMode';
import { isSection, sectionOf, type SectionId, type SectionProps } from '@/features/admin/sections';
import { SettingsFrame } from '@/features/settings/Frame';
import { managementSections } from '@/features/settings/navigation';
import { Page, PageHeader } from '@/shell/Page';
import { useScreenHeading } from '@/shell/screen';
import { Button } from '@/ui/Button';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorBoundary } from '@/ui/ErrorBoundary';
import { ErrorState } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';

type Body = LazyExoticComponent<ComponentType<SectionProps>>;

/** Each section is its own chunk: Management is opened by few, and a chart library or a long form is not for the rest. */
const BODIES: Record<SectionId, Body> = {
  system: lazy(() => import('@/features/admin/System').then((module) => ({ default: module.System }))),
  overview: lazy(() => import('@/features/admin/Overview').then((module) => ({ default: module.Overview }))),
  users: lazy(() => import('@/features/admin/Users').then((module) => ({ default: module.Users }))),
  orgs: lazy(() => import('@/features/admin/Organizations').then((module) => ({ default: module.Organizations }))),
  everything: lazy(() => import('@/features/admin/Everything').then((module) => ({ default: module.Everything }))),
  'sign-in': lazy(() => import('@/features/admin/SignIn').then((module) => ({ default: module.SignIn }))),
  email: lazy(() => import('@/features/admin/Channels').then((module) => ({ default: module.Channels }))),
  hosts: lazy(() => import('@/features/admin/Hosts').then((module) => ({ default: module.Hosts }))),
  runtimes: lazy(() => import('@/features/admin/Runtimes').then((module) => ({ default: module.Runtimes }))),
  templates: lazy(() => import('@/features/admin/Templates').then((module) => ({ default: module.Templates }))),
  mcp: lazy(() => import('@/features/admin/Mcp').then((module) => ({ default: module.Mcp }))),
  capabilities: lazy(() => import('@/features/admin/Capabilities').then((module) => ({ default: module.Capabilities }))),
  harnesses: lazy(() => import('@/features/admin/Harnesses').then((module) => ({ default: module.Harnesses }))),
  agents: lazy(() => import('@/features/admin/Agents').then((module) => ({ default: module.Agents }))),
  plugins: lazy(() => import('@/features/admin/Plugins').then((module) => ({ default: module.Plugins }))),
  defaults: lazy(() => import('@/features/admin/AccessDefaults').then((module) => ({ default: module.AccessDefaults }))),
  rates: lazy(() => import('@/features/admin/Rates').then((module) => ({ default: module.Rates }))),
  settings: lazy(() => import('@/features/admin/Settings').then((module) => ({ default: module.Settings }))),
  storage: lazy(() => import('@/features/admin/Storage').then((module) => ({ default: module.Storage }))),
  'semantic-search': lazy(() => import('@/features/admin/SemanticSearch').then((module) => ({ default: module.SemanticSearch }))),
  pki: lazy(() => import('@/features/admin/Pki').then((module) => ({ default: module.Pki }))),
  audit: lazy(() => import('@/features/admin/Audit').then((module) => ({ default: module.Audit }))),
};

/** What a person who is no system admin reads at the address of Management. */
function Refusal() {
  const { t } = useTranslation();
  return (
    <EmptyState
      className="h-full"
      titleAs="h1"
      icon={<ShieldUser aria-hidden />}
      title={t('admin.refusal.title')}
      description={t('admin.refusal.lead')}
      action={
        <Button asChild>
          <Link to="/">{t('admin.refusal.home')}</Link>
        </Button>
      }
    />
  );
}

export interface ManagementProps extends SectionProps {
  /** The segment of the address; anything that is no section is no screen, and the route has refused it before. */
  section: string;
  onSection: (section: SectionId) => void;
}

export function Management({ section, search, onSearch }: ManagementProps) {
  const { t } = useTranslation();
  const me = useQuery(meQuery);
  const mode = useAdminMode();
  const id: SectionId = isSection(section) ? section : 'overview';
  const { icon: Icon } = sectionOf(id);
  const allowed = me.data?.is_system_admin === true && mode.data?.as_admin === true;
  // The title bar: Management, then the section in view; the way back to the first section is its first step.
  useScreenHeading(
    allowed
      ? { title: t(`admin.sections.${id}.title`), icon: Icon, trail: [{ label: t('admin.title'), to: '/admin/overview' }] }
      : { title: t('admin.title'), icon: ShieldUser },
  );

  if (me.isPending) return <SkeletonRows className="p-6" rows={4} />;
  if (me.isError) return <ErrorState error={me.error} retrying={me.isRefetching} onRetry={() => void me.refetch()} />;
  if (!me.data.is_system_admin) return <Refusal />;
  if (mode.isPending) return <SkeletonRows className="p-6" rows={4} />;
  if (mode.isError) return <ErrorState error={mode.error} onRetry={() => void mode.refetch()} />;
  if (!mode.data.as_admin) return <Page><PageHeader title={t('admin.title')} /><div className="flex flex-wrap items-center gap-2"><AdminModeEnableAction /></div></Page>;

  const Body = BODIES[id];
  return (
    <SettingsFrame title={t(`admin.sections.${id}.title`)} lead={t(`admin.sections.${id}.lead`)}
      label={t('admin.nav.label')} current={id} items={managementSections(t)}>
          <div data-management-content>
          <ErrorBoundary
            resetKey={id}
            fallback={(error, retry) => <ErrorState error={error} onRetry={retry} />}
          >
            <Suspense fallback={<SkeletonRows rows={6} />}>
              <Body search={search} onSearch={onSearch} />
            </Suspense>
          </ErrorBoundary>
          </div>
    </SettingsFrame>
  );
}
