/** Readable choices keep their native refusal beside the same resource preview. */
import type { TFunction } from 'i18next';
import type { Agent, Environment, Host, PlaceHost, Template } from '@/generated/api-types';
import { holds } from '@/lib/permissions';
import { resourceChoice } from '@/features/resource-facts/ResourceFacts';
import { factLine } from '@/ui/Choice';

export function templateUnavailable(template: Template, t: TFunction): string | null {
  if (!template.enabled) return t('resourceChoices.disabled');
  return holds(template.access, 'template.use') ? null : t('resourceChoices.templateUse');
}

/** Whether the person may use the host as themself, without the reach As admin adds. */
export function usesAsThemself(host: Host): boolean {
  const own = (permission: string) => holds(host.access, permission) && !(host.access.admin_only ?? []).includes(permission);
  return own('host.use') || own('host.manage');
}

/** Whether the person may use any of the hosts for it at all, whatever else stands in the way: a host of a place's list,
 * or one whose Use they hold (as themself, for a provider's container). */
export function mayUseAnyHost(hosts: readonly (PlaceHost | Host)[], kind: 'environment' | 'provider'): boolean {
  return hosts.some((entry) => 'automatic' in entry || (kind === 'provider' ? usesAsThemself(entry) : holds(entry.access, 'host.use')));
}

/** A provider's container runs as its owner, so for it a host counts only where its owner may use it as themself. */
export function hostUnavailable(host: Host, t: TFunction, kind: 'environment' | 'provider', driver?: string): string | null {
  if (!holds(host.access, 'host.use') || (kind === 'provider' && !usesAsThemself(host))) return t('resourceChoices.hostUse');
  if (!host.schedulable) return t('resourceChoices.unschedulable');
  return host.drivers.some((entry) => (driver === undefined || entry.name === driver) && entry.creates.includes(kind))
    ? null : t('resourceChoices.driver');
}

/** A refused choice says why first, then its own facts, as one passage under its name. */
function explained(choice: ReturnType<typeof resourceChoice>, reason: string | null, t: TFunction) {
  return { ...choice, disabled: reason !== null, description: factLine([reason, choice.description], t('resourceFacts.separator')) };
}

export function templateChoice(template: Template, t: TFunction) {
  return { value: template.id, name: template.name, ...explained(resourceChoice({ kind: 'template', template }), templateUnavailable(template, t), t) };
}

export function hostChoice(host: Host, t: TFunction, kind: 'environment' | 'provider', driver?: string) {
  return { value: host.id, name: host.name, ...explained(resourceChoice({ kind: 'host', host }), hostUnavailable(host, t, kind, driver), t) };
}

/** A setting can name an environment only where its person manages it: the new threads join it, and it is that person's to start, stop and change. */
export function environmentUnavailable(environment: Environment, t: TFunction): string | null {
  return holds(environment.access, 'environment.manage') ? null : t('settingsEditors.thing.needsManage');
}

export function environmentChoice(environment: Environment, t: TFunction) {
  return { value: environment.id, name: environment.name, ...explained(resourceChoice({ kind: 'environment', environment }), environmentUnavailable(environment, t), t) };
}

/** An agent profile is named by a setting only by whoever may use it. */
export function agentUnavailable(agent: Agent, t: TFunction): string | null {
  return holds(agent.access, 'agent.use') ? null : t('settingsEditors.thing.needsUse', { kind: t('settingsEditors.thing.kinds.agent') });
}

export function agentChoice(agent: Agent, t: TFunction) {
  return { value: agent.id, name: agent.name, ...explained(resourceChoice({ kind: 'agent', agent }), agentUnavailable(agent, t), t) };
}
