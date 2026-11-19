/** The words for what the server says in codes and members: why a provider, a profile, an environment or an arrangement cannot be had, how an arrangement is described, and what a thread would run on in one line.
 *
 * The server names a reason by a code and, for an arrangement, adds a sentence of its own for the person; the sentence
 * is used as it comes, and a code it sends without one is put into words here. An unfamiliar reason
 * gets a readable fallback; protocol codes stay in diagnostic data.
 */
import type { TFunction } from 'i18next';

import { ApiError } from '@/api/client';
import type { Arrangement, Isolation } from '@/generated/api-types';
import { i18n } from '@/i18n';
import { hasErrorCode } from '@/i18n/errors';
import { offeredChoice } from '@/i18n/choices';
import { providerPickerLabels } from '@/features/resources/providers/labels';
import { isolationToolWords } from '@/features/resources/executorNames';

import { type AgentRow, type ProviderRow, type Resolved, modelName } from './choices';

/** Known reasons use the reader's language; an unfamiliar code never becomes the visible explanation. */
export function reasonWords(code: string): string {
  const key = `start.reason.${code}`;
  return i18n.exists(key) ? i18n.t(key,{defaultValue:i18n.t('start.reason.unreported')}) : i18n.t('start.reason.unreported');
}

/** Why an arrangement cannot be had: the server's own sentence, else the code put into words. */
export function arrangementWhy(entry: Arrangement, t: TFunction): string {
  if (entry.detail !== null && entry.detail !== '' && !hasErrorCode(entry.detail)) return entry.detail;
  return entry.reason === null ? t('start.arrangement.notAvailable') : reasonWords(entry.reason);
}

/** A closed profile names the same native reason as its unavailable arrangement. */
export function agentWhy(row: AgentRow, t: TFunction): string {
  if (row.unreported) return t('start.agent.unreported');
  if (row.entry.reason === 'no_host') return t('startPreflight.noHost');
  const closed = row.entry.arrangements.find((entry) => !entry.available || !entry.permitted);
  if (closed !== undefined && (row.entry.reason === null || row.entry.reason === closed.reason)) return arrangementWhy(closed, t);
  return row.entry.reason === null || row.entry.reason === 'binding_incompatible' ? t('startPreflight.changed') : reasonWords(row.entry.reason);
}

/** Why a provider cannot be chosen, with the name of whoever to ask where the caller may not use it and that is known. */
export function providerWhy(row: ProviderRow, owner: string | null, t: TFunction): string | null {
  const why = row.why;
  if (why === null) return null;
  if (why.kind === 'status') return t(`start.provider.status.${why.status}`);
  if (why.reason === 'provider_not_allowed') return t('providerPolicy.notAllowed', { place:row.entry.policy_scope === 'system' ? t('resources.settings.sourceDefault') : row.entry.policy_scope?.name ?? t('craft.private') });
  return why.reason === 'no_use' && owner !== null ? t('start.provider.notYours', { owner }) : reasonWords(why.reason);
}

/** What an arrangement comes to, in a line: where the harness runs, how the credential travels, and whose tools the agent works with. */
export function describeIsolation(isolation: Isolation, t: TFunction): string {
  const parts = [
    // Where the harness runs decides what the environment holds: with the provider it holds nothing of the credential.
    t(`start.arrangement.runs.${isolation.harness_runs_in === 'provider' ? 'provider' : isolation.credential_path}`),
    isolationToolWords(isolation, t, i18n.resolvedLanguage ?? i18n.language) ?? t('start.arrangement.tools.own'),
    isolation.orch_tools.offered ? null : t('start.arrangement.noOrchTools'),
  ];
  return parts.filter((part): part is string => part !== null).join(' ');
}

/** What a thread would run on, in the one line under the composer: the profile on the provider, the model and its effort, and where. */
export function runsLine(resolved: Resolved, t: TFunction): string {
  const where = resolved.fresh ? t('start.quick.inNew') : resolved.environment === null ? null : t('start.quick.in', { name: resolved.environment.environment.name });
  const names=providerPickerLabels(resolved.providers.map((row)=>row.entry.provider));
  const model = resolved.model === null ? null : modelName(resolved.model);
  const effort = resolved.effort === null ? null : offeredChoice(t, 'effort', resolved.effort)?.name ?? t('changeCompact.otherEffort');
  return [
    resolved.agent === null || resolved.provider === null ? null : t('start.quick.runs', { agent: resolved.agent.entry.agent.name, provider: names.get(resolved.provider.id) ?? resolved.provider.entry.provider.name }),
    model === null || effort === null ? model : t('startFlow.modelEffort', { model, effort }),
    where,
  ]
    .filter((part): part is string => part !== null)
    .join(t('startFlow.separator'));
}

/**
 * What a refusal to bind a thread says beyond the sentence of its code: the reason the server gave, in words, so that a way
 * that was asked for and cannot be had is refused with why and never answered by another. `null` for any other error.
 */
export function bindingReason(error: unknown, t: TFunction): string | null {
  // The same code refuses a model and an effort of it; the effort is named, not the model that was allowed.
  if (error instanceof ApiError && error.is('model.not_allowed') && error.problem.reason === 'effort') return t('start.refusedEffort');
  if (!(error instanceof ApiError) || !error.is('binding.incompatible')) return null;
  const reason = typeof error.problem.reason === 'string' ? error.problem.reason : null;
  if (reason === 'setup_changed') return t('startPreflight.changed');
  if (reason === 'no_host') return t('startPreflight.noHost');
  if (reason === null || !i18n.exists(`start.reason.${reason}`)) return t('errors.binding.incompatible');
  const why = reasonWords(reason);
  return t('start.refusedWhy', { what: t('errors.binding.incompatible'), why });
}
