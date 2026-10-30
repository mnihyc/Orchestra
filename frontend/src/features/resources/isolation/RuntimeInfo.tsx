/** The pair "Runs on · Runtime", its one-line hint and the limits there, as the environment page, the template page and the picker show them. */
import type { TFunction } from 'i18next';
import type { ReactNode } from 'react';
import { useTranslation } from 'react-i18next';

import { capabilityName } from '@/features/resources/capabilityWords';
import type { Boundary, RuntimeClass, RuntimeIsolation, RuntimeLimit } from '@/generated/api-types';
import { HelpHint } from '@/ui/HelpHint';
import { useIsolationWords } from './runtimes';

/** The limits of a runtime, one line each, with the capabilities it cannot give and why: the same lines the agent is told. */
export function LimitLines({ limits, unavailable, boundary }: { limits: readonly RuntimeLimit[]; unavailable?: Readonly<Record<string, string>>; boundary: Boundary }) {
  const words = useIsolationWords();
  const refused = Object.entries(unavailable ?? {});
  if (limits.length === 0 && refused.length === 0) return <span className="text-text-muted">–</span>;
  // A class can carry a keyed limit for its exact refusal. Prefer that line to
  // generic boundary wording so the same driver-specific reason is translated.
  const keyed = new Map<string, RuntimeLimit | undefined>(refused.map(([capability, reason]) => [capability, limits.find((limit) =>
    (limit.todo === null || limit.todo === undefined) && limit.why === reason && limit.touches.includes(capability))]));
  const used = new Set([...keyed.values()].filter((limit): limit is RuntimeLimit => limit !== undefined));
  return (
    <ul className="flex min-w-0 flex-col gap-1">
      {limits.filter((limit) => !used.has(limit)).map((limit) => <li key={limit.key} className="wrap-anywhere">{words.limit(limit)}</li>)}
      {refused.map(([key, why]) => {
        const line = keyed.get(key);
        return <li key={key} className="wrap-anywhere">{line === undefined ? <>{capabilityName(key, key)}: {words.unavailable(key, boundary, why)}</> : words.limit(line)}</li>;
      })}
    </ul>
  );
}

/** "Docker · gVisor" with "gVisor: its own kernel written in user space" under it. */
export function RuntimeValue({ driverTitle, runtimeTitle, boundary }: { driverTitle: string; runtimeTitle: string | null; boundary: Boundary }) {
  const words = useIsolationWords();
  return (
    <span className="flex min-w-0 flex-col">
      <span className="wrap-anywhere">{words.pair(driverTitle, runtimeTitle)}</span>
      <span className="text-sm text-text-muted">{runtimeTitle === null || runtimeTitle === '' ? words.boundary(boundary) : words.hint(runtimeTitle, boundary)}</span>
    </span>
  );
}

/** Whether an isolation view names the runtime its environment runs under (a driver that tells none apart does not). */
export function namesRuntime(isolation: RuntimeIsolation | null | undefined): isolation is RuntimeIsolation & { runtime_title: string } {
  return isolation !== null && isolation !== undefined && typeof isolation.runtime_title === 'string' && isolation.runtime_title !== '';
}

/** "Docker · gVisor" for the head of an environment's page, with what the runtime shares with the host as its hint. */
export function RuntimePair({ isolation }: { isolation: RuntimeIsolation & { runtime_title: string } }) {
  const words = useIsolationWords();
  return <span title={words.hint(isolation.runtime_title, isolation.boundary)}>{words.pair(isolation.driver_title, isolation.runtime_title)}</span>;
}

/** The Runtime and Limits rows of an environment, including a machine directory with no separate runtime. */
export function environmentRuntimeRows(isolation: RuntimeIsolation | null | undefined, t: TFunction): { name: string; value: ReactNode }[] {
  if (isolation === null || isolation === undefined) return [];
  return [
    { name: t('environments.overview.runtime'), value: <RuntimeValue driverTitle={isolation.driver_title} runtimeTitle={isolation.runtime_title ?? null} boundary={isolation.boundary} /> },
    { name: t('isolation.limitsHere'), value: <LimitLines limits={isolation.limits} unavailable={isolation.unavailable} boundary={isolation.boundary} /> },
  ];
}

/** The line under the runtime choice of the picker: what the runtime shares, the first limit that touches the template and a "?" with all of them. */
export function RuntimeLimitsLine({ runtime, limits }: { runtime: RuntimeClass; limits: readonly RuntimeLimit[] }) {
  const { t } = useTranslation();
  const words = useIsolationWords();
  const [first, ...rest] = limits;
  return (
    <p className="flex min-w-0 items-center gap-x-1 text-sm text-text-muted max-sm:flex-wrap" data-testid="runtime-limits">
      <span className={`min-w-0 truncate ${first === undefined ? 'flex-1' : 'max-sm:basis-full'}`} title={words.hint(runtime.title, runtime.boundary)}>{words.hint(runtime.title, runtime.boundary)}</span>
      {first !== undefined && <><span aria-hidden>·</span><span className="min-w-0 flex-1 truncate" title={words.limit(first)}>{words.limit(first)}</span></>}
      {(first !== undefined || Object.keys(runtime.unavailable ?? {}).length > 0) && (
        <HelpHint term={t('isolation.limitsHere')} what={<LimitLines limits={[first, ...rest].filter((limit): limit is RuntimeLimit => limit !== undefined)} unavailable={runtime.unavailable} boundary={runtime.boundary} />} />
      )}
    </p>
  );
}
