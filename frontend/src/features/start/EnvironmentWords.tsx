/** What the control for where a thread runs says beside itself when it chose for the person or when what they chose does not fit.
 *
 * Two things are said, each in a line and never in silence: where the default environment came from, or why it is not
 * the one that stands (it cannot be used, or the place gives every thread an environment of its own); and that the agent
 * the person wanted cannot run in the environment that stands, with the reason, what runs instead, and, where the reason is
 * a program of a machine that is a version apart or has not been confirmed, the act that lifts it, or whom to ask.
 */
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';
import type { TFunction } from 'i18next';

import { environmentKey, environmentQuery, updateEnvironment } from '@/api/environments';
import { programVersionConfig } from '@/features/resources/environments/Tools';
import type { BindingEnvironment, EnvironmentUpdateBody, NodeRef } from '@/generated/api-types';
import { holds } from '@/lib/permissions';
import { Button } from '@/ui/Button';
import { ErrorAlert } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';

import type { AgentHere, EnvironmentNote } from './choices';
import { arrangementWhy, closedArrangement, programKind, reasonWords } from './words';

/** The name of a place, where the reader may know it. */
function named(place: NodeRef | null): string | null {
  return place === null || place.redacted || place.name === null ? null : place.name;
}

/** Why a default environment cannot be used, in words. */
function whyNot(code: string, t: TFunction): string {
  return code === 'closed' ? t('start.where.closed') : reasonWords(code);
}

/** What stands in place of an environment that cannot be used. */
function instead(standing: string | null, fresh: boolean, t: TFunction): string {
  return fresh ? t('start.where.insteadNew') : standing === null ? t('start.where.insteadChoose') : t('start.where.insteadNamed', { name: standing });
}

export function EnvironmentNoteLine({ note, standing, fresh }: { note: EnvironmentNote; /** The name of the environment that stands, when one does. */ standing: string | null; /** A new environment stands. */ fresh: boolean }) {
  const { t } = useTranslation();
  if (note.kind === 'inherited') {
    const place = named(note.from);
    return <p className="text-xs text-text-muted" data-environment-note="inherited">{place === null ? t('start.where.defaultFromAbove') : t('start.where.defaultFrom', { place })}</p>;
  }
  if (note.kind === 'fresh') {
    return <p role="status" className="text-xs text-text-muted" data-environment-note="fresh">
      {note.name === null ? t('start.where.defaultFreshUnnamed') : t('start.where.defaultFresh', { name: note.name })}
    </p>;
  }
  const place = named(note.from);
  return <p role="status" className="text-xs text-warning-text" data-environment-note="unusable">
    {[note.name === null ? t('start.where.defaultUnusableUnnamed', { why: whyNot(note.reason, t) }) : t('start.where.defaultUnusable', { name: note.name, why: whyNot(note.reason, t) }),
      place === null ? null : t('start.where.defaultSetOn', { place }), instead(standing, fresh, t)].filter((part): part is string => part !== null).join(' ')}
  </p>;
}

/** The act that lifts a version difference where the person may change the environment, and whom to ask where they may not. */
function AllowVersion({ id, harnessKind, onChanged }: { id: string; harnessKind: string; onChanged: () => void }) {
  const { t } = useTranslation();
  const cache = useQueryClient();
  const read = useQuery(environmentQuery(id));
  const change = useMutation({
    mutationFn: (body: EnvironmentUpdateBody) => updateEnvironment(id, body),
    onSuccess: (updated) => { cache.setQueryData(environmentKey(updated.id), updated); onChanged(); },
  });
  const environment = read.data;
  if (environment === undefined) return read.isError ? <ErrorAlert error={read.error} /> : null;
  const programs = environment.exec.machine_programs ?? [];
  const program = programs.find((entry) => entry.harness_kind === harnessKind) ?? (programs.length === 1 ? programs[0] : undefined);
  const config = program === undefined ? null : programVersionConfig(environment.config, program, true);
  const tools = <a href={`/resources/environments/${id}?tab=tools`} className="text-accent-text hover:underline">{t('start.where.openTools')}</a>;
  if (program === undefined || config === null || !holds(environment.access, 'environment.manage')) {
    return <span className="flex flex-wrap items-center gap-x-2 text-sm text-text-muted">{t('start.where.askOwner', { name: environment.owner.display_name })}{tools}</span>;
  }
  return <span className="flex flex-wrap items-center gap-2">
    <Button size="sm" pending={change.isPending} onClick={() => change.mutate({ expected_revision: environment.revision, config })}>{t('start.where.allowVersions')}</Button>
    {tools}
    {change.error !== null && <ErrorAlert error={change.error} />}
  </span>;
}

export function AgentHereLine({ here, environment, onChanged }: { here: AgentHere; /** The existing environment that shuts it; none where a new one is judged. */ environment: BindingEnvironment | null; onChanged: () => void }) {
  const { t } = useTranslation();
  const closed = closedArrangement(here.wanted);
  const why = closed === undefined ? t('start.arrangement.notAvailable') : arrangementWhy(closed, t);
  const where = environment === null ? t('start.where.aNewEnvironment') : environment.environment.name;
  // Only a program of the machine has an act of its own here: a version that is apart (the environment's manager allows it), or one nobody confirmed (the machine's page).
  const machine = environment?.machine ?? null;
  const program = machine === null ? null : programKind(closed?.detail ?? null);
  const act = environment === null || machine === null || program === null ? undefined
    : program === 'version' ? <AllowVersion id={environment.environment.id} harnessKind={here.wanted.entry.agent.harness_kind} onChanged={onChanged} />
    : <a href={`/resources/machines/${machine.id}`} className="text-sm text-accent-text hover:underline">{t('start.where.openMachine')}</a>;
  return <InlineAlert tone="warning" action={act}>
    <span data-agent-here>
      {t('start.where.cannotRun', { agent: here.wanted.entry.agent.name, where })}{' '}{why}
      {here.instead !== null && <> {t('start.where.using', { agent: here.instead.entry.agent.name })}</>}
    </span>
  </InlineAlert>;
}
