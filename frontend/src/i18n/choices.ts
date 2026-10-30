/** Product choices have a short familiar name and a separate explanation in the reader's catalogue. */
import type { TFunction } from 'i18next';
import type english from './en.json';
import { currentLocale, i18n, type TranslationKey } from './index';

type Definitions = typeof english.choiceCatalog;
export type ChoiceGroup = keyof Definitions;
export type ChoiceId = { [Group in ChoiceGroup]: `${Group}.${keyof Definitions[Group] & string}` }[ChoiceGroup];
export interface ChoiceCopy { name: string; description: string }
export type ChoiceValues = Record<string, string | number>;

/** Schema-only editors receive the setting's context once; nested choices keep the same context. */
export const SETTING_CHOICE_GROUPS: Readonly<Record<string, readonly ChoiceGroup[] | undefined>> = {
  'notify.operations': ['operationNotices'],
  'agent.effort': ['effort'], 'agent.memory': ['memory'], 'approval.mode': ['approvalMode'], 'approval.allowed_modes': ['approvalMode'],
  'sandbox.mode': ['sandboxLevel'], 'collab.attribution': ['attribution'], 'collab.default_delivery': ['delivery'],
  'env.thread_default': ['environmentDefault'], 'env.lifetime': ['environmentLifetime'], 'env.update': ['updateMode'], 'runtime.update': ['updateMode'], 'env.mounts': ['mountMode'],
  'env.data_sources': ['dataSource'], 'runtime.data_sources': ['dataSource'], 'runtime.home': ['dataSource'],
  'orch_tools.enabled': ['sharedTool'], 'sharing.agent_reach': ['agentReach'], 'sharing.agent_documents': ['agentDocumentAccess'],
  'workspace.index': ['indexMode'], 'broadcast.delivery': ['delivery'], 'broadcast.fallback': ['delivery'], 'broadcast.agent_options': ['delivery'],
  'browser.preset': ['browserMode'], 'hosts.when_full': ['capacityMode'], 'resources.created_by_members': ['memberCreates'],
  'broadcast.agent_scope': ['broadcastReach'], 'nodes.new_color': ['newColor'], 'switch.active_turn': ['activeTurn'],
  'binding.prefer': ['arrangement'], 'binding.allowed': ['arrangement'], 'auto.title': ['automation'], 'auto.description': ['automation'],
  'auto.tags': ['automation'], 'auto.topic_rollup': ['automation'], 'auto.triggers': ['automationTrigger'],
  'capability.policy': ['capabilityDecision', 'approverScope'], 'chat.direct': ['directMessages'], 'chat.group_mentions': ['groupMentions'],
  'orgs.created_by': ['orgCreation'], 'navigation.order': ['sortOrder'], 'registration.mode': ['registration'],
  'invitations.forms': ['invitationForm'], 'registration.email_verification': ['verification'], 'auth.email_code': ['emailCode'],
  'auth.second_step': ['secondStep'], 'system.banner': ['severity'], 'thread.inputs_without_use': ['modelUseWait'],
  'thread.switch_history': ['switchHistory'], 'thread.plan_answered_by': ['planAnswers'], 'agents.skills': ['skillChoice'],
  'agents.harness_versions': ['harnessVersion'], 'calls.return': ['callsReturn'], 'documents.links.shown': ['linksTab'],
  'search.semantic.tuning': ['searchScan'], 'environments.runtimes': ['runtimePolicy'],
};

/** The old sentence stays a description; only the short name labels the choice. */
export function choiceCopy(t: TFunction, id: ChoiceId, values: ChoiceValues = {}): ChoiceCopy {
  return { name: t(`choiceCatalog.${id}.name` as TranslationKey, values), description: t(`choiceCatalog.${id}.description` as TranslationKey, values) };
}

/** Selects, radio cards and checkable menus consume the same copy; label keeps the existing option contract. */
export function choiceOption(t: TFunction, id: ChoiceId, values: ChoiceValues = {}): ChoiceCopy & { label: string } {
  const copy = choiceCopy(t, id, values);
  return { ...copy, label: copy.name };
}

/** A plugin may offer a new term; only known built-in values use this catalogue. */
export function offeredChoice(t: TFunction, group: ChoiceGroup, value: string, values: ChoiceValues = {}): (ChoiceCopy & { label: string }) | null {
  const id = `${group}.${value}`;
  const name: unknown = i18n.getResource(currentLocale(), 'translation', `choiceCatalog.${id}.name`);
  const description: unknown = i18n.getResource(currentLocale(), 'translation', `choiceCatalog.${id}.description`);
  return typeof name === 'string' && typeof description === 'string' ? choiceOption(t, id as ChoiceId, values) : null;
}

export function settingChoice(t: TFunction, key: string | null, value: string): (ChoiceCopy & { label: string }) | null {
  for (const group of key === null ? [] : SETTING_CHOICE_GROUPS[key] ?? []) {
    const copy = offeredChoice(t, group, value);
    if (copy !== null) return copy;
  }
  return null;
}
