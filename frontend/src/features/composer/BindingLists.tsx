/** The providers and agents of an answer to what a thread could be bound to, and the agent's list.
 *
 * A provider is not a control of its own: the list of models stands in groups under the provider's name
 * (`ProviderModelChip`), so that choosing a model chooses its provider. The agent has a control of its own,
 * whose list holds the agents that work with the provider of the chosen model, and under "Does not work with …"
 * those that do not. A provider that runs one agent only shows that one, chosen and not to be changed.
 * Changing the model to one of another provider keeps the agent where it still works there and otherwise takes
 * the first that does, and a line says so.
 */
import { Link } from '@tanstack/react-router';
import type { TFunction } from 'i18next';
import { useTranslation } from 'react-i18next';

import { ChoiceChip, type PickGroup, type PickRow } from '@/features/composer/PickList';
import { arrangementWhy, reasonWords } from '@/features/start/words';
import type { Arrangement, BindingOptions, MissingAccess, ProviderStatus, UserRef } from '@/generated/api-types';

export interface ProviderChoice {
  id: string;
  name: string;
  /** Where it comes from, as the short names of its home and its owner. */
  address: string;
  status: ProviderStatus;
  usable: boolean;
  reason: string | null;
  /** Whoever the request for its use is put to. */
  owner: UserRef | null;
}

export interface AgentChoice {
  id: string;
  name: string;
  /** It works with the provider that is chosen. */
  fits: boolean;
  usable: boolean;
  reason: string | null;
  /** The refused native arrangement, including its specific executor or version detail. */
  refusal?: Arrangement;
}

export type ProviderAccess = 'use' | 'ask' | 'off';

/** Which group a provider is in: one that can be used, one that is asked for, or one that is unavailable. */
export function providerAccess(choice: ProviderChoice): ProviderAccess {
  const ready = choice.status === 'ready' || choice.status === 'exhausted';
  if (choice.usable && ready) return 'use';
  if (ready && !choice.usable && (choice.reason === null || choice.reason === 'no_use') && choice.owner !== null) return 'ask';
  return 'off';
}

/** Why a provider is unavailable, in a few words. */
export function providerWhy(choice: ProviderChoice, t: TFunction): string {
  if (choice.status !== 'ready' && choice.status !== 'exhausted') return t(`composer.pick.provider.why.${choice.status}`);
  return choice.reason === 'provider_not_allowed' ? t('composer.pick.provider.why.notAllowed') : t('composer.pick.provider.why.noUse');
}

/** Why an agent cannot be chosen, in a few words. */
export function agentWhy(choice: AgentChoice, t: TFunction): string {
  if (choice.refusal !== undefined) return arrangementWhy(choice.refusal, t);
  switch (choice.reason) {
    case 'harness_unreported': return t('composer.pick.agent.why.unreported');
    case 'harness_error': return t('composer.pick.agent.why.error');
    case 'disabled': return t('composer.pick.agent.why.disabled');
    case 'no_use': return t('composer.pick.agent.why.noUse');
    default: return choice.reason === null ? t('composer.pick.agent.why.unavailable') : reasonWords(choice.reason);
  }
}

/** The request for the use of a provider, put to its owner. */
export function missingUse(choice: ProviderChoice): MissingAccess {
  return { object: { id: choice.id, kind: 'provider', name: choice.name, owner: choice.owner }, permissions: ['provider.use'], options: null,
    ask: choice.owner === null ? [] : [choice.owner], ask_more: 0, override: false, closed: null };
}

/** The agent rows: those that work with the provider, then those that do not, under a header that says so. */
export function agentGroups(choices: readonly AgentChoice[], current: string, provider: string, t: TFunction): PickGroup[] {
  const fit = choices.filter((choice) => choice.fits);
  return [
    { id: 'fits', label: null, rows: fit.map((choice): PickRow => ({ id: choice.id, name: choice.name, checked: choice.id === current, disabled: !choice.usable, detail: choice.usable ? undefined : agentWhy(choice, t) })) },
    { id: 'does-not-fit', label: t('composer.pick.agent.off', { provider }), rows: choices.filter((choice) => !choice.fits).map((choice): PickRow => ({ id: choice.id, name: choice.name, disabled: true, detail: choice.reason === null ? undefined : agentWhy(choice, t) })) },
  ];
}

/** What a change of provider leaves the agent as: the same where it still works with the new one, else the first that does. */
export function agentAfterProvider(agents: readonly AgentChoice[], current: string): { agent: AgentChoice; changed: boolean } | null {
  const works = agents.filter((agent) => agent.fits && agent.usable);
  const kept = works.find((agent) => agent.id === current);
  if (kept !== undefined) return { agent: kept, changed: false };
  const first = works[0];
  return first === undefined ? null : { agent: first, changed: true };
}

/** The line that says the agent was changed with the provider. */
export function agentChangedWords(next: AgentChoice, was: string, provider: string, t: TFunction): string {
  return t('composer.pick.agent.changed', { agent: next.name, was, provider });
}

/** The providers of an answer to what a thread could be bound to. */
export function providerChoicesOf(options: Pick<BindingOptions, 'providers'>): ProviderChoice[] {
  return options.providers.map((entry) => ({ id: entry.provider.id, name: entry.provider.name, address: entry.provider.address, status: entry.provider.status,
    usable: entry.usable, reason: entry.reason, owner: entry.owner }));
}

/** The agents of an answer, for one provider: those it offers, and every other the answer names. */
export function agentChoicesOf(options: Pick<BindingOptions, 'providers'>, providerId: string): AgentChoice[] {
  const chosen = options.providers.find((entry) => entry.provider.id === providerId);
  const fit = (chosen?.agents ?? []).map((entry): AgentChoice => {
    const refusal = entry.arrangements.find((choice) => (!choice.available || !choice.permitted) && (entry.reason === null || entry.reason === choice.reason));
    return { id: entry.agent.id, name: entry.agent.name, fits: true, usable: entry.usable, reason: entry.reason, ...(refusal === undefined ? {} : { refusal }) };
  });
  const others = new Map<string, AgentChoice>();
  for (const entry of options.providers) {
    for (const agent of entry.agents) {
      if (!fit.some((one) => one.id === agent.agent.id) && !others.has(agent.agent.id)) others.set(agent.agent.id, { id: agent.agent.id, name: agent.agent.name, fits: false, usable: false, reason: null });
    }
  }
  return [...fit, ...others.values()];
}

interface ChipProps {
  /** The name on the button: of what is chosen now. */
  name: string;
  disabled?: boolean;
  reason?: string | undefined;
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
  /** Said where the list has not been read yet. */
  pending?: boolean;
}

const link = 'px-2 text-sm text-link hover:underline';

/** The agent, as a control that shows its name alone and opens the list. */
export function AgentChip({ choices, value, name, provider, onChoose, pending = false, loaded = true, ...chip }: ChipProps & {
  choices: readonly AgentChoice[];
  value: string;
  /** The name of the provider the agents have to work with. */
  provider: string;
  /** The list has been read: before that it holds the agent that is chosen alone, which says nothing of what else works. */
  loaded?: boolean;
  onChoose: (choice: AgentChoice) => void;
}) {
  const { t } = useTranslation();
  const fit = choices.filter((choice) => choice.fits);
  // A provider that runs one agent only has nothing to choose among.
  const only = loaded && choices.length > 0 && fit.length === 1 && fit[0]?.id === value && fit[0].usable;
  return <ChoiceChip what={t('composer.row.agentName')} value={name} title={name} control="agent" {...chip} disabled={chip.disabled === true || only}
    reason={only ? t('composer.pick.agent.only') : chip.reason}
    groups={agentGroups(choices, value, provider, t)} searchLabel={t('composer.pick.find')}
    below={value === '' ? undefined : <Link to="/resources/agents/$id" params={{ id: value }} className={link}>{t('composer.pick.agent.open')}</Link>}
    empty={<p role="status" className="px-2 text-sm text-text-muted">{pending ? t('common.loading') : t('start.agent.none')}</p>}
    onChoose={(row, close) => {
      const choice = choices.find((one) => one.id === row.id);
      if (choice === undefined || !choice.fits || !choice.usable) return;
      if (choice.id !== value) onChoose(choice);
      close();
    }} />;
}
