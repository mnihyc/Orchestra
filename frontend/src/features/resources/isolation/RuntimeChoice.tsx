/** The Runtime choice of the dialog that makes an environment: the runtimes the chosen host offers, what the template permits, and what each shares with the host. */
import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';

import type { Host, RuntimeClass, Template } from '@/generated/api-types';
import { Input } from '@/ui/Input';
import { Select, type SelectOption } from '@/ui/Select';
import { RuntimeLimitsLine } from './RuntimeInfo';
import { allowedRuntimes, classNamed, classesOfDriver, limitsTouching, namesOfDriver, policyRefusal, templateRuntime, templateSubjects, useIsolationWords } from './runtimes';

/** The value of the last option, which opens a field for a name the host registered that nobody listed. */
const OTHER = '\u0000other-runtime';
/** A blank setting delegates the runtime to the template or the selected host. */
const DEFAULT = '\u0000default-runtime';

export interface RuntimeOffer {
  /** The classes the hosts that could make it offer, by name. */
  classes: RuntimeClass[];
  /** What the template starts under, and what it permits besides. */
  own: string | null;
  permitted: string[];
  /** A default agreed by the candidate hosts, or unknown when their defaults differ. */
  hostDefault: string | null;
}

/** What the hosts offer a template, with its own default and the choice it permits. */
export function runtimeOffer(template: Template, hosts: readonly Pick<Host, 'drivers'>[]): RuntimeOffer {
  const classes = classesOfDriver(hosts, template.driver);
  const known = new Set(classes.flatMap((runtime) => [runtime.name, ...(runtime.aliases ?? [])]));
  // A host that names its runtimes without declaring them is offered by name.
  for (const name of namesOfDriver(hosts, template.driver)) if (!known.has(name)) classes.push({ name, title: name, boundary: 'shared_kernel', capabilities: {}, live_changes: { gateway_given: [], resources: false, mounts: false, capabilities: [] }, network_egress: [], recreate_preserves_files: false, keeps_software_on_stop: false, memory_overhead_mb: 0, limits: [] });
  const reporters = hosts.flatMap((host) => host.drivers.filter((driver) => driver.name === template.driver));
  const defaults = reporters.map((driver) => classNamed(classes, driver.default_runtime)?.name ?? null);
  const agreed = defaults[0] !== null && defaults[0] !== undefined && defaults.every((name) => name === defaults[0]);
  // A single older host can only say the names it offers; retain its established first-name fallback.
  const reportedDefault = reporters[0]?.default_runtime;
  const legacy = reporters.length === 1 && (reportedDefault === null || reportedDefault === undefined) ? classes[0]?.name ?? null : null;
  return { classes, own: templateRuntime(template.spec), permitted: allowedRuntimes(template.spec), hostDefault: agreed ? defaults[0] ?? null : legacy };
}

/** The runtime an environment is made under where the maker chose none: the template's own, else the host's default. */
export function defaultRuntime(offer: RuntimeOffer): string | null {
  return offer.own ?? offer.hostDefault;
}

/** The runtime to take when nothing was chosen and the template's own is one the place's policy refuses: the first the place allows, else none. */
export function allowedDefault(offer: RuntimeOffer, policy: unknown): string | null {
  const own = defaultRuntime(offer);
  if (own === null || policyRefusal(policy, own) === null) return null;
  return offer.classes.find((runtime) => policyRefusal(policy, runtime.name) === null && (offer.permitted.length === 0 || offer.permitted.includes(runtime.name)))?.name ?? null;
}

export function RuntimeChoice({ template, hosts, value, onChange, policy, disabled = false }: { template: Template; hosts: readonly Pick<Host, 'drivers'>[]; value: string; onChange: (runtime: string) => void; policy?: unknown; disabled?: boolean }) {
  const { t } = useTranslation();
  const words = useIsolationWords();
  const [other, setOther] = useState(false);
  const offer = runtimeOffer(template, hosts);
  // The template's own runtime may be one the place does not allow: then the first that it allows is asked for outright.
  const instead = value === '' ? allowedDefault(offer, policy) : null;
  useEffect(() => { if (instead !== null) onChange(instead); }, [instead, onChange]);
  if (offer.classes.length === 0) return null;
  const chosen = value === '' ? (instead ?? defaultRuntime(offer)) : value;
  const chosenClass = classNamed(offer.classes, chosen);
  const permits = (name: string): boolean => offer.permitted.length === 0 || offer.own === name || offer.permitted.includes(name);
  const refused = (name: string): string | null => {
    if (!permits(name)) return t('isolation.select.notInTemplate');
    const why = policyRefusal(policy, name);
    return why === null ? null : t(`isolation.select.${why}`);
  };
  const options: SelectOption<string>[] = offer.classes.map((runtime) => ({
    value: runtime.name,
    label: runtime.title,
    name: runtime.title,
    description: refused(runtime.name) ?? words.hint(runtime.title, runtime.boundary),
    disabled: refused(runtime.name) !== null,
  }));
  const driverTitle = hosts.flatMap((host) => host.drivers).find((driver) => driver.name === template.driver)?.title ?? template.driver;
  const pair = words.pair(driverTitle, chosenClass?.title ?? chosen);
  if (offer.classes.length === 1 && options[0]?.disabled !== true && !other && chosenClass !== undefined) {
    // One host runtime is permitted: keep the compact line. Refused alternatives stay visible with their reasons.
    return <RuntimeLimitsLine runtime={chosenClass} limits={limitsTouching(chosenClass.limits, templateSubjects(template.spec, true))} />;
  }
  return (
    <div className="flex min-w-0 flex-col gap-1">
      <Select
        label={t('isolation.runtime')}
        value={other ? OTHER : chosen ?? DEFAULT}
        valueLabel={other ? t('isolation.select.other') : undefined}
        disabled={disabled}
        onValueChange={(next) => {
          if (next === OTHER) { setOther(true); onChange(''); return; }
          setOther(false);
          onChange(next === DEFAULT ? '' : next);
        }}
        options={[{ value: DEFAULT, label: t('isolation.select.default'), name: t('isolation.select.default'), description: t('isolation.select.defaultLead') }, ...options, { value: OTHER, label: t('isolation.select.other'), name: t('isolation.select.other'), description: t('isolation.select.otherLead') }]}
      />
      {other && <Input label={t('isolation.select.otherLabel')} value={value} disabled={disabled} autoComplete="off" onChange={(event) => onChange(event.target.value.trim())} />}
      {!other && chosenClass !== undefined && <RuntimeLimitsLine runtime={chosenClass} limits={limitsTouching(chosenClass.limits, templateSubjects(template.spec, true))} />}
      <span className="sr-only">{pair}</span>
    </div>
  );
}
