/** Identity, facts, roles, registered resources and certificate.
 *
 * A machine's page is its console in Orch. It says what the machine is and reports (its system, its
 * processors and memory, the connector it runs, how good its link was when last measured, the
 * harnesses found on it and when its certificate ends), what is registered from it, and the limits it
 * keeps to itself. Its owner renames it, asks it to report again, and revokes it. Which tab is open
 * is in the address.
 */
import { useQuery } from '@tanstack/react-query';
import { Laptop } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { machineQuery } from '@/api/resources';
import { MachineProgramsTab } from '@/features/resources/machines/Programs';
import { Installation,installationOf } from '@/features/resources/machines/Installation';
import { UpdateNotice } from '@/features/resources/ComponentUpdate';
import { adminSpec } from '@/ui/AdminAct';
import { Facts } from '@/features/resources/Facts';
import { DetailSections, ResourceDetails, ResourceTabs as Tabs, ResourceHeadingActs as HeadingActs } from '@/features/resources/DetailSections';
import { MachineRoles } from '@/features/resources/machines/Roles';
import { HarnessSessions, harnessesOfMachine } from '@/features/resources/adopt/Sessions';
import { useMachineActs } from '@/features/resources/machines/acts';
import { lostEnrollment } from '@/features/resources/machines/EnrollAgain';
import { MachineStatus } from '@/features/resources/machines/MachineList';
import { MachinePolicyTab } from '@/features/resources/machines/Policy';
import { MachineStorages } from '@/features/resources/machines/Storages';
import { RegisteredHere } from '@/features/resources/machines/Registered';
import { MachineSandboxReadiness } from '@/features/resources/SandboxReadiness';
import { MountsHere } from '@/features/resources/mounts/Here';
import { EnvironmentDirectoriesHere } from '@/features/resources/environments/DataSources';
import { formatRate, OS_NAMES, readMachineFacts, SLOW_LINK_MS } from '@/features/resources/machines/facts';
import { useResourceHeading } from '@/features/resources/trail';
import { ResourceAccess } from '@/features/resources/ResourceAccess';
import type { Machine as MachineRow } from '@/generated/api-types';
import { useAppearance } from '@/lib/appearance';
import { formatBytes, formatNumber } from '@/lib/format';
import { DAY_MS, formatDateTime, useNow } from '@/lib/time';
import { Page, PageHeader, PlaceMark, Section } from '@/shell/Page';
import { Badge } from '@/ui/Badge';
import { ErrorState } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';
import { RelativeTime } from '@/ui/RelativeTime';
import { Skeleton, SkeletonGroup, SkeletonRows } from '@/ui/Skeleton';
import { JsonTree } from '@/ui/JsonTree';

export type MachineTab = 'overview' | 'registered' | 'sessions' | 'programs' | 'storage' | 'policy' | 'certificate' | 'installation' | 'access';

/** How many days before its end a machine's certificate is called out: the connector renews it by itself, so a short notice is enough. */
const CERTIFICATE_NOTICE_DAYS = 30;

function Skeletons() {
  return (
    <Page>
      <SkeletonGroup className="flex flex-col gap-6">
        <div className="flex items-center gap-4">
          <Skeleton className="size-10 rounded-lg" />
          <div className="flex flex-1 flex-col gap-2">
            <Skeleton className="h-6 w-1/3" />
            <Skeleton className="h-4 w-1/2" />
          </div>
        </div>
        <Skeleton className="h-8 w-full" />
        <SkeletonRows rows={5} />
      </SkeletonGroup>
    </Page>
  );
}

/** What the machine is and reports, in one list of facts. */
function Overview({ machine }: { machine: MachineRow }) {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  const timeZone = useAppearance((state) => state.timeZone);
  const now = useNow();
  const facts = readMachineFacts(machine.facts);
  const installation=machine.installation ?? installationOf(machine.facts);
  const link = facts.link;
  const slow = link !== null && link.roundTripMs > SLOW_LINK_MS;
  const certificateEnds = Date.parse(machine.cert_not_after);
  const lost = lostEnrollment(machine, now);
  const certificateSoon = lost === null && machine.status === 'active' && certificateEnds - now < CERTIFICATE_NOTICE_DAYS * DAY_MS;

  return (
    <div className="flex flex-col gap-6">
      {lost !== null && (
        <InlineAlert tone="warning" title={t(`machines.lost.${lost}`, { name: machine.name })}>
          {t('machines.lost.lead')}
        </InlineAlert>
      )}
      <Section title={t('machines.overview.about')} id="machine-about">
        <Facts compact
          label={t('machines.overview.about')}
          rows={[
            { name: t('machines.overview.system'), value: facts.osVersion ?? OS_NAMES[machine.platform_os] },
            ...(facts.dedicated ? [{name:t('machines.policy.isolation'),value:t('machines.policy.dedicated')}] : []),
            ...(facts.hostname === null || facts.hostname===machine.name ? [] : [{ name: t('machines.overview.hostname'), value: facts.hostname }]),
            ...(facts.cpus === null ? [] : [{ name: t('machines.overview.cpus'), value: formatNumber(facts.cpus, locale) }]),
            ...(facts.memoryBytes === null ? [] : [{ name: t('machines.overview.memory'), value: formatBytes(facts.memoryBytes, locale) }]),
            { name: t('machines.overview.roles'), value: <MachineRoles machine={machine}/> },
            { name: t('machines.overview.lastSeen'), value: machine.last_seen_at === null ? t('machines.list.never') : <span className="flex flex-wrap items-center gap-x-2"><RelativeTime at={machine.last_seen_at} />{machine.last_ip !== null && <span className="text-text-muted">{t('machines.overview.from', { address: machine.last_ip })}</span>}</span> },
            ...(link === null
              ? []
              : [
                  {
                    name: t('machines.overview.link'),
                    value: (
                      <span className="flex flex-wrap items-center gap-x-2 gap-y-1">
                        <span>
                          {t('machines.overview.linkValue', {
                            trip: formatNumber(link.roundTripMs, locale),
                            up: formatRate(link.upBps, locale),
                            down: formatRate(link.downBps, locale),
                          })}
                        </span>
                        {slow && <Badge tone="warning">{t('machines.overview.slow')}</Badge>}
                      </span>
                    ),
                  },
                ]),
            ...(facts.clockOffsetMs === null || Math.abs(facts.clockOffsetMs) < 1_000 ? [] : [{ name: t('machines.overview.clock'), value: t('machines.overview.clockValue', { seconds: formatNumber(Math.round(facts.clockOffsetMs / 1_000), locale) }) }]),
            { name: t('machineInstallation.version'), value: machine.connector_version },
            ...(installation === null ? [] : [
              { name: t('machineInstallation.runsAs'), value: installation.user ?? t('resourceDetails.layout.notReported') },
              { name: t('machineInstallation.service'), value: installation.service === null ? t('machineInstallation.foreground.name') : <code>{installation.service}</code> },
            ]),
            {
              name: t('machines.overview.certificate'),
              value: (
                <span className="flex flex-wrap items-center gap-x-2 gap-y-1">
                  <span>{lost === null ? t('machines.overview.certificateEnds', { date: formatDateTime(machine.cert_not_after, { locale, timeZone }, now) }) : t('machines.overview.certificateEnded', { date: formatDateTime(machine.cert_not_after, { locale, timeZone }, now) })}</span>
                  {certificateSoon && <Badge tone="warning">{t('machines.overview.certificateSoon')}</Badge>}
                </span>
              ),
            },
          ]}
        />
        {slow && <p className="text-sm text-text-muted">{t('machines.overview.slowWhy', { ms: SLOW_LINK_MS })}</p>}
      </Section>

      <ResourceDetails><JsonTree value={machine.facts} label={t('machines.overview.about')} /></ResourceDetails>

    </div>
  );
}

/** The certificate that makes the machine this machine to Orch: when it ends, how to give the machine a new one, and the way to take the machine out of use for good. */
function Certificate({ machine }: { machine: MachineRow }) {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  const timeZone = useAppearance((state) => state.timeZone);
  const now = useNow();
  const lost = lostEnrollment(machine, now);
  return (
    <div className="flex flex-col gap-6">
      <Section title={t('machines.certificate.title')} id="machine-certificate" lead={t('machines.certificate.lead')}>
        <Facts compact
          label={t('machines.certificate.title')}
          rows={[
            { name: t('machines.certificate.state'), value: lost === 'revoked' ? t('machines.certificate.revoked') : lost === 'lapsed' ? t('machines.certificate.lapsed') : t('machines.certificate.good') },
            { name: t('machines.certificate.ends'), value: formatDateTime(machine.cert_not_after, { locale, timeZone }, now) },
            ...(lost === null ? [{ name: t('machines.certificate.renewal'), value: t('machines.certificate.renewsByItself') }] : []),
          ]}
        />
      </Section>
    </div>
  );
}

export interface MachineProps {
  machineId: string;
  tab: MachineTab;
  onTab: (tab: MachineTab) => void;
}

export function Machine({ machineId, tab, onTab }: MachineProps) {
  const { t } = useTranslation();
  const query = useQuery(machineQuery(machineId));
  const back = useResourceHeading('machines', Laptop, query.data?.name ?? t('common.loading'), query.data?.id);
  const acts = useMachineActs();

  if (query.isPending) return <Skeletons />;
  if (query.isError) return <ErrorState className="h-full" error={query.error} retrying={query.isRefetching} onRetry={() => void query.refetch()} />;
  const machine = query.data;
  const { edit, primary, menu } = acts.page(machine);
  const registration = tab === 'programs' || tab === 'sessions' ? tab : 'registered';
  const software = tab === 'installation' ? 'installation' : 'certificate';
  const shown = tab === 'programs' || tab === 'sessions' ? 'registered' : tab === 'installation' ? 'certificate' : tab;
  const installation = machine.installation ?? installationOf(machine.facts);
  const sectionHref = (section: string) => `/resources/machines/${encodeURIComponent(machine.id)}?tab=${section}`;

  return (
    <Page>
      <PageHeader
        adminAccess={machine.access?.admin_access} ownership={machine} opening={{ kind: 'machine', id: machine.id }}
        back={back}
        mark={
          <PlaceMark>
            <Laptop aria-hidden />
          </PlaceMark>
        }
        title={machine.name}
        facts={[<MachineStatus key="state" machine={machine} />]}
        actions={<HeadingActs omit={primary?.id === 'update-now' ? ['update-now'] : []} admin={adminSpec(machine, ['machine.manage', 'host.manage'])} name={machine.name} primary={primary?.id === 'update-now' ? null : primary} edit={edit} menu={menu} />}
      />
      <UpdateNotice update={machine.software_update} action={primary?.id === 'update-now' ? primary : null} />
      <MachineSandboxReadiness key={machine.id} values={machine.environment_sandboxes} />
      <Tabs
        label={t('machines.tabs.label')}
        value={shown}
        onValueChange={onTab}
        tabs={[
          { value: 'overview', label: t('machines.tabs.overview'), panel: <Overview machine={machine} /> },
          {
            value: 'registered',
            label: t('machines.tabs.registered'),
            count: machine.agents.length + machine.providers.length + machine.environments.length,
            panel: <DetailSections label={t('machines.tabs.registered')} value={registration} onValueChange={onTab} hrefOf={sectionHref} sections={[
              { value: 'registered', label: t('resourceDetails.layout.registered'), panel: <RegisteredHere machine={machine} /> },
              { value: 'programs', label: t('machinePrograms.title'), panel: <MachineProgramsTab machine={machine} /> },
              ...(harnessesOfMachine(machine).length === 0 ? [] : [{ value: 'sessions' as const, label: t('adopt.tab'), panel: <HarnessSessions harnesses={harnessesOfMachine(machine)} /> }]),
            ]} />,
          },
          { value: 'storage', label: t('resourceDetails.layout.storage'), panel: <DetailSections label={t('resourceDetails.layout.storage')} sections={[
            { value: 'mounts', label: t('resourceDetails.mount.here.title'), panel: <MountsHere machine={machine.id} name={machine.name} /> },
            { value: 'directories', label: t('environmentData.roots'), panel: <EnvironmentDirectoriesHere machine={machine.id} /> },
            { value: 'storages', label: t('machineStorage.title'), panel: <MachineStorages facts={machine.facts} /> },
          ]} /> },
          { value: 'policy', label: t('machines.tabs.policy'), panel: <MachinePolicyTab machine={machine} /> },
          ...(machine.access === null || machine.access === undefined ? [] : [{value:'access' as const,label:t('machines.tabs.access'),panel:<ResourceAccess access={machine.access} target={{kind:machine.host ? 'host' : 'machine',id:machine.host?.id ?? machine.id,name:machine.name}} />}]),
          { value: 'certificate', label: t('resourceDetails.layout.software'), panel: <DetailSections label={t('resourceDetails.layout.software')} value={software} onValueChange={onTab} hrefOf={sectionHref} sections={[
            { value: 'certificate', label: t('machines.tabs.certificate'), panel: <Certificate machine={machine} /> },
            { value: 'installation', label: t('machineInstallation.title'), panel: <Installation facts={machine.facts} installation={installation} version={machine.connector_version} /> },
          ]} /> },
        ]}
        panelClassName="pt-4"
      />
      {acts.dialogs}
    </Page>
  );
}
