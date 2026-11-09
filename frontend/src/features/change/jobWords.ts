/** The confirmed operation names its actual destination; resource identities come only from the admitted plan and its existing place read. */
import type { TFunction } from 'i18next';
import type { ForkPlan, HostRef, MachineRef, Place } from '@/generated/api-types';
import { planProviderName, targetModelWords, targetName, type PlanContext } from './plan';

/** Frozen history may lack location facts. Never infer an old location from today's resource. */
export function environmentLocation(host: HostRef | null | undefined, machine: MachineRef | null | undefined): string | null {
  if (host !== null && host !== undefined && machine !== null && machine !== undefined && host.name !== machine.name) return `${host.name} · ${machine.name}`;
  return machine?.name ?? host?.name ?? null;
}

export function changeJobWords(plan: ForkPlan, fork: boolean, environmentOnly: boolean, t: TFunction, context: PlanContext, places: readonly Place[]): { heading: string; done: string } {
  if (fork) return { heading: t('change.progress.forking', { name: plan.source.name ?? '' }), done: t('change.result.forked') };
  if (environmentOnly) {
    const target = plan.to.environment;
    const place = 'new' in target ? target.new.place : places.find((candidate) => 'environment' in candidate && candidate.environment.id === target.id);
    const location = place === undefined ? null : 'environment' in place ? environmentLocation(place.host, place.machine) : place.host.name;
    const environment = targetName(target);
    return {
      heading: location === null ? t('change.progress.switchingEnvironment', { environment }) : t('change.progress.switchingEnvironmentOn', { environment, machine: location }),
      done: t('change.result.environmentSwitched', { environment }),
    };
  }
  return {
    heading: t('change.progress.switchingTo', { target: planProviderName(plan, 'to', context, t) }),
    done: t('change.result.switched', { provider: planProviderName(plan, 'to', context, t), model: targetModelWords(plan, context, t) }),
  };
}
