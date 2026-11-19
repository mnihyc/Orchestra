/** What a turn ran with beyond its model and effort, for the turn's footer: the speed, the access program and the context it was started with, as it recorded them.
 *
 * A turn records the parameters it was started with. Which of them is the speed, the access program and the
 * context is told by what the thread's agent declares for them, as it is for the model control, and never
 * by their keys. A turn at the ordinary speed and with no program records neither and says nothing of them;
 * one that ran with a context says which. Where the agent's declaration cannot be read, a recorded
 * parameter that nothing tells apart is left out rather than guessed at.
 */
import { useQuery } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';

import { agentQuery } from '@/api/resources';
import { parametersQuery } from '@/api/threads';
import { kindOf, type ParamDef, readDef } from '@/features/composer/modelState';
import { formatContext } from '@/features/models/context';
import { accessProgramDisplayName } from '@/features/models/names';
import type { ModelOption, Turn } from '@/generated/api-types';
import { i18n } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { useThreadPart } from '@/realtime/stores/thread';

const isRecord = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value);

/** Whether a turn recorded any parameter at all. */
export const hasParams = (params: unknown): boolean => isRecord(params) && Object.keys(params).length > 0;

/** The declarations of an agent's parameters, as its descriptor holds them. */
function declared(descriptor: unknown): ParamDef[] {
  if (!isRecord(descriptor) || !Array.isArray(descriptor.parameters)) return [];
  return descriptor.parameters.flatMap((def: unknown) => {
    const read = readDef(def);
    return read === null ? [] : [read];
  });
}

/**
 * What a turn ran with, in order: the speed, the access program and the context it was started with. `options`
 * are the models of the thread's provider, for the name of a speed; a speed that none of them names is written
 * as it was recorded.
 */
export function ranWithParts(params: unknown, defs: readonly ParamDef[], model: string, options: readonly Pick<ModelOption, 'model_name' | 'parameters'>[], locale: string): string[] {
  if (!isRecord(params)) return [];
  const entry = options.find((option) => option.model_name === model)?.parameters;
  const parts: Partial<Record<'speed' | 'access' | 'context', string>> = {};
  for (const def of defs) {
    const kind = kindOf(def);
    const recorded = params[def.key];
    if (kind === null || recorded === undefined || recorded === null) continue;
    if (kind === 'context') {
      if (typeof recorded === 'number') parts.context ??= formatContext(recorded, locale);
    } else if (typeof recorded === 'string' && recorded !== '') {
      parts[kind] ??= kind === 'speed' ? ((entry?.service_tiers ?? []).find((tier) => tier.id === recorded)?.label ?? recorded) : accessProgramDisplayName(recorded, i18n.getFixedT(locale));
    }
  }
  return [parts.speed, parts.access, parts.context].filter((part): part is string => part !== undefined);
}

/** Recorded non-default options, as a named list. Nothing where the turn recorded none. */
export function RanWith({ threadId, turn }: { threadId: string; turn: Pick<Turn, 'model' | 'params'> }) {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  const agent = useThreadPart(threadId, (view) => view.thread?.binding?.agent.id ?? null);
  const parameters = useQuery(parametersQuery(threadId));
  // The agent's own declaration says which parameters it has whatever model the thread is on now; a reader who may not read the
  // profile is told by the parameters the thread has now.
  const profile = useQuery({ ...agentQuery(agent ?? ''), enabled: agent !== null, retry: false });
  const defs = [...declared(profile.data?.descriptor), ...(parameters.data?.parameters ?? []).flatMap((view) => {
    const read = readDef(view.def);
    return read === null ? [] : [read];
  })];
  const parts = ranWithParts(turn.params, defs, turn.model, parameters.data?.model.options ?? [], locale);
  if (parts.length === 0) return null;
  return <span>{t('timeline.footer.options',{choices:new Intl.ListFormat(locale).format(parts)})}</span>;
}
