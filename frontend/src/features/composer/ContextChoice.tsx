/** The context of a thread: the sizes the model's managers opened, between the model's own window and the largest, with a field for another number, the sentence its adapter gives about where the conversation is shortened, and the line about a size that crosses a step of the model's rate.
 *
 * The thread opens on what it runs with now: all that is open unless the model's managers named another
 * start or somebody chose less ("1M · as this model starts"). Where the conversation is shortened below
 * the context is worked out from the size and chosen by nobody, so it is only told, in the adapter's words.
 * It says when a change takes effect, as the adapter declares it ("from the next turn", "in a new session").
 */
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { appliesWords, ChoiceHead, LockedLine, SECTION } from '@/features/composer/SpeedAndAccess';
import type { Chooser, ContextState } from '@/features/composer/modelState';
import { contextChoices, firstStep, formatContext, readTokens, stepBelow } from '@/features/models/context';
import { useAppearance } from '@/lib/appearance';
import { Button } from '@/ui/Button';
import { Input } from '@/ui/Input';
import { SegmentedControl } from '@/ui/SegmentedControl';
import { ChoiceFieldHelp } from '@/ui/ChoiceField';

export function ContextChoice({ state, writable, chooser, onChoose }: { state: ContextState; writable: boolean; chooser: Chooser | null; onChoose: (size: number) => void }) {
  const { t } = useTranslation();
  const locale = useAppearance((appearance) => appearance.locale);
  const [typed, setTyped] = useState('');
  const [shown, setShown] = useState(false);
  const sizes = contextChoices({ least: state.least, own: state.window, largest: state.largest });
  // A size that is chosen and is none of the round ones is a choice too.
  const choices = sizes.includes(state.force) ? sizes : [...sizes, state.force].sort((one, other) => other - one);
  const step = firstStep(state.rate);
  const crossing = (size: number) => stepBelow(state.rate, size) !== null;
  const options = choices.map((size) => ({
    value: String(size),
    name: crossing(size) ? t('composer.model.context.higherRate', { size: formatContext(size, locale) }) : formatContext(size, locale),
    description: crossing(size) ? t('composer.model.context.crosses', { size: formatContext(step ?? 0, locale) }) : undefined,
    disabled: !writable,
  }));

  const asked = readTokens(typed);
  const fault =
    typed.trim() === ''
      ? null
      : asked === null
        ? t('composer.model.context.faultSize')
        : asked < state.least
          ? t('composer.model.context.faultLow', { least: formatContext(state.least, locale) })
          : asked > state.largest
            ? t('composer.model.context.faultHigh', { largest: formatContext(state.largest, locale) })
            : null;
  const set = () => {
    setShown(true);
    if (asked === null || fault !== null) return;
    onChoose(asked);
    setTyped('');
    setShown(false);
  };

  return (
    <section aria-label={t('composer.model.context.title')} data-section="context" className={SECTION}>
      <ChoiceHead title={t('composer.model.context.title')} value={formatContext(state.force, locale)} state={state} chooser={chooser}
        help={<ChoiceFieldHelp label={t('composer.model.context.title')} options={options} />} />
      <SegmentedControl
        label={t('composer.model.context.title')}
        size="sm"
        value={String(state.force)}
        onValueChange={(next) => onChoose(Number(next))}
        segments={options}
      />
      {writable && (
        <div className="flex flex-wrap items-start gap-2">
          <Input
            label={t('composer.model.context.another')}
            hint={t('composer.model.context.range', { least: formatContext(state.least, locale), largest: formatContext(state.largest, locale) })}
            error={shown ? (fault ?? undefined) : undefined}
            inputMode="numeric"
            value={typed}
            fieldClassName="min-w-0 flex-1 basis-[10rem]"
            onChange={(event) => {
              setTyped(event.target.value);
              setShown(false);
            }}
            onKeyDown={(event) => {
              if (event.key !== 'Enter') return;
              event.preventDefault();
              set();
            }}
            autoComplete="off"
          />
          <Button action="save" size="sm" className="mt-6" disabled={typed.trim() === ''} onClick={set}>
            {t('composer.model.context.set')}
          </Button>
        </div>
      )}
      {state.description !== '' && <p className="text-xs text-text-muted">{state.description}</p>}
      <LockedLine state={state} />
      <p className="text-xs text-text-subtle">{appliesWords(state, t)}</p>
    </section>
  );
}
