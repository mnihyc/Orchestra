/** API client for what a model can be run with, as the two screens that show it need it: the caps of a provider that cover a model, and a thread's own choices of speed, access program and context.
 *
 * What a model may be run with is its entry at the provider (`resources.ts` writes it). What a thread does
 * run with is the thread's own setting: the object `agent.params`, keyed by the parameters of its agent. The
 * thread's choices are written as a whole object. A picker changes the object and revision it displayed;
 * a competing change is shown with the current value before its maker chooses again.
 */
import { queryOptions } from '@tanstack/react-query';

import { api, ok } from '@/api/client';
import { clearThreadSetting } from '@/api/workspace';
import type { CapStatus, ResolvedSetting } from '@/generated/api-types';

export const providerCapsKey = (providerId: string) => ['provider-caps', providerId] as const;

/** The caps defined on a provider, each with how far it is used. */
export const providerCapsQuery = (providerId: string) =>
  queryOptions({
    queryKey: providerCapsKey(providerId),
    queryFn: ({ signal }): Promise<CapStatus[]> => ok(api.GET('/usage/caps', { params: { query: { provider_id: providerId } }, signal })),
  });

/** The setting in which a thread keeps its own choices among the parameters of its agent. */
export const PARAMS_SETTING = 'agent.params';

const isRecord = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value);

/** The thread's own choices as they stand now, read afresh, with the revision to name when they are changed. */
async function ownChoices(threadId: string): Promise<ResolvedSetting> {
  const settings = await ok(api.GET('/nodes/{id}/settings', { params: { path: { id: threadId } } }));
  const setting = settings.find((setting) => setting.key === PARAMS_SETTING);
  if (setting === undefined) throw new Error('The thread choices could not be read.');
  return setting;
}

/**
 * Changes the thread's own choices: `change` gets the object as it stands and returns what it is to be. An
 * object left empty puts the thread back to following what is set above it and what its model starts with.
 * A picker supplies the setting it displayed. A competing write is refused and shown to its maker,
 * never silently retried on top of another person's choice. Zero guards a confirmed absent row.
 */
export async function changeThreadChoices(threadId: string, change: (choices: Record<string, unknown>) => Record<string, unknown>, seen?: ResolvedSetting): Promise<void> {
  const setting = seen ?? await ownChoices(threadId);
  const choices = isRecord(setting.own?.value) ? { ...setting.own.value } : {};
  const revision = setting.own?.revision ?? 0;
  const after = change({ ...choices });
  if (JSON.stringify(after) === JSON.stringify(choices)) return;
  if (Object.keys(after).length === 0) {
    if (revision !== 0) await clearThreadSetting(threadId, PARAMS_SETTING, revision);
    return;
  }
  await ok(api.PUT('/nodes/{id}/settings/{key}', {
    params: { path: { id: threadId, key: PARAMS_SETTING } },
    body: { value: after, expected_revision: revision },
  }));
}
