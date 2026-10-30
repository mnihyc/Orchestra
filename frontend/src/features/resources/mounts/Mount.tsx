/** Mount overview, where it appears, whether its data may be written, used-by and sharing.
 * Locations and uses are read separately; a failed read never becomes an empty list.
 */
import { useMutation, useQuery } from '@tanstack/react-query';
import { MOUNT_TABS, type MountTab } from '@/features/resources/tabs';
import { Link, useNavigate } from '@tanstack/react-router';
import { Box, Pencil, Trash2 } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { EmptyState } from '@/ui/EmptyState';
import { deleteMount, mountQuery, mountUsesQuery } from '@/api/resource-tools';
import { entriesOf, shareAct } from '@/features/resources/acts';
import { useResourceCleanup } from '@/features/storage/acts';
import { DetailPages, ResourceDetails, ResourceTabs as Tabs, ResourceHeadingActs as HeadingActs } from '@/features/resources/DetailSections';
import { placePath } from '@/features/navigation/places';
import { Facts } from '@/features/resources/Facts';
import { ResourceChip } from '@/features/resource-facts/ResourceFacts';
import { ResourceAccess } from '@/features/resources/ResourceAccess';
import { useResourceHeading } from '@/features/resources/trail';
import { holds } from '@/lib/permissions';
import { Page, PageHeader, PlaceMark, Section } from '@/shell/Page';
import { Button } from '@/ui/Button';
import { adminSpec } from '@/ui/AdminAct';
import { ConfirmDialog } from '@/ui/ConfirmDialog';
import { ErrorState } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';
import { SettingRow } from '@/ui/SettingRow';
import type { TranslationKey } from '@/i18n';
import { choiceCopy } from '@/i18n/choices';
import { EditMount } from './Edit';
import { Locations } from './Locations';
import { MountWhere } from './Where';

function Uses({ id }: { id: string }) {
  const { t } = useTranslation();
  const query = useQuery(mountUsesQuery(id));
  if (query.data === undefined) return query.error === null ? <SkeletonRows rows={3} /> : <ErrorState compact error={query.error} onRetry={() => void query.refetch()} />;
  const { scopes, environments, hidden } = query.data;
  return <Section id="mount-uses" title={t('resourceDetails.mount.uses')}>
    {scopes.length === 0 && environments.length === 0 && hidden === 0 && <EmptyState compact title={t('resourceDetails.mount.unused')} />}
    <ul>{scopes.map((scope) => <li key={scope.node.id} className="flex flex-wrap gap-x-3 gap-y-1"><Link to={placePath(scope.node)}>{scope.node.name}</Link><span>{scope.value === true ? t('resourceDetails.mount.shared') : scope.value === 'private' ? t('resourceDetails.mount.private') : t('resourceDetails.mount.readOnly')}</span></li>)}</ul>
    <ul>{environments.map((use) => {
      const names: Readonly<Record<string, TranslationKey | undefined>> = { attached: 'resourceDetails.layout.mountState.attached', skipped: 'resourceDetails.layout.mountState.skipped', pending: 'resourceDetails.layout.mountState.pending' };
      const state = Object.hasOwn(names, use.state) ? names[use.state] : undefined;
      return <li key={use.environment.id} className="flex flex-wrap gap-x-3 gap-y-1"><ResourceChip subject={{ kind: 'environment', environment: use.environment }} asChild hint={false}><Link to="/resources/environments/$id" params={{ id: use.environment.id }}>{use.environment.name}</Link></ResourceChip><span>{t(state ?? 'resourceDetails.layout.notReported')}</span>{use.workspace && <span>{t('resourceDetails.mount.workspace')}</span>}</li>;
    })}</ul>
    {hidden > 0 && <p className="text-sm text-text-muted">{t('resourceDetails.mount.hidden', { count: hidden })}</p>}
  </Section>;
}

export { MOUNT_TABS, type MountTab };

export function MountScreen({ id, tab, onTab }: { id: string; tab?: MountTab; onTab?: (tab: MountTab) => void }) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const cleanup = useResourceCleanup();
  const query = useQuery(mountQuery(id));
  const back = useResourceHeading('mounts', Box, query.data?.name ?? t('common.loading'), query.data?.id);
  const [editing, setEditing] = useState(false);
  const [removing, setRemoving] = useState(false);
  const [localTab, setLocalTab] = useState<MountTab>('overview');
  const remove = useMutation({ mutationFn: () => deleteMount(id), onSuccess: () => { void navigate({ to: '/resources/mounts' }); } });
  if (query.data === undefined) return query.error === null ? <SkeletonRows rows={6} /> : <ErrorState error={query.error} onRetry={() => void query.refetch()} />;
  const mount = query.data;
  const manage = holds(mount.access, 'mount.manage');
  const target = { kind: 'mount' as const, id, name: mount.name };
  const shown = tab ?? localTab;
  const edit = manage ? { id: 'edit', label: t('resources.act.editButton'), icon: <Pencil aria-hidden />, run: () => setEditing(true) } : null;
  const menu = entriesOf([shareAct(t, target, mount.access)], [manage ? { id: 'delete', label: t('resourceDetails.remove'), icon: <Trash2 aria-hidden />, danger: true, run: () => setRemoving(true) } : null], [cleanup.act({ kind: 'mount', id }, manage)]);
  const overview = <><Section id="mount-about" title={t('resourceDetails.overview')}><Facts compact label={t('resourceDetails.overview')} rows={[
      { name: t('resources.home.label'), value: mount.home?.name ?? t('resourceDetails.system') },
      { name: t('resourceDetails.mount.target'), value: <code>{mount.target}</code> },
      { name: t('resourceDetails.mount.mode'), value: t(mount.read_only ? 'resourceDetails.mount.readOnly' : 'resourceDetails.mount.shared') },
      { name: t('resourceDetails.enabled'), value: t(mount.enabled ? 'resourceDetails.on' : 'resourceDetails.off') },
      { name: t('resourceDetails.mount.defaultFate'), value: choiceCopy(t, `mountFate.${mount.default_fate}`).name },
      { name: t('resourceDetails.mount.where'), value: <DetailPages items={mount.locations}>{(page) => <MountWhere locations={[...page]} hidden={mount.locations_hidden} />}</DetailPages> },
    ]} /></Section>
    <p className="max-w-prose text-text-muted">{t('resourceDetails.mount.locationExplanation')}</p>
    <ResourceDetails><Facts rows={[{ name: t('resourceDetails.address'), value: mount.address }]} /></ResourceDetails></>;
  return <Page><PageHeader adminAccess={mount.access.admin_access} ownership={mount} opening={{ kind: 'mount', id: mount.id }} back={back} title={mount.name} lead={mount.description} mark={<PlaceMark><Box aria-hidden /></PlaceMark>}
    actions={<HeadingActs admin={adminSpec(mount, 'mount.manage')} name={mount.name} primary={null} edit={shown === 'settings' ? null : edit} menu={menu} />} />
    <Tabs label={t('resourceDetails.layout.tabs')} value={shown} onValueChange={onTab ?? setLocalTab} panelClassName="pt-4" tabs={[
      { value: 'overview', label: t('resourceDetails.overview'), panel: overview },
      { value: 'locations', label: t('resourceDetails.mount.locations'), panel: <Locations mount={mount} /> },
      { value: 'uses', label: t('resourceDetails.mount.uses'), panel: <Uses id={id} /> },
      { value: 'settings', label: t('resourceDetails.layout.settings'), panel: <ul aria-label={t('resourceDetails.layout.configuration')} className="divide-y divide-border rounded-lg border border-border">
        <SettingRow name={t('resourceDetails.layout.configuration')} value={mount.target} readOnly={!manage}
          actions={edit === null ? undefined : <Button size="sm" onClick={edit.run}>{t('resources.act.editButton')}</Button>} />
      </ul> },
      { value: 'access', label: t('agents.tabs.access'), panel: <ResourceAccess access={mount.access} target={target} /> },
    ]} />
    {editing && <EditMount mount={mount} onClose={() => setEditing(false)} />}
    {cleanup.dialogs}
    <ConfirmDialog admin={adminSpec(mount, 'mount.manage')} open={removing} onOpenChange={setRemoving} title={t('resourceDetails.removeTitle', { name: mount.name })} confirmLabel={t('resourceDetails.remove')} danger pending={remove.isPending} error={remove.error} onConfirm={() => remove.mutate()}>{t('resourceDetails.mount.removeHelp')}</ConfirmDialog>
  </Page>;
}
