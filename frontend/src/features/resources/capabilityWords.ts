/** Translate shipped capability labels, preserving names and descriptions an administrator supplied. */
import type { TFunction } from 'i18next';
import { i18n } from '@/i18n';

const SHIPPED = {
  ptrace: 'Debugger access', kvm: 'KVM virtualisation', gpu: 'GPU devices',
  net_admin: 'Network administration', unconfined: 'No container sandbox', privileged: 'Privileged container',
  host_network: 'Host network', docker_socket: 'Host Docker socket',
  docker_inside: 'Docker inside',
  port_place: 'Ports for a place', port_host: 'Ports on the host', port_link: 'Ports through a link',
} as const;
type Shipped = keyof typeof SHIPPED;
function shipped(key: string): key is Shipped { return Object.hasOwn(SHIPPED, key); }

export function capabilityName(key: string, label: string, t: TFunction = i18n.t): string {
  if (shipped(key) && (label === key || label === SHIPPED[key])) return t(`capabilityWords.${key}.name`);
  return label === '' ? t('capabilityWords.unreported') : label;
}

export function capabilityDescription(key: string, description: string, t: TFunction = i18n.t): string {
  if (!shipped(key)) return description;
  const path = `capabilityWords.${key}.description` as const;
  // An edited description belongs to its author; only the shipped English is translated here.
  return description === i18n.getFixedT('en')(path) ? t(path) : description;
}

/** Shipped live-application guidance is translated; authored consequences keep their own words. */
export function capabilityConsequence(line: string, t: TFunction = i18n.t): string {
  const path = 'approvals.capability.liveEffect';
  return line === i18n.getFixedT('en')(path) ? t(path) : line;
}
