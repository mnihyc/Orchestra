/** A skill's page: its latest version and where it came from, Update from there, Upload a version; its versions with who made each and when, and their files; its draft; and who may see, use and manage it. */
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useNavigate } from '@tanstack/react-router';
import { GraduationCap, RefreshCw, Trash2, Upload } from 'lucide-react';
import { useState } from 'react';
import { useTranslation, Trans } from 'react-i18next';
import { deleteSkill, reimportSkill, skillKey, skillQuery, skillVersionFileQuery, skillVersionQuery, skillVersionsKey, skillVersionsQuery, updateSkill } from '@/api/skills';
import { entriesOf, shareAct } from '@/features/resources/acts';
import { useResourceCleanup } from '@/features/storage/acts';
import { ResourceTabs as Tabs, ResourceHeadingActs as HeadingActs } from '@/features/resources/DetailSections';
import { Facts } from '@/features/resources/Facts';
import { ResourceAccess } from '@/features/resources/ResourceAccess';
import { useResourceHeading } from '@/features/resources/trail';
import { languageOf } from '@/features/files/languages';
import type { Skill, SkillOrigin, SkillVersionInfo } from '@/generated/api-types';
import { holds } from '@/lib/permissions';
import { Page, PageHeader, PlaceMark, Section } from '@/shell/Page';
import { adminSpec } from '@/ui/AdminAct';
import { Badge } from '@/ui/Badge';
import { Button } from '@/ui/Button';
import { ByteSize } from '@/ui/ByteSize';
import { CodeView } from '@/ui/CodeView';
import { ConfirmDialog } from '@/ui/ConfirmDialog';
import { cx } from '@/ui/cx';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { RelativeTime } from '@/ui/RelativeTime';
import { SkeletonRows } from '@/ui/Skeleton';
import { Switch } from '@/ui/Switch';
import { UserChip } from '@/ui/UserChip';
import { SkillDraftPanel } from './Draft';
import { Hint } from '@/features/help/Hint';
import { LibraryCounts, libraryQuery } from '@/features/resources/Library';
import { SkillRefusal } from './Refusal';
import { AddVersion } from './SkillDialog';

export type SkillTab = 'overview' | 'versions' | 'draft' | 'access';
export const SKILL_TABS: readonly SkillTab[] = ['overview', 'versions', 'draft', 'access'];

/** Where a version came from, in a few words. */
export function OriginWords({ origin }: { origin: SkillOrigin }) {
  const { t } = useTranslation();
  switch (origin.kind) {
    case 'upload': return <span className="break-all">{t('resourceDetails.skill.origin.upload', { name: origin.name })}</span>;
    case 'environment': return <span className="break-all">{t('resourceDetails.skill.origin.environment', { path: origin.path === '' ? '/' : origin.path })}</span>;
    case 'git': return <span className="break-all"><Trans i18nKey="resourceDetails.skill.origin.gitReference" values={{ repository: origin.repository, reference: origin.reference }} components={{ repository: <code />, reference: <code /> }} /></span>;
    case 'url': return <span className="break-all">{t('resourceDetails.skill.origin.url', { url: origin.url })}</span>;
    case 'draft': return <span>{t('resourceDetails.skill.origin.draft')}</span>;
  }
}

export function SkillScreen({ id, tab, onTab }: { id: string; tab?: SkillTab; onTab?: (tab: SkillTab) => void }) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const cleanup = useResourceCleanup();
  const cache = useQueryClient();
  const query = useQuery(skillQuery(id));
  const back = useResourceHeading('skills', GraduationCap, query.data?.name ?? t('common.loading'), query.data?.id);
  const [localTab, setLocalTab] = useState<SkillTab>('overview');
  const [adding, setAdding] = useState(false);
  const [removing, setRemoving] = useState(false);
  const kept = (skill: Skill) => cache.setQueryData(skillKey(id), skill);
  const reimport = useMutation({ mutationFn: () => reimportSkill(id), onSuccess: () => { void cache.invalidateQueries({ queryKey: skillKey(id) }); void cache.invalidateQueries({ queryKey: skillVersionsKey(id) }); } });
  const toggle = useMutation({ mutationFn: (enabled: boolean) => updateSkill(id, { enabled, expected_revision: query.data?.revision ?? 0 }), onSuccess: kept });
  const remove = useMutation({ mutationFn: () => deleteSkill(id), onSuccess: () => { void navigate({ to: '/resources/skills' }); } });
  if (query.data === undefined) return query.error === null ? <SkeletonRows rows={6} /> : <ErrorState error={query.error} onRetry={() => void query.refetch()} />;
  const skill = query.data;
  const target = { kind: 'skill' as const, id, name: skill.name };
  const manage = holds(skill.access, 'skill.manage');
  const admin = adminSpec(skill, 'skill.manage');
  const shown = tab ?? localTab;
  const latest = skill.latest;
  const menu = entriesOf([shareAct(t, target, skill.access)], [manage ? { id: 'delete', label: t('resourceDetails.remove'), icon: <Trash2 aria-hidden />, danger: true, run: () => setRemoving(true) } : null], [cleanup.act({ kind: 'skill', id }, manage)]);
  const add = manage ? { id: 'add-version', label: t('resourceDetails.skill.upload'), icon: <Upload aria-hidden />, run: () => setAdding(true) } : null;
  const facts = [
    { name: t('resources.home.label'), value: skill.home.name ?? t('access.place.hidden') },
    { name: t('resourceDetails.enabled'), value: manage
      ? <Switch label={t(skill.enabled ? 'resourceDetails.on' : 'resourceDetails.off')} checked={skill.enabled} disabled={toggle.isPending} onCheckedChange={(enabled) => toggle.mutate(enabled)} />
      : <Badge tone={skill.enabled ? 'accent' : 'neutral'}>{t(skill.enabled ? 'resourceDetails.on' : 'resourceDetails.off')}</Badge> },
    { name: t('resourceDetails.skill.latest'), value: latest === null ? t('resourceDetails.skill.noVersion') : <span className="flex flex-wrap items-center gap-x-2">
      {t('resourceDetails.skill.version', { number: latest.number })}
      {latest.created_by !== null && <UserChip user={latest.created_by} />}
      <RelativeTime at={latest.created_at} />
    </span> },
    ...(latest === null ? [] : [
      { name: t('resourceDetails.skill.origin.label'), value: <OriginWords origin={latest.origin} /> },
      { name: t('resourceDetails.skill.size'), value: <span>{t('resourceDetails.skill.fileCount', { count: latest.file_count })} · <ByteSize bytes={latest.size} /></span> },
    ]),
    { name: t('resourceDetails.address'), value: <code className="wrap-anywhere">{skill.address}</code> },
  ];
  const overview = <>
    <Section id="skill-overview" title={t('resourceDetails.overview')}>
      <Facts compact label={t('resourceDetails.overview')} rows={facts} />
    </Section>
    {manage && <div className="flex flex-wrap items-center gap-2">
      {skill.origin !== null && <Button admin={admin} icon={<RefreshCw aria-hidden />} pending={reimport.isPending} onClick={() => reimport.mutate()}>{t('resourceDetails.skill.reimport')}</Button>}
      {reimport.data !== undefined && !reimport.data.made && <span role="status" className="text-sm text-text-muted">{t('resourceDetails.skill.upToDate')}</span>}
      {reimport.error !== null && <SkillRefusal error={reimport.error} />}
      {toggle.error !== null && <ErrorAlert error={toggle.error} />}
    </div>}
    {latest !== null && <VersionFiles skillId={id} versionId={latest.id} />}
  </>;
  return <Page><PageHeader adminAccess={skill.access.admin_access} ownership={skill} opening={{ kind: 'skill', id: skill.id }} back={back} title={skill.name} lead={skill.description}
    mark={<PlaceMark><GraduationCap aria-hidden /></PlaceMark>}
    actions={<><Hint id="skills" popover /><HeadingActs admin={admin} name={skill.name} primary={add} edit={null} menu={menu} /></>} />
    <Tabs label={t('resourceDetails.layout.tabs')} value={shown} onValueChange={onTab ?? setLocalTab} panelClassName="pt-4" tabs={[
      { value: 'overview', label: t('resourceDetails.overview'), panel: overview },
      { value: 'versions', label: t('resourceDetails.skill.versions'), panel: <Versions skill={skill} /> },
      ...(manage ? [{ value: 'draft' as const, label: t('resourceDetails.skill.draft.title'), panel: <SkillDraftPanel skill={skill} /> }] : []),
      { value: 'access', label: t('agents.tabs.access'), panel: <ResourceAccess access={skill.access} target={target} /> },
    ]} />
    {adding && <AddVersion skill={skill} onClose={() => setAdding(false)} />}
    {cleanup.dialogs}
    <ConfirmDialog admin={admin} open={removing} onOpenChange={setRemoving} title={t('resourceDetails.removeTitle', { name: skill.name })} confirmLabel={t('resourceDetails.remove')} danger
      pending={remove.isPending} error={remove.error} onConfirm={() => remove.mutate()}>{t('resourceDetails.skill.removeHelp')}</ConfirmDialog>
  </Page>;
}

function Versions({ skill }: { skill: Skill }) {
  const { t } = useTranslation();
  const [cursor,setCursor]=useState<string|null>(null);
  const versions = useQuery(skillVersionsQuery(skill.id,cursor));
  const targets=(versions.data?.items??[]).map((version)=>({object:{kind:'skill' as const,id:skill.id},version_id:version.id}));
  const useFacts=useQuery({...libraryQuery(targets),enabled:versions.isSuccess&&targets.length>0});
  const factsByVersion=new Map(useFacts.data?.map((entry)=>[entry.target.version_id,entry] as const)??[]);
  const [open, setOpen] = useState<string | null>(null);
  if (versions.data === undefined) return versions.error === null ? <SkeletonRows rows={4} /> : <ErrorState error={versions.error} onRetry={() => void versions.refetch()} />;
  if (versions.data.items.length === 0) return <EmptyState compact title={t('resourceDetails.skill.noVersion')} />;
  return <>{useFacts.isPending&&<SkeletonRows rows={1}/>} {useFacts.isError&&<ErrorState compact error={useFacts.error} onRetry={()=>void useFacts.refetch()}/>}<ul aria-label={t('resourceDetails.skill.versions')} className="flex flex-col divide-y divide-border rounded-lg border border-border">
    {versions.data.items.map((version: SkillVersionInfo) => {const entry=factsByVersion.get(version.id);return <li key={version.id} className="flex min-w-0 flex-col gap-2 px-3 py-2">
      <button type="button" aria-expanded={open === version.id} onClick={() => setOpen(open === version.id ? null : version.id)}
        className="focus-inset flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1 rounded-sm text-left text-sm">
        <span className="font-medium">{t('resourceDetails.skill.version', { number: version.number })}</span>
        <span className="text-text-muted"><OriginWords origin={version.origin} /></span>
        {version.created_by !== null && <UserChip user={version.created_by} />}
        <RelativeTime at={version.created_at} />
        {skill.latest?.id === version.id && <Badge tone="accent">{t('resourceDetails.skill.latestShort')}</Badge>}
      </button>
      {entry !== undefined && <LibraryCounts facts={entry.facts} version={version.id} given={false}/>}
      {open === version.id && <VersionFiles skillId={skill.id} versionId={version.id} />}
    </li>;})}
  </ul>{(cursor!==null||versions.data.next_cursor!==null)&&<div className="flex justify-between gap-2"><Button disabled={cursor===null} onClick={()=>{setOpen(null);setCursor(null);}}>{t('documentFolders.beginning')}</Button><Button disabled={versions.data.next_cursor===null} onClick={()=>{setOpen(null);setCursor(versions.data.next_cursor);}}>{t('documentFolders.next')}</Button></div>}</>;
}

/** The files of one version, each opened read-only in the code view. */
function VersionFiles({ skillId, versionId }: { skillId: string; versionId: string }) {
  const { t } = useTranslation();
  const version = useQuery(skillVersionQuery(skillId, versionId));
  const [selected, setSelected] = useState<string | null>(null);
  if (version.data === undefined) return version.error === null ? <SkeletonRows rows={3} /> : <ErrorState error={version.error} onRetry={() => void version.refetch()} />;
  const path = selected ?? version.data.files.find((file) => file.path === 'SKILL.md')?.path ?? version.data.files[0]?.path ?? null;
  return <div className="grid min-w-0 gap-3 md:grid-cols-[minmax(12rem,16rem)_minmax(0,1fr)]">
    <ul aria-label={t('resourceDetails.skill.files')} className="flex flex-col rounded-lg border border-border">
      {version.data.files.map((file) => <li key={file.path}>
        <button type="button" aria-current={file.path === path ? 'true' : undefined} onClick={() => setSelected(file.path)}
          className={cx('focus-inset flex w-full min-w-0 items-baseline justify-between gap-2 px-3 py-1.5 text-left text-sm hover:bg-bg-muted', file.path === path && 'bg-bg-muted font-medium')}>
          <code className="min-w-0 break-all font-mono">{file.path}</code>
          <span className="shrink-0 text-xs text-text-muted">{file.executable && `${t('resourceDetails.skill.executableShort')} · `}<ByteSize bytes={file.size} /></span>
        </button>
      </li>)}
    </ul>
    {path !== null && <VersionFile skillId={skillId} versionId={versionId} path={path} />}
  </div>;
}

function VersionFile({ skillId, versionId, path }: { skillId: string; versionId: string; path: string }) {
  const { t } = useTranslation();
  const content = useQuery(skillVersionFileQuery(skillId, versionId, path));
  if (content.data === undefined) return content.error === null ? <SkeletonRows rows={6} /> : <ErrorState error={content.error} onRetry={() => void content.refetch()} />;
  if (content.data.text === null) return <EmptyState compact title={t('resourceDetails.skill.binary')} />;
  const language = languageOf(path, content.data.text.split('\n', 1)[0] ?? null);
  return <div className="h-96 min-w-0 overflow-hidden rounded-lg border border-border">
    <CodeView text={content.data.text} version={`${versionId}:${path}`} label={path} readOnly language={language?.load ?? null} languageKey={language?.id ?? null} className="h-full" />
  </div>;
}
