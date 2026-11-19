/** Administrative program pages use server summaries, grouped declarations and bounded build pages. */
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { ArrowLeft, Package, Plus } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { choiceOption } from '@/i18n/choices';

import { adminProgramBuildsQuery, adminProgramQuery, adminProgramsQuery, checkProgramBuild, readProgramChannels, saveProgramBuild } from '@/api/programAdmin';
import { BuildStatus, ChannelReadStatus, ProgramVersionsHelp } from '@/features/admin/HarnessVersions';
import { programName } from '@/features/resources/programNames';
import { useGuideEntry } from '@/features/help/guideEntries';
import { useHelpOpener } from '@/features/help/open';
import type { HarnessBuild, HarnessKindName, MachineOs, ProgramDetail, ProgramSummary } from '@/generated/api-types';
import { ProgramList, type ProgramListEntry } from '@/features/admin/ProgramList';
import { ProgramCapabilities, type ProgramCapability } from '@/features/admin/ProgramCapabilities';
import { ProgramSettings } from '@/features/admin/ProgramSettings';
import type { SectionProps } from '@/features/admin/sections';
import { Tabs } from '@/ui/Tabs';
import { useForm } from '@/lib/forms';
import { Section } from '@/shell/Page';
import { StatusTag } from '@/features/admin/StatusTag';
import { Button } from '@/ui/Button';
import { Checkbox } from '@/ui/Checkbox';
import { CopyButton } from '@/ui/CopyButton';
import { Dialog } from '@/ui/Dialog';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { Input } from '@/ui/Input';
import { HelpHint } from '@/ui/HelpHint';
import { Select } from '@/ui/Select';
import { SkeletonRows } from '@/ui/Skeleton';
import { type Column, Table } from '@/ui/Table';
import { toast } from '@/ui/Toast';

const SYSTEMS: readonly MachineOs[] = ['linux', 'macos', 'windows'];
const PROCESSORS = ['x86_64', 'aarch64'] as const;
const SHA256 = /^[0-9a-f]{64}$/;
const SHORT_SHA = 12;
const BUILD_HELP = { article: 'agent-program-builds' } as const;
/** How often the list is read again while a build is being checked: a check takes seconds once the build is fetched. */
const CHECK_FOLLOW_MS = 5_000;

function Checksum({ build, name }: { build: HarnessBuild; name: string }) {
  const { t } = useTranslation();
  return <span className="flex items-center gap-1 font-mono text-sm text-text-muted">
    <span tabIndex={0} data-overflow-text={build.sha256} className="focus-inset rounded-md">{build.sha256.slice(0, SHORT_SHA)}</span>
    <CopyButton value={build.sha256} label={t('admin.harnesses.copySha', { kind: name, version: build.version })} />
  </span>;
}

/** The form that adds a build. The kind is one the system has, or one a person writes for an adapter that is not listed yet. */
function BuildDialog({ kinds, onClose }: { kinds: readonly HarnessKindName[]; onClose: () => void }) {
  const { t, i18n } = useTranslation();
  const form = useForm(
    { kind: kinds[0]?.kind ?? '', version: '', url: '', sha256: '' },
    {
      kind: (value) => (/^[a-z][a-z0-9_-]*$/.test(value) ? null : t('admin.harnesses.kindRule')),
      version: (value) => (value.trim() === '' ? t('admin.harnesses.versionRule') : null),
      url: (value) => (value.trim().startsWith('https://') ? null : t('admin.harnesses.urlRule')),
      sha256: (value) => (SHA256.test(value.trim().toLowerCase()) ? null : t('admin.harnesses.shaRule')),
    },
  );
  const [os, setOs] = useState<MachineOs>('linux');
  const [arch, setArch] = useState<(typeof PROCESSORS)[number]>('x86_64');
  const [isDefault, setDefault] = useState(false);
  const [placed, setPlaced] = useState(false);

  const save = useMutation({
    mutationFn: (values: typeof form.values) =>
      // What a build is known to do is the server's to say: a build added by hand starts untested.
      saveProgramBuild({ kind: values.kind, version: values.version.trim(), os, arch, url: values.url.trim(), sha256: values.sha256.trim().toLowerCase(), is_default: isDefault, source: 'admin', status: 'untested' }),
    onSuccess: (_all, values) => {
      const registered = kinds.find((kind) => kind.kind === values.kind);
      toast({ tone: 'success', title: t('admin.harnesses.added', { kind: programName(registered, i18n.resolvedLanguage ?? i18n.language) ?? t('resources.harnessNameUnknown'), version: values.version.trim() }) });
      onClose();
    },
    onError: (error) => setPlaced(form.refuse(error)),
  });

  return (
    <Dialog
      open
      onOpenChange={(open) => !open && !save.isPending && onClose()}
      title={t('admin.harnesses.addTitle')}
      description={t('admin.harnesses.addLead')}
      dismissible={!save.isPending}
      footer={
        <>
          <Button disabled={save.isPending} onClick={onClose}>
            {t('common.cancel')}
          </Button>
          <Button variant="primary" type="submit" form="admin-build-form" pending={save.isPending}>
            {t('admin.harnesses.add')}
          </Button>
        </>
      }
    >
      <form id="admin-build-form" className="flex flex-col gap-4 py-2" noValidate onSubmit={form.submit((values) => save.mutate(values))}>
        <div className="grid gap-4 sm:grid-cols-2">
          <Input label={t('admin.harnesses.kind')} hint={t('admin.harnesses.kindHint')} autoComplete="off" autoCapitalize="none" mono list="admin-harness-kinds" {...form.field('kind')} />
          <Input label={t('admin.harnesses.version')} autoComplete="off" mono {...form.field('version')} />
        </div>
        <datalist id="admin-harness-kinds">
          {kinds.map((kind) => (
            <option key={kind.kind} value={kind.kind} />
          ))}
        </datalist>
        <div className="grid gap-4 sm:grid-cols-2">
          <Select label={t('admin.harnesses.os')} value={os} onValueChange={setOs} options={SYSTEMS.map((one) => ({ value: one, ...choiceOption(t, `operatingSystem.${one}`) }))} />
          <Select label={t('admin.harnesses.arch')} value={arch} onValueChange={setArch} options={PROCESSORS.map((one) => ({ value: one, ...choiceOption(t, `architecture.${one}`) }))} />
        </div>
        <Input label={t('admin.harnesses.url')} hint={t('admin.harnesses.urlHint')} autoComplete="off" autoCapitalize="none" mono {...form.field('url')} />
        <Input label={t('admin.harnesses.sha')} hint={t('admin.harnesses.shaHint')} autoComplete="off" autoCapitalize="none" mono {...form.field('sha256')} />
        <Checkbox label={t('admin.harnesses.makeDefault')} hint={t('admin.harnesses.makeDefaultHint')} checked={isDefault} onCheckedChange={setDefault} />
        {save.error !== null && !placed && <ErrorAlert error={save.error} />}
      </form>
    </Dialog>
  );
}

/** The existing build rows and actions, ready to be shown for one program. */
function BuildsTable({ builds, title, catalogue, onChange }: { builds: readonly HarnessBuild[]; title: (kind: string) => string; catalogue: (kind: string) => unknown; onChange: () => Promise<unknown> }) {
  const { t } = useTranslation();
  const check = useMutation({ mutationFn: async (build: HarnessBuild) => {
    const accepted = await checkProgramBuild(build);
    await onChange();
    return accepted;
  } });

  const makeDefault = useMutation({
    mutationFn: (build: HarnessBuild) => saveProgramBuild({ ...build, is_default: true }),
    onSuccess: async (_all, build) => {
      await onChange();
      toast({ tone: 'success', title: t('admin.harnesses.defaulted', { kind: title(build.kind), version: build.version }) });
    },
  });

  const status = (build: HarnessBuild) => <span className="flex min-w-0 flex-wrap items-center gap-2">
    <BuildStatus build={build} name={title(build.kind)} catalogue={catalogue(build.kind)} />
    {build.source !== 'release' && build.status !== 'checking' && <Button size="sm" variant="ghost" pending={check.isPending && check.variables.kind === build.kind && check.variables.version === build.version} onClick={() => check.mutate(build)}>{t('admin.harnesses.check')}</Button>}
  </span>;
  const defaultChoice = (build: HarnessBuild) => build.is_default
    ? <StatusTag tone="accent">{t('admin.harnesses.isDefault')}</StatusTag>
    : <Button size="sm" pending={makeDefault.isPending && makeDefault.variables.version === build.version && makeDefault.variables.kind === build.kind} onClick={() => makeDefault.mutate(build)}>{t('admin.harnesses.makeDefaultButton')}</Button>;

  const columns: Column<HarnessBuild>[] = [
    {
      id: 'kind',
      header: t('admin.harnesses.kind'),
      cell: (build, shown) => {
        const platformHidden = !shown.has('platform');
        const sourceHidden = !shown.has('source');
        const checksumHidden = !shown.has('sha');
        const statusHidden = !shown.has('status');
        const defaultHidden = !shown.has('default');
        return <span className="flex min-w-0 flex-col gap-1">
        <span className="min-w-0 wrap-anywhere font-medium text-text">{title(build.kind)}</span>
        {platformHidden && <span className="text-sm text-text-muted">{t(`admin.harnesses.systems.${build.os}`)} · {build.arch}</span>}
        {sourceHidden && <span className="text-sm text-text-muted">{t(`admin.harnesses.sources.${build.source}`)}</span>}
        {checksumHidden && <Checksum build={build} name={title(build.kind)} />}
        {statusHidden && status(build)}
        {defaultHidden && <span>{defaultChoice(build)}</span>}
      </span>;
      },
    },
    { id: 'version', header: t('admin.harnesses.version'), cell: (build) => <span tabIndex={0} data-overflow-text={build.version} className="focus-inset inline-block max-w-(--field-narrow) truncate rounded-md font-mono" title={build.version}>{build.version}</span> },
    { id: 'platform', header: t('admin.harnesses.platform'), from: 'sm', cell: (build) => `${t(`admin.harnesses.systems.${build.os}`)} · ${build.arch}` },
    { id: 'source', header: t('admin.harnesses.source'), from: 'lg', cell: (build) => t(`admin.harnesses.sources.${build.source}`) },
    {
      id: 'status',
      header: t('admin.harnesses.statusTitle'),
      from: 'md',
      cell: status,
    },
    {
      id: 'sha',
      header: t('admin.harnesses.sha'),
      from: 'xl',
      cell: (build) => <Checksum build={build} name={title(build.kind)} />,
    },
    {
      id: 'default',
      header: t('admin.harnesses.default'),
      from: 'md',
      align: 'end',
      cell: defaultChoice,
    },
  ];

  return <>
    {check.error !== null && <ErrorAlert error={check.error} />}
    <Table label={t('admin.harnesses.buildsTitle')} columns={columns} rows={builds} rowKey={(build) => `${build.kind}/${build.version}/${build.os}/${build.arch}`} />
    {makeDefault.error !== null && <ErrorAlert error={makeDefault.error} />}
  </>;
}

/** A program's name is declared; only administrators see the kind beside it. */
function SummaryName({ summary }: { summary: ProgramSummary }) {
  const { t, i18n } = useTranslation();
  const name = programName(summary.harness, i18n.resolvedLanguage ?? i18n.language) ?? t('resources.harnessNameUnknown');
  return <span className="flex min-w-0 flex-wrap items-baseline gap-2"><span className="min-w-0 wrap-anywhere">{name}</span><span className="min-w-0 wrap-anywhere text-sm font-normal text-text-muted">{summary.harness.kind}</span></span>;
}

/** Page navigation replaces the rows; previous pages remain available in browser history. */
function Pages({ cursor, next, loading, onPage }: { cursor: string | undefined; next: string | null | undefined; loading: boolean; onPage: (cursor: string | undefined) => void }) {
  const { t } = useTranslation();
  if (cursor === undefined && !next) return null;
  return <nav aria-label={t('programList.pages')} className="flex flex-wrap items-center justify-end gap-2">
    {cursor !== undefined && <Button disabled={loading} onClick={() => onPage(undefined)}>{t('programList.firstPage')}</Button>}
    {next && <Button pending={loading} onClick={() => onPage(next)}>{t('programList.nextPage')}</Button>}
  </nav>;
}

/** One read holds one program page and never pulls its build table into the directory. */
function ProgramDirectory({ search, onSearch }: SectionProps) {
  const { t, i18n } = useTranslation();
  const client = useQueryClient();
  const programs = useQuery(adminProgramsQuery(search.q ?? '', search.programCursor));
  const [adding, setAdding] = useState(false);
  const locale = i18n.resolvedLanguage ?? i18n.language;
  const state = (program: ProgramSummary): ProgramListEntry => ({
    kind: program.harness.kind, name: programName(program.harness, locale) ?? t('resources.harnessNameUnknown'), protocol: program.protocol ?? null,
    shipped: program.platforms.flatMap((p) => p.shipped === null ? [] : [{ version: p.shipped, platform: `${p.os}/${p.arch}` }]),
    newestChecked: program.platforms.flatMap((p) => p.newest_checked === null ? [] : [{ version: p.newest_checked, platform: `${p.os}/${p.arch}` }]),
    channel: { label: t(`programList.channelStates.${program.channel}`), state: program.channel === 'ready' ? 'done' : program.channel === 'failed' ? 'failed' : 'idle' },
    check: { label: program.check === null ? t('programList.notReported') : t(`admin.harnesses.status.${program.check}`), state: program.check === 'checked' ? 'done' : program.check === 'failed' ? 'failed' : program.check === 'checking' ? 'running' : 'idle' },
  });
  return <Section id="admin-programs" title={t('programList.title')} action={<Button variant="primary" icon={<Plus aria-hidden />} onClick={() => setAdding(true)}>{t('admin.harnesses.addTitle')}</Button>}>
    <Input label={t('programList.search')} value={search.q ?? ''} onChange={(event) => onSearch({ q: event.target.value, programCursor: undefined })} />
    {programs.isPending ? <SkeletonRows rows={3} /> : programs.isError ? <ErrorState compact error={programs.error} retrying={programs.isRefetching} onRetry={() => void programs.refetch()} /> : <>
      <ProgramList programs={programs.data.items.map(state)} onOpen={(program) => onSearch({ program, programTab: undefined, buildCursor: undefined }, { replace: false })} />
      <Pages cursor={search.programCursor} next={programs.data.next_cursor} loading={programs.isFetching} onPage={(programCursor) => onSearch({ programCursor }, { replace: false })} />
    </>}
    {adding && <BuildDialog kinds={programs.data?.items.map((p) => p.harness) ?? []} onClose={() => { setAdding(false); void client.invalidateQueries({ queryKey: adminProgramsQuery(search.q ?? '', search.programCursor).queryKey, exact: true }); }} />}
  </Section>;
}

/** Today's table and actions, restricted to one program and one page of builds. */
function ProgramBuilds({ detail, search, onSearch }: SectionProps & { detail: ProgramDetail }) {
  const { t, i18n } = useTranslation();
  const client = useQueryClient();
  const kind = detail.summary.harness.kind;
  const name = programName(detail.summary.harness, i18n.resolvedLanguage ?? i18n.language) ?? t('resources.harnessNameUnknown');
  const builds = useQuery({ ...adminProgramBuildsQuery(kind, search.buildCursor), refetchInterval: (query) => query.state.data?.items.some((build) => build.status === 'checking') === true ? CHECK_FOLLOW_MS : false });
  const [adding, setAdding] = useState(false);
  const refresh = () => Promise.all([
    client.invalidateQueries({ queryKey: adminProgramQuery(kind).queryKey, exact: true }),
    client.invalidateQueries({ queryKey: adminProgramBuildsQuery(kind, search.buildCursor).queryKey, exact: true }),
  ]);
  const readChannels = useMutation({ mutationFn: () => readProgramChannels(kind), onSuccess: async () => { await refresh(); toast({ tone: 'success', title: t('admin.harnesses.channelsRead') }); } });
  const openHelp = useHelpOpener();
  const helpHeading = t('admin.harnesses.sourceHeading');
  const { ref: checksumHelpRef, hidden: checksumHelpHidden } = useGuideEntry(BUILD_HELP.article, helpHeading);
  return <Section id="admin-builds" title={<span className="inline-flex items-center gap-1">{t('admin.harnesses.buildsTitle')}<ProgramVersionsHelp /></span>} lead={t('admin.harnesses.buildsLead')} action={<span className="flex flex-wrap items-center gap-2">
    <span ref={checksumHelpRef} className="inline-flex">{!checksumHelpHidden && <HelpHint term={t('admin.harnesses.source')} what={t('admin.harnesses.sourceHelp')} guide={{ article: 'agent-program-builds', heading: t('admin.harnesses.sourceHeading') }} more={{ label: t('help.hint.more'), onOpen: () => openHelp(BUILD_HELP.article, helpHeading) }} />}</span>
    <Button pending={readChannels.isPending} onClick={() => readChannels.mutate()}>{t('admin.harnesses.readChannels')}</Button>
    <Button variant="primary" icon={<Plus aria-hidden />} onClick={() => setAdding(true)}>{t('admin.harnesses.addTitle')}</Button>
  </span>}>
    {readChannels.error !== null && <ErrorAlert error={readChannels.error} />}
    <ChannelReadStatus value={detail.channels} title={() => name} />
    {builds.isPending ? <SkeletonRows rows={3} /> : builds.isError ? <ErrorState compact error={builds.error} retrying={builds.isRefetching} onRetry={() => void builds.refetch()} /> : builds.data.items.length === 0 ? <EmptyState compact icon={<Package aria-hidden />} title={t('admin.harnesses.noBuilds')} description={t('admin.harnesses.noBuildsHint')} /> : <BuildsTable builds={builds.data.items} title={() => name} catalogue={() => detail.program?.checks ?? null} onChange={refresh} />}
    <Pages cursor={search.buildCursor} next={builds.data?.next_cursor} loading={builds.isFetching} onPage={(buildCursor) => onSearch({ buildCursor }, { replace: false })} />
    {adding && <BuildDialog kinds={[detail.summary.harness]} onClose={() => { setAdding(false); void refresh(); }} />}
  </Section>;
}

/** Detail tabs live in the address and fetch the selected program independently of the directory page. */
function ProgramPage({ search, onSearch }: SectionProps) {
  const { t, i18n } = useTranslation();
  const detail = useQuery(adminProgramQuery(search.program ?? ''));
  const tab = search.programTab ?? 'builds';
  const locale = i18n.resolvedLanguage ?? i18n.language;
  const localized = (text: Record<string, string | undefined>) => text[locale] ?? text.en ?? '';
  const body = detail.isPending ? <SkeletonRows rows={3} /> : detail.isError ? <ErrorState compact error={detail.error} retrying={detail.isRefetching} onRetry={() => void detail.refetch()} /> : <Section id="admin-program" title={<SummaryName summary={detail.data.summary} />}>
    <Tabs label={t('programList.title')} value={tab} onValueChange={(programTab) => onSearch({ programTab }, { replace: false })} panelClassName="pt-4" tabs={[
      { value: 'builds', label: t('admin.harnesses.buildsTitle'), panel: <ProgramBuilds detail={detail.data} search={search} onSearch={onSearch} /> },
      { value: 'capabilities', label: t('programList.capabilities'), panel: <ProgramCapabilities groups={detail.data.capabilities?.map((group) => ({ id: group.id, title: localized(group.title), items: group.items.map((item): ProgramCapability => item.available ? { id: item.id, title: localized(item.title), available: true } : { id: item.id, title: localized(item.title), available: false, reason: item.reason === null ? t('programCapabilities.off') : localized(item.reason) }) })) ?? null} /> },
      { value: 'settings', label: t('programList.settings'), panel: <ProgramSettings detail={detail.data} /> },
    ]} />
  </Section>;
  return <div className="flex min-w-0 flex-col gap-4"><div><Button variant="ghost" icon={<ArrowLeft aria-hidden />} onClick={() => onSearch({ program: undefined, programTab: undefined, buildCursor: undefined }, { replace: false })}>{t('programList.title')}</Button></div>{body}</div>;
}

export function Harnesses(props: SectionProps) {
  return props.search.program === undefined ? <ProgramDirectory {...props} /> : <ProgramPage key={props.search.program} {...props} />;
}
