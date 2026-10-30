/** Resource creation from lists, the Resources sidebar and empty binding choices. */
import { Link } from '@tanstack/react-router';
import { Plus } from 'lucide-react';
import { lazy, Suspense, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { Button } from '@/ui/Button';
import { DialogBoundary } from '@/ui/DialogBoundary';
import { IconButton } from '@/ui/IconButton';
import { Menu } from '@/ui/Menu';
import { SkeletonRows } from '@/ui/Skeleton';
import { KINDS } from './ResourceNav';
import type { ResourceKind } from './trail';

const AddAgentDialog = lazy(() => import('./agents/AddAgent').then((module) => ({ default: module.AddAgentDialog })));
const AddProviderDialog = lazy(() => import('./providers/AddProvider').then((module) => ({ default: module.AddProviderDialog })));
const AddMachineDialog = lazy(() => import('./machines/Enrollment').then((module) => ({ default: module.AddMachineDialog })));
const AddHostDialog = lazy(() => import('./hosts/AddHost').then((module) => ({ default: module.AddHostDialog })));
const MakeEnvironmentDialog = lazy(() => import('./environments/MakeEnvironment').then((module) => ({ default: module.MakeEnvironmentDialog })));
const NewTemplateDialog = lazy(() => import('./templates/NewTemplate').then((module) => ({ default: module.NewTemplateDialog })));
const EditMcp = lazy(() => import('./mcp/Edit').then((module) => ({ default: module.EditMcp })));
const EditMount = lazy(() => import('./mounts/Edit').then((module) => ({ default: module.EditMount })));
const AddSkill = lazy(() => import('./skills/SkillDialog').then((module) => ({ default: module.AddSkill })));

export type CreationKind = ResourceKind;

function ResourceCreationDialog({ kind, onClose, onCreated }: { kind: CreationKind; onClose: () => void; onCreated?: ((id: string) => void) | undefined }) {
  const { t } = useTranslation();
  return <DialogBoundary resetKey={kind} title={t(`resourceCreation.${kind}`)} onClose={onClose}><Suspense fallback={<SkeletonRows rows={2} />}>{kind === 'providers' ? <AddProviderDialog onClose={onClose} onCreated={onCreated === undefined ? undefined : (provider) => onCreated(provider.id)} />
    : kind === 'agents' ? <AddAgentDialog onClose={onClose} onCreated={onCreated === undefined ? undefined : (agent) => onCreated(agent.id)} />
      : kind === 'machines' ? <AddMachineDialog onClose={onClose} />
        : kind === 'hosts' ? <AddHostDialog onClose={onClose} />
          : kind === 'templates' ? <NewTemplateDialog onClose={onClose} />
            : kind === 'mcp' ? <EditMcp onClose={onClose} onCreated={onCreated === undefined ? undefined : (server) => onCreated(server.id)} />
              : kind === 'mounts' ? <EditMount onClose={onClose} onCreated={onCreated === undefined ? undefined : (mount) => onCreated(mount.id)} />
                : kind === 'skills' ? <AddSkill onClose={onClose} onCreated={onCreated === undefined ? undefined : (skill) => onCreated(skill.id)} />
                  : <MakeEnvironmentDialog onClose={onClose} />}</Suspense></DialogBoundary>;
}

/** The Resources header's + creates a resource; adding a workbench view remains in the view chooser. */
export function ResourceAddMenu() {
  const { t } = useTranslation();
  const [kind, setKind] = useState<CreationKind | null>(null);
  return <>
    <Menu label={t('resources.controls.add')} align="end" trigger={<IconButton size="sm" label={t('resources.controls.add')} icon={<Plus aria-hidden />} />}
      entries={KINDS.map(({ name, icon: Icon }) => ({ id: name, label: t(`screens.${name}.title`), icon: <Icon aria-hidden />, onSelect: () => setKind(name) }))} />
    {kind !== null && <ResourceCreationDialog kind={kind} onClose={() => setKind(null)} />}
  </>;
}

export function ResourceCreateButton({ kind, primary = true, compact = false, onCreated }: { kind: CreationKind; primary?: boolean; compact?: boolean; onCreated?: (id: string) => void }) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const close = () => setOpen(false);
  return <>
    <Button size="sm" aria-label={t(`resourceCreation.${kind}`)} variant={primary ? 'primary' : 'secondary'} icon={<Plus aria-hidden />} onClick={() => setOpen(true)}>{compact ? t('common.add') : t(`resourceCreation.${kind}`)}</Button>
    {open && <ResourceCreationDialog kind={kind} onClose={close} onCreated={onCreated} />}
  </>;
}

/** Adding keeps the unfinished starter open; setup and rereading choices are explicit next acts. */
export function ResourceCreationActions({ onReadChoices }: { onReadChoices?: (kind: 'providers' | 'agents') => void }) {
  const { t } = useTranslation();
  const [provider, setProvider] = useState<string | null>(null);
  const [added, setAdded] = useState<'providers' | 'agents' | null>(null);
  return <div className="flex min-w-0 flex-wrap items-center gap-2" data-resource-creation>
    <ResourceCreateButton kind="providers" onCreated={(id) => { setProvider(id); setAdded('providers'); }} />
    <ResourceCreateButton kind="agents" primary={false} onCreated={() => setAdded('agents')} />
    {provider !== null && <Button size="sm" asChild><Link to="/resources/providers/$id" params={{ id: provider }} search={{ tab: 'key' }} target="_blank" rel="noopener noreferrer">{t('resourceCreation.finishProvider')}</Link></Button>}
    {added !== null && onReadChoices !== undefined && <Button size="sm" onClick={() => onReadChoices(added)}>{t('resourceCreation.readChoices')}</Button>}
  </div>;
}
