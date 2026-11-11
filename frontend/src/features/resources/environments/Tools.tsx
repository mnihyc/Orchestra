/** The confirmed machine program supplying this environment's tools, beside its existing MCP servers. */
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { useId } from 'react';
import { useTranslation } from 'react-i18next';
import { ApiError } from '@/api/client';
import { environmentKey, updateEnvironment } from '@/api/environments';
import { McpServers } from '@/features/resources/environments/McpServers';
import { PathValue } from '@/features/resources/TechnicalValue';
import type { Environment, EnvironmentMachineProgram, EnvironmentUpdateBody } from '@/generated/api-types';
import { holds } from '@/lib/permissions';
import { ErrorAlert } from '@/ui/ErrorState';
import { Switch } from '@/ui/Switch';

const VERSION_OVERRIDES = 'exec.version_overrides';
function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

/** Only this decision changes; all other own configuration and recorded decisions stay intact. */
export function programVersionConfig(config: unknown, program: EnvironmentMachineProgram, allow: boolean): Record<string, unknown> | null {
  if (!isRecord(config)) return null;
  const previous = config[VERSION_OVERRIDES];
  if (previous !== undefined && !isRecord(previous)) return null;
  const decisions = allow ? { ...previous, [program.harness_kind]: { path: program.path, version: program.version } }
    : Object.fromEntries(Object.entries(previous ?? {}).filter(([kind]) => kind !== program.harness_kind));
  return { ...config, [VERSION_OVERRIDES]: decisions };
}

interface Asked { body: EnvironmentUpdateBody; kind: string; path: string; version: string; allow: boolean }
function MachineProgramTools({ environment, program }: { environment: Environment; program: EnvironmentMachineProgram }) {
  const { t } = useTranslation();
  const id = useId();
  const cache = useQueryClient();
  const change = useMutation({
    mutationFn: (asked: Asked) => updateEnvironment(environment.id, asked.body),
    onSuccess: (updated) => cache.setQueryData(environmentKey(updated.id), updated),
    onError: (error) => {
      if (error instanceof ApiError && error.is('conflict.stale_revision')) void cache.invalidateQueries({ queryKey: environmentKey(environment.id) });
    },
  });
  const manages = holds(environment.access, 'environment.manage');
  const asked = change.isPending && change.variables.kind === program.harness_kind && change.variables.path === program.path && change.variables.version === program.version ? change.variables : undefined;
  const checked = asked?.allow ?? program.approval !== null;
  const approval = asked?.allow === false ? null : program.approval;
  const name = approval === null ? null : approval.by.display_name || t('environmentTools.approverUnavailable');
  const availableConfig = programVersionConfig(environment.config, program, true) !== null;
  return <section aria-labelledby={id} className="flex min-w-0 flex-col gap-2">
    <h3 id={id} className="font-medium">{t(program.harness_kind === 'codex' ? 'environmentTools.codex' : 'environmentTools.unknownProgram')}</h3>
    <div className="flex min-w-0 flex-wrap items-baseline gap-x-3 gap-y-1 text-sm">
      <span className="text-text-muted">{program.version === '' || program.version === 'unknown' ? t('machinePrograms.nativeUnknownVersion') : program.version}</span>
      <PathValue path={program.path} />
    </div>
    <Switch name={name === null ? t('environmentTools.anyVersion') : t('environmentTools.anyVersionBy', { name })}
      checked={checked} disabled={!manages || !availableConfig} aria-busy={change.isPending}
      onCheckedChange={(allow) => {
        const config = programVersionConfig(environment.config, program, allow);
        if (manages && !change.isPending && config !== null) change.mutate({ body: { expected_revision: environment.revision, config }, kind: program.harness_kind, path: program.path, version: program.version, allow });
      }} />
    {!availableConfig && <p className="text-sm text-text-muted">{t('environmentTools.configUnavailable')}</p>}
    {change.error !== null && <ErrorAlert error={change.error} />}
  </section>;
}

export function EnvironmentTools({ environment }: { environment: Environment }) {
  return <div className="flex min-w-0 flex-col gap-6">
    {(environment.exec.machine_programs ?? []).map((program) => <MachineProgramTools key={`${program.harness_kind}:${program.path}:${program.version}`} environment={environment} program={program} />)}
    <McpServers environment={environment} />
  </div>;
}
