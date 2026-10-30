/** What an environment runs on and under, as the screens tell it: the classes a host reports for a driver's runtimes, the limits that touch what an environment has or can be given, and the words of both. */
import { useTranslation } from 'react-i18next';

import type { Boundary, EnvironmentMovePlan, Host, RuntimeClass, RuntimeLimit } from '@/generated/api-types';

/** A move from a hosted unit removes installed software unless its kept image is carried, even when that image is empty. */
export function moveRemovesSoftware(plan: Pick<EnvironmentMovePlan, 'from'> & { files: Pick<EnvironmentMovePlan['files'], 'kept_image_bytes'> }): boolean {
  return plan.from !== null && (plan.files.kept_image_bytes === null || plan.files.kept_image_bytes === undefined);
}

/** The word a limit names when it touches an environment that has a mount attached. */
export const MOUNTS_SUBJECT = 'mounts';

/** The first letter of a sentence in capitals: a limit is stored as a fragment and shown as a sentence. */
export function capitalize(text: string): string {
  return text === '' ? text : text.charAt(0).toUpperCase() + text.slice(1);
}

/** Whether a limit touches what an environment has or can be given: it names nothing, or one of what it names is among `subjects`. */
export function limitTouches(limit: RuntimeLimit, subjects: readonly string[]): boolean {
  return limit.touches.length === 0 || limit.touches.some((touched) => subjects.includes(touched));
}

/** The limits of a class that touch what an environment has or can be given, in the order the class declares them. */
export function limitsTouching(limits: readonly RuntimeLimit[] | undefined, subjects: readonly string[]): RuntimeLimit[] {
  return (limits ?? []).filter((limit) => limitTouches(limit, subjects));
}

/** Whether a class is the runtime `name`: its name or one of the other names the daemon knows it by. */
export function isNamed(runtime: RuntimeClass, name: string): boolean {
  return runtime.name === name || (runtime.aliases ?? []).includes(name);
}

/** The class named `name` among `classes`, or none. */
export function classNamed(classes: readonly RuntimeClass[], name: string | null | undefined): RuntimeClass | undefined {
  return name === null || name === undefined || name === '' ? undefined : classes.find((runtime) => isNamed(runtime, name));
}

/** The classes the hosts report for a driver, each once, in the order they were first seen. */
export function classesOfDriver(hosts: readonly Pick<Host, 'drivers'>[], driver: string): RuntimeClass[] {
  const seen = new Map<string, RuntimeClass>();
  for (const host of hosts) {
    for (const entry of host.drivers) {
      if (entry.name !== driver) continue;
      for (const runtime of entry.runtimes ?? []) if (!seen.has(runtime.name)) seen.set(runtime.name, runtime);
    }
  }
  return [...seen.values()];
}

/** The runtime names a host reports for a driver that it describes by name alone (an older node, a driver with no classes). */
export function namesOfDriver(hosts: readonly Pick<Host, 'drivers'>[], driver: string): string[] {
  const names = new Set<string>();
  for (const host of hosts) for (const entry of host.drivers) if (entry.name === driver) for (const name of entry.runtime_classes) names.add(name);
  return [...names];
}

const text = (value: unknown): string | null => (typeof value === 'string' && value !== '' ? value : null);

/** The runtime a template's spec starts under. */
export function templateRuntime(spec: unknown): string | null {
  return spec !== null && typeof spec === 'object' ? text((spec as Record<string, unknown>).runtime) : null;
}

/** The runtimes a template permits besides its own; empty permits every one. */
export function allowedRuntimes(spec: unknown): string[] {
  const listed = spec !== null && typeof spec === 'object' ? (spec as Record<string, unknown>).allowed_runtimes : null;
  return Array.isArray(listed) ? listed.filter((entry): entry is string => typeof entry === 'string' && entry !== '') : [];
}

/** The capability keys a template holds from the start or allows later: what an environment made from it has or can be given. */
export function templateSubjects(spec: unknown, mounts = false): string[] {
  const members = spec !== null && typeof spec === 'object' ? (spec as Record<string, unknown>) : {};
  const keys = (member: unknown): string[] => (Array.isArray(member) ? member.filter((entry): entry is string => typeof entry === 'string') : []);
  const subjects = [...new Set([...keys(members.base_capabilities), ...keys(members.allowed_capabilities)])];
  return mounts ? [...subjects, MOUNTS_SUBJECT] : subjects;
}

/** What a place's policy `environments.runtimes` says of a runtime: why it may not be chosen, if it may not. */
export function policyRefusal(policy: unknown, runtime: string): 'forbidden' | 'required_elsewhere' | null {
  const rules = policy !== null && typeof policy === 'object' ? (policy as Record<string, unknown>) : {};
  const rule = (name: string): string | null => text(rules[name]) ?? text(rules['*']);
  if (rule(runtime) === 'forbid') return 'forbidden';
  const required = Object.entries(rules).filter(([name, value]) => name !== '*' && value === 'require');
  return required.length > 0 && rule(runtime) !== 'require' ? 'required_elsewhere' : null;
}

/** The words of the pair, the boundary and the limits, in the reader's language. */
export function useIsolationWords() {
  const { t } = useTranslation();
  const boundary = (value: Boundary): string => t(`isolation.boundary.${value}`);
  const driverTitle = (title: string): string => {
    switch (title) {
      case 'Docker': return t('isolation.titles.docker');
      case 'Docker containers': return t('isolation.titles.dockerContainers');
      case 'Registered directories': return t('isolation.titles.directories');
      case 'Programs on the host': return t('isolation.titles.hostPrograms');
      default: return title;
    }
  };
  return {
    boundary,
    /** Translate shipped descriptor titles while keeping an operator's custom name. */
    driver: driverTitle,
    /** "gVisor: its own kernel written in user space". */
    hint: (title: string, value: Boundary): string => t('isolation.hint', { runtime: title, boundary: boundary(value) }),
    /** "Docker · gVisor", or the driver alone where it has no runtimes to tell apart. */
    pair: (driver: string, runtime: string | null | undefined): string => (runtime === null || runtime === undefined || runtime === '' ? driverTitle(driver) : t('isolation.pair', { driver: driverTitle(driver), runtime })),
    /** One limit as a sentence: what to do and why, or the reason alone. */
    limit: (limit: RuntimeLimit): string => {
      const why = t(`isolation.limits.${limit.key}.why`, { defaultValue: limit.why });
      const todo = limit.todo === null || limit.todo === undefined ? null : t(`isolation.limits.${limit.key}.todo`, { defaultValue: limit.todo });
      return capitalize(todo === null ? why : t('isolation.limit', { todo, why }));
    },
    /** Why a capability cannot be given under a runtime, as a sentence. */
    unavailable: (capability: string, value: Boundary, fallback: string): string => t(`isolation.unavailable.${capability}.${value}`, { defaultValue: fallback }),
  };
}
