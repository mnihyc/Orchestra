/** The thread's environment as far as the viewer may see it: where it runs, its state, start and stop where served, its idle control, and the switches its managers hold.
 *
 * Everybody who can see the thread sees its environment's name, its owner, where it runs, its state and one
 * line that says what they can do here. Whoever manages the environment also sees who else is in it (the other
 * threads that share its files, processes and network), starts and stops one that Orch made, keeps it awake, sets
 * how long it may stand idle, and turns on and off the two things that are the environment's to allow: writing
 * files into it from Orch, and opening shells in it.
 */
import { useMutation } from '@tanstack/react-query';
import { Box, Play, Square } from 'lucide-react';
import { Trans, useTranslation } from 'react-i18next';
import { programNameIn } from '@/features/resources/programNames';

import { api, ok } from '@/api/client';
import { startEnvironment, stopEnvironment } from '@/api/workspace';
import type { WorkspaceAccess } from '@/features/files/door';
import { useEnvironment } from '@/features/files/door';
import { PortsExposure } from '@/features/ports/Exposure';
import { ResourceName } from '@/features/resource-facts/ResourceFacts';
import { AgentReach } from '@/features/resources/environments/AgentReach';
import { McpServers } from '@/features/resources/environments/McpServers';
import { keptStopped } from '@/features/resources/environments/Network';
import { RestartEnvironmentButton } from '@/features/resources/environments/Restart';
import { TemporaryStatus } from '@/features/resources/environments/Temporary';
import type { Environment } from '@/generated/api-types';
import type { ViewProps } from '@/features/workbench/views';
import { holds } from '@/lib/permissions';
import { Button } from '@/ui/Button';
import { CopyButton } from '@/ui/CopyButton';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';
import { StatusDot, type DotTone } from '@/ui/StatusDot';
import { UserChip } from '@/ui/UserChip';
import { Switch } from '@/ui/Switch';
import { ScrollArea } from '@/ui/ScrollArea';

import { IdleControl } from './IdleControl';
import { InstructionSources } from './InstructionSources';
import { ThreadViewFrame } from './parts';
import { environmentStatusWord } from './words';
import { EnvironmentPrograms, ProgramBuild } from '@/features/resources/agents/ProgramBuild';

const TONE: Record<Environment['status'], DotTone> = {
  none: 'off',
  provisioning: 'info',
  running: 'success',
  stopped: 'idle',
  recreating: 'info',
  moving: 'info',
  failed: 'danger',
  offline: 'warning',
  lost: 'warning',
  disabled: 'off',
  removed: 'off',
};

function Section({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <section aria-label={title} className="flex flex-col gap-2 border-b border-border px-3 py-3 last:border-b-0">
      <h3 className="text-xs font-medium text-text-muted">{title}</h3>
      {children}
    </section>
  );
}

function EnvironmentPanel({ access }: { access: WorkspaceAccess }) {
  const { t, i18n } = useTranslation();
  const { environment, failed } = useEnvironment(access.door.environmentId);
  const start = useMutation({ mutationFn: () => startEnvironment(access.door.environmentId) });
  const stop = useMutation({ mutationFn: () => stopEnvironment(access.door.environmentId, false) });
  const switches = useMutation({
    mutationFn: (change: { remote_write?: boolean; terminal?: boolean }) =>
      ok(api.PATCH('/environments/{id}', { params: { path: { id: access.door.environmentId } }, body: { ...change, expected_revision: environment?.revision ?? 0 } })),
  });
  if (failed !== null && environment === undefined) return <ErrorState compact className="h-full" error={failed} />;
  if (environment === undefined) return <SkeletonRows rows={6} />;

  const manages = holds(environment.access, 'environment.manage');
  const showsEnvironmentPrograms = holds(environment.access, 'environment.view') && access.thread.binding?.arrangement !== 'separated';
  const owns = environment.owner.id === access.thread.owner.id && manages;
  const others = environment.threads.filter((one) => one.thread.id !== access.thread.id);
  const hidden = environment.threads_hidden;
  const sentence = manages
    ? owns
      ? t('inspector.environment.yours')
      : t('inspector.environment.manage')
    : <Trans i18nKey="inspector.environment.ownedBy" components={{ owner: <UserChip user={environment.owner} /> }} />;
  const away = environment.status !== 'running' || environment.machine?.online === false;

  return (
    <ScrollArea className="h-full" data-testid="environment-panel">
      <Section title={t('inspector.environment.what')}>
        <div className="flex items-center gap-2">
          <h2 className="min-w-0 flex-1 truncate text-lg font-semibold text-text"><ResourceName subject={{ kind: 'environment', environment }} hint={false} /></h2>
          <PortsExposure environment={environment} />
          <StatusDot tone={TONE[environment.status]} label={environmentStatusWord(t, environment.status)} showLabel />
          <CopyButton value={environment.address} label={t('inspector.environment.copyAddress')} />
        </div>
        <TemporaryStatus environment={environment} />
        <p className="text-sm text-text" data-testid="environment-can">
          {sentence}
        </p>
        {environment.status_detail !== null && <p className="text-sm text-text-muted">{environment.status_detail}</p>}
        {environment.job !== null && <p className="text-sm text-text-muted">{t('inspector.environment.job')}</p>}
      </Section>
      <Section title={t('inspector.environment.where')}>
        <p className="text-sm text-text">
          {environment.machine === null
            ? environment.host === null
              ? t('inspector.environment.placement.managed')
              : <ResourceName subject={{ kind: 'host', host: environment.host }} />
            : t('inspector.environment.onMachine', { name: environment.machine.name })}
        </p>
        {environment.machine !== null && (
          <StatusDot tone={environment.machine.online ? 'success' : 'warning'} label={environment.machine.online ? t('inspector.environment.machineOnline') : t('inspector.environment.machineAway')} showLabel />
        )}
        <p className="text-sm text-text-muted">
          {access.directory === '' ? t('inspector.environment.directoryRoot') : t('inspector.environment.directory', { directory: access.directory })}
        </p>
        <p className="text-sm text-text-muted">
          {others.length + hidden === 0 ? t('inspector.environment.alone') : t('inspector.environment.shared', { count: others.length + hidden })}
        </p>
        {manages && others.length > 0 && (
          <ul className="flex flex-col gap-1 text-sm" aria-label={t('inspector.environment.others')}>
            {others.map((one) => (
              <li key={one.thread.id} className="flex flex-wrap items-baseline gap-x-2">
                <span className="text-text">{one.thread.name}</span>
                <span className="flex flex-wrap gap-x-3 gap-y-1 text-xs text-text-muted"><span>{one.topic.name}</span><span>{one.directory === '' ? t('inspector.environment.directoryRoot') : one.directory}</span></span>
              </li>
            ))}
          </ul>
        )}
        {environment.reach > 0 && <p className="text-sm text-text-muted">{t('inspector.environment.reach', { count: environment.reach })}</p>}
      </Section>
      <AgentReach environment={environment} compact />
      <Section title={t('programVersions.running')}>{showsEnvironmentPrograms ? <EnvironmentPrograms id={environment.id} generation={environment.generation} compact /> : access.thread.binding?.program !== undefined && access.thread.binding.program !== null ? <ProgramBuild program={access.thread.binding.program} name={programNameIn(access.thread.binding.agent, i18n.resolvedLanguage ?? i18n.language) ?? undefined} /> : <span className="text-sm text-text-muted">{t('programVersions.noneObserved')}</span>}</Section>
      <McpServers environment={environment} compact />
      <Section title={t('inspector.environment.lifecycle')}>
        {environment.placement === 'managed' && manages ? (
          <div className="flex flex-wrap gap-2">
            <Button size="sm" icon={<Play aria-hidden />} disabled={environment.status !== 'stopped' || keptStopped(environment.network)} pending={start.isPending} onClick={() => start.mutate()}>
              {t('inspector.environment.start')}
            </Button>
            <Button size="sm" icon={<Square aria-hidden />} disabled={environment.status !== 'running'} pending={stop.isPending} onClick={() => stop.mutate()}>
              {t('inspector.environment.stop')}
            </Button>
            <RestartEnvironmentButton environment={environment} disabled={environment.status !== 'running' || keptStopped(environment.network) || environment.job?.state === 'running' || environment.job?.state === 'queued'} />
          </div>
        ) : (
          <p className="text-sm text-text-muted">{environment.placement === 'managed' ? t('inspector.environment.needsManage') : t('inspector.environment.notMadeHere')}</p>
        )}
        {environment.placement === 'managed' && <IdleControl environment={environment} door={access.door} mayManage={manages} />}
        {start.error !== null && <ErrorAlert error={start.error} />}
        {stop.error !== null && <ErrorAlert error={stop.error} />}
        {away && environment.status === 'running' && <p className="text-sm text-text-muted">{t('inspector.environment.machineAwayAbout')}</p>}
      </Section>
      {manages && (
        <Section title={t('inspector.environment.allows')}>
          <Switch
            label={t('inspector.environment.remoteWrite')}
            hint={t('inspector.environment.remoteWriteAbout')}
            checked={environment.remote_write}
            disabled={switches.isPending}
            onCheckedChange={(on) => switches.mutate({ remote_write: on })}
          />
          <Switch
            label={t('inspector.environment.terminal')}
            hint={t('inspector.environment.terminalAbout')}
            checked={environment.terminal}
            disabled={switches.isPending}
            onCheckedChange={(on) => switches.mutate({ terminal: on })}
          />
          {switches.error !== null && <ErrorAlert error={switches.error} />}
        </Section>
      )}
      <InstructionSources threadId={access.thread.id} />
    </ScrollArea>
  );
}

/** The Environment view of a thread. */
export function EnvironmentView(_props: ViewProps) {
  return (
    <ThreadViewFrame need="thread.view" icon={<Box aria-hidden />}>
      {(access) => <EnvironmentPanel access={access} />}
    </ThreadViewFrame>
  );
}
