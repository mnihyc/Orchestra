/** Speed and access: the faster speeds and the access programs that a manager of the provider enabled for this model, and nothing that was only offered.
 *
 * The two choices of a thread that are made among a few named things. Each opens on what the thread runs
 * with now and says where that comes from ("Fast · chosen by Dana", "Standard · as this model starts"), and
 * in one line what choosing it changes. The speeds and programs listed are those the model's managers
 * switched on, never those the source merely offered: a speed that is switched on and not chosen reads
 * "Standard", with Fast as a choice. Somebody who may not change the thread's setup reads the same, with the
 * choices switched off. These pieces are drawn by the model control; the choice of context is `ContextChoice`.
 */
import type { TFunction } from 'i18next';
import { Lock } from 'lucide-react';
import { type ReactNode, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { type Chooser, type ContextState, forceLabel, type SpeedOrAccess, speedMultiple, type State } from '@/features/composer/modelState';
import type { Rate } from '@/generated/api-types';
import type { TranslationKey } from '@/i18n';
import { SegmentedControl } from '@/ui/SegmentedControl';
import { Select } from '@/ui/Select';

/** The frame of one choice: a rule above it, except for the first. */
export const SECTION = 'flex flex-col gap-2 border-t border-border pt-3 first:border-t-0 first:pt-0';

/** The value of the choice that stands for the ordinary speed or for no program: a control cannot hold an empty value. */
export const NONE = '\u0000none';

/** Where a value comes from, in words: "chosen by Dana", "as this model starts". */
export function fromWords(state: State, chooser: Chooser | null, t: TFunction): string {
  if (state.stale) return t('composer.model.from.stale');
  if (state.from === 'chosen') return chooser === null ? t('composer.model.from.chosen') : t('composer.model.from.chosenBy', { name: chooser.name });
  if (state.from === 'above') return state.place === null ? t('composer.model.from.above') : t('composer.model.from.aboveBy', { place: state.place });
  return t('composer.model.from.start');
}

/** When a change of a parameter takes effect, as its adapter declares it. */
export const appliesWords = (state: Pick<State, 'apply'>, t: TFunction): string => t(`composer.model.applies.${state.apply}`);

/** The head of one choice: its name, and what the thread runs with now and where that comes from. */
export function ChoiceHead({ title, value, state, chooser, help }: { title: string; value: string; state: State; chooser: Chooser | null; help?: ReactNode }) {
  const { t } = useTranslation();
  return (
    <div className="flex flex-wrap items-baseline justify-between gap-x-3 gap-y-1">
      <div className="flex items-center gap-1"><h4 className="text-sm font-medium text-text">{title}</h4>{help}</div>
      <p className="flex items-center gap-1 text-xs text-text-muted" data-runs-with={state.kind}>
        {state.lockedBy !== null && <Lock aria-hidden className="size-4 text-text-subtle" />}
        <span>
          <span className="font-medium text-text">{value}</span> · {fromWords(state, chooser, t)}
        </span>
      </p>
    </div>
  );
}

/** Why a choice cannot be changed by this reader: a place above decides it, or it is for whoever manages the thread. */
export function LockedLine({ state }: { state: State }) {
  const { t } = useTranslation();
  if (state.lockedBy === null) return null;
  const place = state.lockedBy.node?.name ?? t(`composer.model.scope.${state.lockedBy.scope}`);
  return <p className="text-xs text-text-muted">{t('composer.model.lockedBy', { place })}</p>;
}

/** What a speed comes to, in one line: "Fast: quicker answers, counted at twice the rate here". Without anything to say of it, the adapter's own words stand. */
export function speedLine(label: string, id: string, rate: Rate | null, t: TFunction, exists: (key: string) => boolean, fallback: string): string {
  const aboutKey = `composer.model.speed.about.${id}`;
  // The key is built from an id the adapter gave, so it is not one the catalogue's types know.
  const about = exists(aboutKey) ? t(aboutKey as TranslationKey) : '';
  const multiple = speedMultiple(rate, id);
  const counted =
    multiple === null
      ? ''
      : multiple === 2
        ? t('composer.model.speed.counted.twice')
        : multiple === 0.5
          ? t('composer.model.speed.counted.half')
          : t('composer.model.speed.counted.times', { multiple });
  const what = [about, counted].filter((part) => part !== '').join(t('composer.model.speed.join'));
  const said = what === '' ? fallback : what;
  return said === '' ? label : t('composer.model.speed.line', { label, what: said });
}

interface ChoiceProps<S extends SpeedOrAccess> {
  state: S;
  /** Whether this reader may change it, which also needs that nothing else is being written. */
  writable: boolean;
  chooser: Chooser | null;
  /** The rate of the model, for what a speed costs. */
  rate: Rate | null;
  onChoose: (value: string | null) => void;
  performance?: (value: string | null) => string | null;
}

/** Speed: Standard, and the speeds the model's managers switched on. */
export function SpeedChoice({ state, writable, chooser, rate, onChoose, performance }: ChoiceProps<SpeedOrAccess>) {
  const { t, i18n } = useTranslation();
  const standard = t('composer.model.speed.standard');
  const inForce = forceLabel(state) ?? standard;
  // A model whose managers made a faster speed its start has no way back to Standard for a thread: the choice is not offered.
  const startLabel = state.start === null ? null : (state.open.find((one) => one.id === state.start)?.label ?? state.start);
  const segments = [
    { value: NONE, name: t('usagePerf.choices.tier.name', { name: standard }), description: <>{t('usagePerf.choices.tier.description')}{performance?.(null) && <span className="block">{performance(null)}</span>}</>, disabled: !writable || state.start !== null },
    ...state.open.map((one) => ({ value: one.id, name: t('usagePerf.choices.tier.name', { name: one.label }), tooltip: speedLine(one.label, one.id, rate, t, (key) => i18n.exists(key), state.description), description: <>{speedLine(one.label, one.id, rate, t, (key) => i18n.exists(key), state.description)}{performance?.(one.id) && <span className="block">{performance(one.id)}</span>}</>, disabled: !writable })),
  ];
  return (
    <section aria-label={t('composer.model.speed.title')} data-section="speed" className={SECTION}>
      <ChoiceHead title={t('composer.model.speed.title')} value={inForce} state={state} chooser={chooser} />
      <SegmentedControl label={t('composer.model.speed.title')} size="sm" value={state.force ?? NONE} onValueChange={(next) => onChoose(next === NONE ? null : next)} segments={segments} />
      {startLabel !== null && <p className="text-xs text-text-muted">{t('composer.model.speed.startsWith', { label: startLabel })}</p>}
      <LockedLine state={state} />
      <p className="text-xs text-text-subtle">{appliesWords(state, t)}</p>
    </section>
  );
}

/** Access: no program, and the programs the model's managers switched on. */
export function AccessChoice({ state, writable, chooser, onChoose, performance, reopen = false }: Omit<ChoiceProps<SpeedOrAccess>, 'rate'> & { reopen?: boolean }) {
  const [open, setOpen] = useState(reopen);
  const [wasRefused, setWasRefused] = useState(reopen);
  // Each new refusal opens the current choices once; the reader can close them afterward.
  if (wasRefused !== reopen) {
    setWasRefused(reopen);
    if (reopen) setOpen(true);
  }
  const { t } = useTranslation();
  const none = t('composer.model.access.none');
  const inForce = forceLabel(state) ?? none;
  const startLabel = state.start === null ? null : (state.open.find((one) => one.id === state.start)?.label ?? state.start);
  return (
    <section aria-label={t('composer.model.access.title')} data-section="access" className={SECTION}>
      <ChoiceHead title={t('composer.model.access.title')} value={inForce} state={state} chooser={chooser} />
      <Select
        open={open}
        onOpenChange={setOpen}
        label={t('composer.model.access.title')}
        hideLabel
        size="sm"
        value={state.force ?? NONE}
        disabled={!writable}
        onValueChange={(next) => onChoose(next === NONE ? null : next)}
        options={[{ value: NONE, name: t('usagePerf.choices.program.name', { name: none }), description: <>{t('composer.model.access.lineNone')}{performance?.(null) && <span className="block">{performance(null)}</span>}</>, disabled: state.start !== null }, ...state.open.map((one) => ({ value: one.id, name: t('usagePerf.choices.program.name', { name: one.label }), description: <>{t('usagePerf.choices.program.description')}{performance?.(one.id) && <span className="block">{performance(one.id)}</span>}</> }))]}
      />
      {startLabel !== null && <p className="text-xs text-text-muted">{t('composer.model.access.startsWith', { label: startLabel })}</p>}
      <p className="text-xs text-text-muted">{state.force === null ? t('composer.model.access.lineNone') : t('composer.model.access.line', { label: inForce })}</p>
      <LockedLine state={state} />
      <p className="text-xs text-text-subtle">{appliesWords(state, t)}</p>
    </section>
  );
}

/** For the pieces that need the context state alone. */
export type { ContextState };
