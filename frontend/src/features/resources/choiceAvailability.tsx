/** Readable choices keep their native refusal beside the same resource preview. */
import type { TFunction } from 'i18next';
import type { Agent, Environment, Host, Template } from '@/generated/api-types';
import { holds } from '@/lib/permissions';
import { resourceChoice } from '@/features/resource-facts/ResourceFacts';

export function templateUnavailable(template: Template, t: TFunction): string | null {
  if (!template.enabled) return t('resourceChoices.disabled');
  return holds(template.access, 'template.use') ? null : t('resourceChoices.templateUse');
}

export function hostUnavailable(host: Host, t: TFunction, kind: 'environment' | 'provider', driver?: string): string | null {
  if (!holds(host.access, 'host.use')) return t('resourceChoices.hostUse');
  if (!host.schedulable) return t('resourceChoices.unschedulable');
  return host.drivers.some((entry) => (driver === undefined || entry.name === driver) && entry.creates.includes(kind))
    ? null : t('resourceChoices.driver');
}

function explained(choice: ReturnType<typeof resourceChoice>, reason: string | null) {
  return { ...choice, disabled: reason !== null, description: <>{choice.description}{reason !== null && <span className="block text-sm text-text-muted">{reason}</span>}</> };
}

export function templateChoice(template: Template, t: TFunction) {
  return { value: template.id, name: template.name, ...explained(resourceChoice({ kind: 'template', template }), templateUnavailable(template, t)) };
}

export function hostChoice(host: Host, t: TFunction, kind: 'environment' | 'provider', driver?: string) {
  return { value: host.id, name: host.name, ...explained(resourceChoice({ kind: 'host', host }), hostUnavailable(host, t, kind, driver)) };
}

/** A setting can name an environment only where its person manages it: the new threads join it, and it is that person's to start, stop and change. */
export function environmentUnavailable(environment: Environment, t: TFunction): string | null {
  return holds(environment.access, 'environment.manage') ? null : t('settingsEditors.thing.needsManage');
}

export function environmentChoice(environment: Environment, t: TFunction) {
  return { value: environment.id, name: environment.name, ...explained(resourceChoice({ kind: 'environment', environment }), environmentUnavailable(environment, t)) };
}

/** An agent profile is named by a setting only by whoever may use it. */
export function agentUnavailable(agent: Agent, t: TFunction): string | null {
  return holds(agent.access, 'agent.use') ? null : t('settingsEditors.thing.needsUse', { kind: t('settingsEditors.thing.kinds.agent') });
}

export function agentChoice(agent: Agent, t: TFunction) {
  return { value: agent.id, name: agent.name, ...explained(resourceChoice({ kind: 'agent', agent }), agentUnavailable(agent, t)) };
}
