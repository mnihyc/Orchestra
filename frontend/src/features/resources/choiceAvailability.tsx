/** Readable choices keep their native refusal beside the same resource preview. */
import type { TFunction } from 'i18next';
import type { Host, Template } from '@/generated/api-types';
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
