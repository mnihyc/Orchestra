/** The row editor of a model, headed "What this model can be run with here": its efforts and the one a thread starts with, images, and the speeds, access programs and context that are off until a manager turns each on, with what the source offered and what a thread starts with; its groups, the limits that cover it, and its rate.
 *
 * What a model may be run with differs from model to model and from provider to provider, so it is the
 * model's own entry here and nothing a thread chooses: switching a speed on makes no thread fast, it only
 * puts the choice into the controls of the threads on this model. Beside each list that is switched on
 * stands what a thread on the model starts with; for the context that is all that is open unless a manager
 * names another start. What the server refuses is shown at the field it is about.
 */
import { useMutation } from '@tanstack/react-query';
import { type ReactNode, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { EmptyState } from '@/ui/EmptyState';
import { choiceOption, offeredChoice } from '@/i18n/choices';

import { createModel, updateModel } from '@/api/resources';
import { Hint } from '@/features/help/Hint';
import { firstStep, formatContext, readTokens } from '@/features/models/context';
import { accessProgramDisplayName } from '@/features/models/names';
import { type EntryForm, effortsOf, type Faults, faultsOf, formOf, hasLargest, parametersOf, refusedAt, type StepForm } from '@/features/resources/providers/entry';
import { ModelLimits } from '@/features/resources/providers/Caps';
import { GroupsField, groupsOf } from '@/features/resources/providers/Groups';
import { useHarnessTitle } from '@/features/resources/harnesses';
import type { ModelParameters, Provider, ProviderModel, ServiceTier } from '@/generated/api-types';
import { useAppearance } from '@/lib/appearance';
import { Button } from '@/ui/Button';
import { adminSpec } from '@/ui/AdminAct';
import { Checkbox } from '@/ui/Checkbox';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert } from '@/ui/ErrorState';
import { Input } from '@/ui/Input';
import { RadioGroup } from '@/ui/RadioGroup';
import { Select } from '@/ui/Select';
import { Switch } from '@/ui/Switch';
import { toast } from '@/ui/Toast';

/** The value of a choice that stands for "none": a select cannot hold an empty value. */
const NONE = '\u0000none';

const shownName = (model: Pick<ProviderModel, 'display_name' | 'model_name'>): string => (model.display_name === '' ? model.model_name : model.display_name);

/** A group of fields under a name of its own. */
function Part({ title, lead, children }: { title: string; lead?: ReactNode; children: ReactNode }) {
  return (
    <fieldset className="flex min-w-0 flex-col gap-3 rounded-lg border border-border p-3">
      <legend className="px-1 text-sm font-medium text-text">{title}</legend>
      {lead !== undefined && <p className="text-sm text-text-muted">{lead}</p>}
      {children}
    </fieldset>
  );
}

interface Item {
  id: string;
  label: string;
  /** What the source said of it: offered by it, or only typed in. */
  hint: string;
  on: boolean;
}

/** A list of things the source offers for a model, each a switch that is off until a manager turns it on, with the one a thread starts with. */
function Switches({
  title,
  lead,
  empty,
  items,
  onSwitch,
  anotherLabel,
  onAnother,
  start,
  disabled,
}: {
  title: string;
  lead: ReactNode;
  empty: string;
  items: readonly Item[];
  onSwitch: (id: string, on: boolean) => void;
  anotherLabel: string;
  onAnother: (id: string) => void;
  start: { label: string; none: string; hint: string; value: string; options: readonly { value: string; label: string }[]; error: string | undefined; onChange: (value: string) => void };
  disabled: boolean;
}) {
  const { t } = useTranslation();
  const [typed, setTyped] = useState('');
  const anyOn = items.some((item) => item.on);
  const addTyped = () => {
    if (typed.trim() === '') return;
    onAnother(typed.trim());
    setTyped('');
  };
  return (
    <Part title={title} lead={lead}>
      {items.length === 0 && <EmptyState compact title={empty} />}
      {items.map((item) => (
        <Switch key={item.id} label={item.label} hint={item.hint} checked={item.on} disabled={disabled} onCheckedChange={(on) => onSwitch(item.id, on)} />
      ))}
      {anyOn && (
        <Select
          label={start.label}
          hint={start.hint}
          value={start.value === '' ? NONE : start.value}
          disabled={disabled}
          error={start.error}
          onValueChange={(next) => start.onChange(next === NONE ? '' : next)}
          options={[{ value: NONE, ...choiceOption(t, 'modelStart.none'), description: start.none }, ...start.options.map((option) => ({ ...option, ...choiceOption(t, 'dynamic.value', { name: option.label, context: title }) }))]}
        />
      )}
      {!disabled && (
        <div className="flex flex-wrap items-end gap-2">
          <Input
            label={anotherLabel}
            value={typed}
            fieldClassName="min-w-0 flex-1 basis-[10rem]"
            onChange={(event) => setTyped(event.target.value)}
            // Enter switches the typed one on; it does not send the form of the dialog this stands in.
            onKeyDown={(event) => {
              if (event.key !== 'Enter') return;
              event.preventDefault();
              addTyped();
            }}
            autoComplete="off"
          />
          <Button size="sm" action="create" disabled={typed.trim() === ''} onClick={addTyped}>
            {t('providers.models.switchOn')}
          </Button>
        </div>
      )}
    </Part>
  );
}

/** One step of the rate: a prompt above a size pays these prices instead. */
function Step({ step, index, onChange, onRemove, disabled }: { step: StepForm; index: number; onChange: (step: StepForm) => void; onRemove: () => void; disabled: boolean }) {
  const { t } = useTranslation();
  const field = (name: keyof StepForm, label: string, optional = false) => (
    <Input label={label} optional={optional} inputMode="decimal" value={step[name]} disabled={disabled} onChange={(event) => onChange({ ...step, [name]: event.target.value })} autoComplete="off" />
  );
  return (
    <li className="flex flex-col gap-2 rounded-md border border-border p-2" aria-label={t('models.editor.rate.stepOf', { number: index + 1 })}>
      <div className="grid grid-cols-1 gap-2 sm:grid-cols-2">
        <Input label={t('models.editor.rate.over')} inputMode="numeric" value={step.over} disabled={disabled} onChange={(event) => onChange({ ...step, over: event.target.value })} autoComplete="off" />
        {field('input', t('providers.models.rateInput'))}
        {field('cached', t('providers.models.rateCached'), true)}
        {field('output', t('providers.models.rateOutput'))}
        {field('write', t('providers.models.rateWrite'), true)}
      </div>
      {!disabled && (
        <div>
          <Button size="sm" variant="ghost" action="delete" onClick={onRemove}>
            {t('models.editor.rate.removeStep')}
          </Button>
        </div>
      )}
    </li>
  );
}

export interface EntryFieldsProps {
  /** What the entry had: above all, what the source offered. */
  base: ModelParameters;
  form: EntryForm;
  onChange: (form: EntryForm) => void;
  faults: Faults;
  disabled: boolean;
  /** The groups of the provider's other models, offered beside the typed ones. */
  otherGroups: readonly string[];
  /** The limits that cover the model, where it has an entry to ask about. */
  limits?: ReactNode;
  provider: Pick<Provider, 'kind' | 'harness_kind'>;
}

/** The fields that say what a model can be run with here: the same in the dialog that adds a model and in its row editor. */
export function EntryFields({ base, form, onChange, faults, disabled, otherGroups, limits, provider }: EntryFieldsProps) {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  const harnessTitle = useHarnessTitle();
  const source = provider.harness_kind === null ? t('models.editor.theEndpoint') : harnessTitle(provider.harness_kind);
  const efforts = effortsOf(form);
  const window = readTokens(form.window);
  const largest = readTokens(form.largest);
  const offeredLargest = base.offered?.context_max ?? null;
  const offeredTiers = base.offered?.service_tiers ?? [];
  const offeredPrograms = base.offered?.access_programs ?? [];
  const step = firstStep(parametersOf(form, base).rate);

  // What is on is listed with what is offered, so that a switch that was typed in can be switched off again.
  const tierItems: Item[] = [...new Map([...offeredTiers, ...form.tiers].map((tier) => [tier.id, tier])).values()].map((tier) => ({
    id: tier.id,
    label: tier.label,
    hint: offeredTiers.some((offered) => offered.id === tier.id) ? t('models.editor.speeds.offered', { source }) : t('models.editor.typedIn', { source }),
    on: form.tiers.some((on) => on.id === tier.id),
  }));
  const programItems: Item[] = [...new Set([...offeredPrograms, ...form.programs])].map((name) => ({
    id: name,
    label: accessProgramDisplayName(name, t),
    hint: offeredPrograms.includes(name) ? t('models.editor.programs.offered', { source: provider.harness_kind === null ? t('models.editor.theEndpoint') : t('models.editor.thePlan') }) : t('models.editor.typedIn', { source }),
    on: form.programs.includes(name),
  }));

  const switchTier = (id: string, on: boolean) => {
    const label = [...offeredTiers, ...form.tiers].find((tier) => tier.id === id)?.label ?? id;
    const tiers: ServiceTier[] = on ? [...form.tiers.filter((tier) => tier.id !== id), { id, label }] : form.tiers.filter((tier) => tier.id !== id);
    // A start that is no longer switched on is no start.
    onChange({ ...form, tiers, defaultTier: tiers.some((tier) => tier.id === form.defaultTier) ? form.defaultTier : '' });
  };
  const switchProgram = (name: string, on: boolean) => {
    const programs = on ? [...form.programs.filter((program) => program !== name), name] : form.programs.filter((program) => program !== name);
    onChange({ ...form, programs, defaultProgram: programs.includes(form.defaultProgram) ? form.defaultProgram : '' });
  };
  const setStep = (index: number, next: StepForm) => onChange({ ...form, steps: form.steps.map((one, at) => (at === index ? next : one)) });

  return (
    <div className="flex flex-col gap-4">
      <Part title={t('models.editor.efforts.title')}>
        <Input help={<Hint id="model-efforts" />} label={t('providers.models.effortsLabel')} hint={t('models.editor.efforts.hint')} value={form.efforts} disabled={disabled} error={faults.efforts} onChange={(event) => onChange({ ...form, efforts: event.target.value })} autoComplete="off" />
        <Select
          label={t('models.editor.startsWith')}
          value={form.defaultEffort === '' || !efforts.includes(form.defaultEffort) ? NONE : form.defaultEffort}
          disabled={disabled || efforts.length === 0}
          error={faults.defaultEffort}
          onValueChange={(next) => onChange({ ...form, defaultEffort: next === NONE ? '' : next })}
          options={[{ value: NONE, ...choiceOption(t, 'modelStart.none'), description: t('providers.models.noDefault') }, ...efforts.map((effort) => ({ value: effort, ...(offeredChoice(t, 'effort', effort) ?? choiceOption(t, 'dynamic.value', { name: effort, context: t('models.editor.efforts.title') })) }))]}
        />
        <Checkbox label={t('providers.models.imagesLabel')} checked={form.images} disabled={disabled} onCheckedChange={(on) => onChange({ ...form, images: on })} />
      </Part>

      <Switches
        title={t('models.editor.speeds.title')}
        lead={<>{t('models.editor.speeds.lead', { source })} <Hint id="model-speeds" /></>}
        empty={t('models.editor.speeds.none', { source })}
        items={tierItems}
        onSwitch={switchTier}
        anotherLabel={t('providers.models.anotherSpeed')}
        onAnother={(id) => switchTier(id, true)}
        start={{
          label: t('models.editor.startsWith'),
          none: t('models.editor.standard'),
          hint: t('models.editor.speeds.startHint'),
          value: form.defaultTier,
          options: form.tiers.map((tier) => ({ value: tier.id, label: tier.label })),
          error: faults.tier,
          onChange: (value) => onChange({ ...form, defaultTier: value }),
        }}
        disabled={disabled}
      />

      <Switches
        title={t('models.editor.programs.title')}
        lead={<>{t('models.editor.programs.lead')} <Hint id="model-programs" /></>}
        empty={t('models.editor.programs.none')}
        items={programItems}
        onSwitch={switchProgram}
        anotherLabel={t('providers.models.anotherProgram')}
        onAnother={(name) => switchProgram(name, true)}
        start={{
          label: t('models.editor.startsWith'),
          none: t('models.editor.noProgram'),
          hint: t('models.editor.programs.startHint'),
          value: form.defaultProgram,
          options: form.programs.map((program) => ({ value: program, label: accessProgramDisplayName(program, t) })),
          error: faults.program,
          onChange: (value) => onChange({ ...form, defaultProgram: value }),
        }}
        disabled={disabled}
      />

      <Part title={t('models.editor.context.title')} lead={<>{t('models.editor.context.lead')} <Hint id="model-context" /></>}>
        <Input
          label={t('models.editor.context.window')}
          hint={t('models.editor.context.windowHint')}
          inputMode="numeric"
          value={form.window}
          disabled={disabled}
          error={faults.window}
          onChange={(event) => onChange({ ...form, window: event.target.value })}
          autoComplete="off"
        />
        <div className="flex flex-col gap-2">
          <Input
            label={t('models.editor.context.largest')}
            hint={largest === null ? t('models.editor.context.largestHint') : t('models.editor.context.largestFilled')}
            inputMode="numeric"
            value={form.largest}
            disabled={disabled}
            error={faults.largest}
            onChange={(event) => {
              const next = { ...form, largest: event.target.value };
              // Without a larger context there is nothing to start with but the model's own window.
              onChange(readTokens(event.target.value) === null ? { ...next, start: 'all', startNumber: '' } : next);
            }}
            autoComplete="off"
          />
          {offeredLargest !== null && (
            <p className="flex flex-wrap items-center gap-x-3 gap-y-1 text-sm text-text-muted">
              <span>{t('models.editor.context.offered', { size: formatContext(offeredLargest, locale), source })}</span>
              {!disabled && largest !== offeredLargest && (
                <Button action="save" size="sm" onClick={() => onChange({ ...form, largest: String(offeredLargest) })}>
                  {t('models.editor.context.use', { size: formatContext(offeredLargest, locale) })}
                </Button>
              )}
            </p>
          )}
        </div>
        {hasLargest(form) && (
          <div className="flex flex-col gap-2">
            <p className="text-sm font-medium text-text">{t('models.editor.startsWith')}</p>
            <RadioGroup
              label={t('models.editor.startsWith')}
              variant="list"
              disabled={disabled}
              value={form.start}
              onValueChange={(start) => onChange({ ...form, start })}
              options={[
                { value: 'all', ...choiceOption(t, 'modelStart.all'), ...(largest === null ? {} : { name: t('models.editor.context.startAllName', { size: formatContext(largest, locale) }) }) },
                {
                  value: 'window',
                  ...choiceOption(t, 'modelStart.window'),
                  ...(window === null ? {} : { name: t('models.editor.context.startWindowName', { size: formatContext(window, locale) }) }),
                  description: window === null ? t('models.editor.context.startWindowNone') : t('models.editor.context.startWindowAbout', { size: formatContext(window, locale) }),
                  disabled: window === null ? t('models.editor.context.startWindowNone') : false,
                },
                { value: 'number', ...choiceOption(t, 'modelStart.number'), description: t('models.editor.context.startNumber') },
              ]}
            />
            {form.start === 'number' && (
              <Input label={t('models.editor.context.startNumberLabel')} inputMode="numeric" value={form.startNumber} disabled={disabled} error={faults.start} onChange={(event) => onChange({ ...form, startNumber: event.target.value })} autoComplete="off" />
            )}
            {form.start !== 'number' && faults.start !== undefined && <p className="text-sm text-danger-text">{faults.start}</p>}
          </div>
        )}
        <p className="text-sm text-text-muted">{t('models.editor.context.shortened')}</p>
        {step !== null && largest !== null && largest > step && <p className="text-sm text-text-muted">{t('models.editor.context.step', { size: formatContext(step, locale) })}</p>}
      </Part>
      <p className="text-sm text-text-muted">{t('models.editor.switchedOn')}</p>

      <Part title={t('models.groups.title')}>
        <GroupsField groups={form.groups} offered={otherGroups} error={faults.groups} disabled={disabled} onChange={(groups) => onChange({ ...form, groups })} />
      </Part>

      {limits !== undefined && <Part title={t('models.limits.title')}>{limits}</Part>}

      <Part title={t('providers.models.rateTitle')} lead={<>{t('providers.models.rateLead')} <Hint id="model-rate" /></>}>
        <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
          <Input label={t('providers.models.rateInput')} inputMode="decimal" value={form.rate.input} disabled={disabled} onChange={(event) => onChange({ ...form, rate: { ...form.rate, input: event.target.value } })} autoComplete="off" />
          <Input label={t('providers.models.rateCached')} inputMode="decimal" value={form.rate.cached} disabled={disabled} onChange={(event) => onChange({ ...form, rate: { ...form.rate, cached: event.target.value } })} autoComplete="off" />
          <Input label={t('providers.models.rateOutput')} inputMode="decimal" value={form.rate.output} disabled={disabled} onChange={(event) => onChange({ ...form, rate: { ...form.rate, output: event.target.value } })} autoComplete="off" />
          <Input label={t('providers.models.rateWrite')} optional inputMode="decimal" value={form.rate.write} disabled={disabled} onChange={(event) => onChange({ ...form, rate: { ...form.rate, write: event.target.value } })} autoComplete="off" />
        </div>
        {form.tiers.map((tier) => (
          <Input
            key={tier.id}
            label={t('providers.models.rateTier', { speed: tier.label })}
            optional
            hint={t('providers.models.rateTierHint')}
            inputMode="decimal"
            value={form.multipliers[tier.id] ?? ''}
            disabled={disabled}
            onChange={(event) => onChange({ ...form, multipliers: { ...form.multipliers, [tier.id]: event.target.value } })}
            autoComplete="off"
          />
        ))}
        <div className="flex flex-col gap-2">
          <p className="text-sm font-medium text-text">{t('models.editor.rate.steps')}</p>
          <p className="text-sm text-text-muted">{t('models.editor.rate.stepsHint')}</p>
          {form.steps.length > 0 && (
            <ul className="flex flex-col gap-2">
              {form.steps.map((one, index) => (
                <Step key={index} step={one} index={index} disabled={disabled} onChange={(next) => setStep(index, next)} onRemove={() => onChange({ ...form, steps: form.steps.filter((_, at) => at !== index) })} />
              ))}
            </ul>
          )}
          {faults.steps !== undefined && <p className="text-sm text-danger-text">{faults.steps}</p>}
          {!disabled && (
            <div>
              <Button size="sm" action="create" onClick={() => onChange({ ...form, steps: [...form.steps, { over: '', input: '', cached: '', output: '', write: '' }] })}>
                {t('models.editor.rate.addStep')}
              </Button>
            </div>
          )}
        </div>
        {faults.rate !== undefined && <p className="text-sm text-danger-text">{faults.rate}</p>}
      </Part>
    </div>
  );
}

/** What the model's row editor opens with while it is being filled in. */
function useEntryForm(initial: () => EntryForm) {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  const [form, setForm] = useState<EntryForm>(initial);
  const faults = faultsOf(form, t, locale);
  return { form, setForm, faults, wrong: Object.keys(faults).length > 0 };
}

/** Adds a model: the routing name first, then the name people see, then what it can be run with. */
export function AddModelDialog({ provider, onClose }: { provider: Provider; onClose: () => void }) {
  const { t } = useTranslation();
  const { form, setForm, faults, wrong } = useEntryForm(() => formOf({}));
  const [routing, setRouting] = useState('');
  const [display, setDisplay] = useState('');
  const [shown, setShown] = useState(false);
  const add = useMutation({
    mutationFn: () => {
      const parameters = parametersOf(form, {});
      return createModel(provider.id, {
        model_name: routing.trim(),
        ...(display.trim() === '' ? {} : { display_name: display.trim() }),
        ...(Object.keys(parameters).length === 0 ? {} : { parameters }),
        ...(form.groups.length === 0 ? {} : { groups: form.groups }),
      });
    },
    onSuccess: (model) => {
      toast({ tone: 'success', title: t('providers.models.added', { name: shownName(model) }) });
      onClose();
    },
  });
  const refused = refusedAt(add.error);
  const submit = () => {
    setShown(true);
    if (routing.trim() === '' || wrong) return;
    add.mutate();
  };
  const routingFault = shown && routing.trim() === '' ? t('providers.models.routingEmpty') : refused.faults.routing;
  return (
    <Dialog
      open
      onOpenChange={(next) => !next && !add.isPending && onClose()}
      size="lg"
      title={t('providers.models.addTitle', { name: provider.name })}
      description={t('providers.models.addLead')}
      dismissible={!add.isPending}
      footer={
        <>
          <Button disabled={add.isPending} onClick={onClose}>
            {t('common.cancel')}
          </Button>
          <Button admin={adminSpec(provider, 'provider.manage')} variant="primary" pending={add.isPending} onClick={submit}>
            {t('providers.models.addSubmit')}
          </Button>
        </>
      }
    >
      <form
        className="flex flex-col gap-4 py-2"
        noValidate
        onSubmit={(event) => {
          event.preventDefault();
          submit();
        }}
      >
        <Input label={t('providers.models.routingName')} hint={t('providers.models.routingHint')} mono value={routing} error={routingFault} disabled={add.isPending} onChange={(event) => setRouting(event.target.value)} autoComplete="off" />
        <Input label={t('providers.models.displayName')} optional hint={t('providers.models.displayHint')} error={refused.faults.display} value={display} disabled={add.isPending} onChange={(event) => setDisplay(event.target.value)} autoComplete="off" />
        <EntryFields
          base={{}}
          form={form}
          onChange={(next) => {
            if (add.isError) add.reset();
            setForm(next);
          }}
          faults={{ ...refused.faults, ...faults }}
          disabled={add.isPending}
          otherGroups={groupsOf(provider.models).map((group) => group.name)}
          provider={provider}
        />
        {add.error !== null && refused.rest && <ErrorAlert error={add.error} />}
      </form>
    </Dialog>
  );
}

/** The row editor of a model: what it can be run with here, saved as the entry it is. */
export function ModelEditorDialog({ provider, model, onClose }: { provider: Provider; model: ProviderModel; onClose: () => void }) {
  const { t } = useTranslation();
  const { form, setForm, faults, wrong } = useEntryForm(() => formOf(model.parameters, model.groups));
  const save = useMutation({
    mutationFn: () => updateModel(provider.id, model.id, { parameters: parametersOf(form, model.parameters), groups: form.groups }),
    onSuccess: () => {
      toast({ tone: 'success', title: t('providers.models.saved', { name: shownName(model) }) });
      onClose();
    },
  });
  const refused = refusedAt(save.error);
  return (
    <Dialog
      open
      onOpenChange={(next) => !next && !save.isPending && onClose()}
      size="lg"
      title={t('models.editor.title')}
      description={t('models.editor.lead', { name: shownName(model), routing: model.model_name, provider: provider.name })}
      dismissible={!save.isPending}
      footer={
        <>
          <Button disabled={save.isPending} onClick={onClose}>
            {t('common.cancel')}
          </Button>
          <Button admin={adminSpec(provider, 'provider.manage')} variant="primary" pending={save.isPending} blocked={wrong} onClick={() => save.mutate()}>
            {t('providers.models.save')}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-4 py-2">
        <EntryFields
          base={model.parameters}
          form={form}
          onChange={(next) => {
            if (save.isError) save.reset();
            setForm(next);
          }}
          faults={{ ...refused.faults, ...faults }}
          disabled={save.isPending}
          otherGroups={groupsOf(provider.models).map((group) => group.name)}
          limits={<ModelLimits provider={provider} model={model} />}
          provider={provider}
        />
        {save.error !== null && refused.rest && <ErrorAlert error={save.error} />}
      </div>
    </Dialog>
  );
}
