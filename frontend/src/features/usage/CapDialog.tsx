/** The dialog that makes a cap or changes one: the sentence with its gaps as controls (whom it covers, the amount, its unit, the period, the models), followed by what else it narrows to and how it holds.
 *
 * "Everyone together may use at most 2M tokens a day on all models in project Web." is the sentence the
 * form completes, in the order the language says it. The models are named, or are a family by the
 * beginning of their names, or are the group a provider's managers made of several. Every choice stays visible: the
 * agents and the speeds it narrows to, the warning, what happens at the cap and to work that cannot be
 * counted, and a note. Whoever changes a cap changes what may change (its amount, what it covers, how it
 * holds); the rest is fixed when it is made. It is drawn by `CapHost` only while a form is requested.
 */
import { useMutation, useQuery } from '@tanstack/react-query';
import { X } from 'lucide-react';
import { type ReactNode, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { choiceOption } from '@/i18n/choices';

import { ApiError } from '@/api/client';
import { useCandidates } from '@/features/access/candidates';
import { nodeQuery } from '@/api/nodes';
import { providerQuery, providersQuery } from '@/api/resources';
import { useHarnessNames } from '@/features/resources/harnesses';
import { CurrentDetailPage } from '@/features/resources/CurrentDetailPage';
import { createCap, ratesQuery, updateCap } from '@/api/usage';
import type { Node, NodeRef, Provider } from '@/generated/api-types';
import {
  type CapDraft,
  check,
  draftOf,
  emptyDraft,
  type Faults,
  faultTarget,
  amountText,
  inputOf,
  isChange,
  modelPattern,
  type Who,
  WHO,
  updateOf,
} from '@/features/usage/capForm';
import { patternWords, placeWords, sentenceOf, whoWords } from '@/features/usage/words';
import { useFormatContext } from '@/lib/appearance';
import { useActKey } from '@/lib/forms';
import { Button } from '@/ui/Button';
import { Checkbox } from '@/ui/Checkbox';
import { Combobox, type ComboboxGroup, type ComboboxOption } from '@/ui/Combobox';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { StateLine } from '@/ui/StateLine';
import { InlineAlert } from '@/ui/InlineAlert';
import { Input } from '@/ui/Input';
import { DateField } from '@/ui/DateField';
import { RadioGroup } from '@/ui/RadioGroup';
import { Select } from '@/ui/Select';
import { Textarea } from '@/ui/Textarea';
import { toast } from '@/ui/Toast';
import type { CapRequest } from '@/features/usage/AddCap';

// ---------------------------------------------------------------- the sentence, in the order the language says it

/** A sentence whose gaps are controls: the text is the language's, and the gaps stand where it puts them. */
function Sentence({ template, slots }: { template: string; slots: Record<string, ReactNode> }) {
  const parts = template.split(/(\{\w+\})/g);
  return (
    <div className="flex flex-wrap items-center gap-x-2 gap-y-3 text-base text-text">
      {parts.map((part, index) => {
        const name = /^\{(\w+)\}$/.exec(part)?.[1];
        if (name !== undefined) return <span key={index}>{slots[name]}</span>;
        const text = part.trim();
        if (text === '') return null;
        // A stop or a comma belongs to the word before it, not to the gap that follows it.
        return (
          <span key={index} className={/^[.,;:。，；：！？]+$/.test(text) ? '-ml-2' : undefined}>
            {text}
          </span>
        );
      })}
    </div>
  );
}

// ---------------------------------------------------------------- the models a cap names

/** What may be named on a provider or among the providers usable at a place: routing names, display names and the groups. */
interface Known {
  models: { name: string; display: string }[];
  groups: string[];
  tiers: string[];
}

function knownOf(providers: readonly Pick<Provider, 'models'>[]): Known {
  const names = new Map<string, string>();
  const groups = new Set<string>();
  const tiers = new Set<string>();
  for (const provider of providers) {
    for (const model of provider.models) {
      names.set(model.model_name, model.display_name === '' ? model.model_name : model.display_name);
      for (const group of model.groups) groups.add(group);
      for (const tier of [...(model.parameters.service_tiers ?? []), ...(model.parameters.offered?.service_tiers ?? [])]) tiers.add(tier.id);
    }
  }
  return { models: [...names].map(([name, display]) => ({ name, display })), groups: [...groups].sort(), tiers: [...tiers].sort() };
}

function ModelsField({ models, known, onChange, error }: { models: string[]; known: Known; onChange: (models: string[]) => void; error?: string | undefined }) {
  const { t } = useTranslation();
  const [query, setQuery] = useState('');
  const typed = modelPattern(query);
  const add = (pattern: string) => {
    const clean = modelPattern(pattern);
    if (clean !== '' && !models.includes(clean)) onChange([...models, clean]);
    setQuery('');
  };
  const needle = query.trim().toLowerCase();
  const options: ComboboxOption<string>[] = [
    ...(typed === '' || models.includes(typed)
      ? []
      : [{ value: typed, label: t('caps.form.useTyped', { text: patternWords(typed, t) }), text: typed, description: typed.startsWith('@') ? t('caps.form.typedGroup') : typed.endsWith('*') ? t('caps.form.typedFamily') : t('caps.form.typedName') }]),
    ...known.groups
      .filter((group) => `@${group}`.includes(needle) && !models.includes(`@${group}`))
      .map((group) => ({ value: `@${group}`, label: patternWords(`@${group}`, t), text: `@${group}`, description: t('caps.form.groupOf') })),
    ...known.models
      .filter((model) => model.name.toLowerCase().includes(needle) || model.display.toLowerCase().includes(needle))
      .filter((model) => !models.includes(model.name.toLowerCase()))
      .map((model) => ({ value: model.name.toLowerCase(), label: model.display, text: model.display, description: model.name })),
  ];
  const groups: ComboboxGroup<string>[] = [{ id: 'models', label: t('caps.form.modelsList'), options }];
  return (
    <div className="flex min-w-[14rem] flex-1 flex-col gap-1">
      {models.length > 0 && (
        <ul aria-label={t('caps.form.modelsChosen')} className="flex flex-wrap gap-1">
          {models.map((pattern) => (
            <li key={pattern} className="flex items-center gap-1 rounded-full border border-border-strong bg-bg-subtle py-1 pr-1 pl-3 text-sm text-text">
              <span>{patternWords(pattern, t)}</span>
              <button
                type="button"
                aria-label={t('caps.form.modelRemove', { name: patternWords(pattern, t) })}
                onClick={() => onChange(models.filter((one) => one !== pattern))}
                className="flex size-5 items-center justify-center rounded-full text-text-muted hover:bg-bg-muted hover:text-text"
              >
                <X aria-hidden className="size-4" />
              </button>
            </li>
          ))}
        </ul>
      )}
      <Combobox
        label={t('caps.form.models')}
        hideLabel
        query={query}
        onQueryChange={setQuery}
        groups={groups}
        onSelect={(option) => add(option.value)}
        placeholder={models.length === 0 ? t('caps.form.modelsAll') : t('caps.form.modelsAnother')}
        error={error}
        empty={t('caps.form.modelsNone')}
      />
    </div>
  );
}

// ---------------------------------------------------------------- the dialog

/** The faults that are left once one of them is put right. */
function without(faults: Faults, member: string): Faults {
  return Object.fromEntries(Object.entries(faults).filter(([name]) => name !== member));
}

/** Whether the request names a place, a provider or neither (the whole system). */
function targetOf(request: CapRequest): { nodeId: string | undefined; providerId: string | undefined } {
  return {
    nodeId: request.edit === undefined ? request.node?.id : request.edit.node?.id,
    providerId: request.edit === undefined ? request.provider?.id : request.edit.provider?.id,
  };
}

export function CapDialog({ request, open, onClose }: { request: CapRequest; open: boolean; onClose: () => void }) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  const actKey = useActKey();
  const editing = request.edit;
  const { nodeId, providerId } = targetOf(request);

  const node = useQuery({ ...nodeQuery(nodeId ?? ''), enabled: nodeId !== undefined });
  const provider = useQuery({ ...providerQuery(providerId ?? ''), enabled: providerId !== undefined });
  const place: Node | undefined = node.data;
  const rootOrgId = place?.org_id ?? provider.data?.home.id;
  const usable = useQuery({ ...providersQuery('usable', place?.org_id ?? null), enabled: providerId === undefined && place !== undefined });
  const known = knownOf(providerId === undefined ? (usable.data ?? []) : provider.data === undefined ? [] : [provider.data]);
  const programs = useHarnessNames();
  const harnesses = programs.kinds;
  const rates = useQuery({ ...ratesQuery(nodeId ?? ''), enabled: nodeId !== undefined && providerId === undefined && editing === undefined, retry: false });

  const [draft, setDraft] = useState<CapDraft>(() =>
    editing === undefined ? emptyDraft({ models: (request.models ?? []).map(modelPattern), tiers: [...(request.tiers ?? [])] }) : draftOf(editing),
  );
  const [faults, setFaults] = useState<Faults>({});
  const [typedPerson, setTypedPerson] = useState('');
  const [chosenHolderName, setChosenHolderName] = useState<string | null>(() => editing === undefined ? null : whoWords(editing, t));
  const set = <Name extends keyof CapDraft>(name: Name, value: CapDraft[Name]) => {
    if (name === 'who') { setChosenHolderName(null); setTypedPerson(''); }
    setDraft((before) => ({ ...before, [name]: value }));
    // What a person changes is no longer what was wrong with it.
    setFaults((before) => without(before, name === 'noLimit' ? 'amount' : name === 'countedFrom' ? 'counted_from' : name === 'warn' ? 'warn_percent' : name));
  };

  const holders = useCandidates(typedPerson, rootOrgId ?? null, { kinds: ['user', 'group', 'role', 'members'], thing: place?.name ?? provider.data?.name ?? '', placeId: nodeId ?? rootOrgId });

  const make = useMutation({
    mutationFn: () => {
      if (editing !== undefined) return updateCap(editing.id, updateOf(draft, editing));
      const body = inputOf(draft, { nodeId, providerId });
      return createCap(body, actKey(body));
    },
    onSuccess: () => {
      toast({ tone: 'success', title: editing === undefined ? t('caps.form.made') : t('caps.form.changed') });
      onClose();
    },
    onError: (error) => {
      if (error instanceof ApiError) {
        const found: Faults = {};
        for (const fault of error.faults()) {
          const target = faultTarget(fault.path);
          if (target !== null) found[target] = fault.message;
        }
        if (Object.keys(found).length > 0) setFaults(found);
      }
    },
  });

  // Where the cap is, in the words a sentence has for it: the place or the provider it is set on, or the whole system.
  const whereNode: Pick<NodeRef, 'kind' | 'name' | 'redacted'> | null = editing !== undefined ? editing.node : place === undefined ? null : { kind: place.kind, name: place.name, redacted: false };
  const whereProvider: { name: string } | null = editing !== undefined ? editing.provider : (provider.data ?? (request.provider === undefined ? null : { name: request.provider.name ?? '' }));
  const chosenPerson = chosenHolderName ?? t('caps.form.somebody');
  const chosenGroup = chosenHolderName ?? t('caps.form.aGroup');
  const who =
    draft.who === 'everyone' ? t('caps.who.everyone') : draft.who === 'each' ? t('caps.who.each') : draft.who === 'user' ? chosenPerson : draft.who === 'group' ? t('caps.who.groupTogether', { name: chosenGroup }) : draft.who === 'eachGroup' ? t('caps.who.eachMember', { name: chosenGroup }) : chosenHolderName ?? t('caps.form.chooseHolder');
  const preview = sentenceOf(
    {
      who,
      amount: draft.noLimit ? null : (amountText(draft.amount, draft.meter) ?? '0'),
      figureText: draft.noLimit || amountText(draft.amount, draft.meter) !== null ? undefined : `… ${draft.meter === 'running' ? t('caps.form.units.running') : draft.meter === 'tokens' ? t('caps.unit.tokens') : t('caps.unit.credits')}`,
      meter: draft.meter,
      period: draft.meter === 'running' ? null : draft.period,
      counted_from: draft.period === 'total' && draft.countedFrom !== '' ? draft.countedFrom : null,
      models: draft.models,
      harness_kinds: draft.harnessKinds,
      tiers: draft.tiers,
      node: whereNode,
      provider: whereProvider,
    },
    t,
    locale,
    programs.title,
  );

  const unrated = (rates.data?.models ?? []).filter((model) => model.rate === null && (draft.models.length === 0 || draft.models.some((pattern) => pattern === model.model_name.toLowerCase() || (pattern.endsWith('*') && model.model_name.toLowerCase().startsWith(pattern.slice(0, -1)))))).length;
  const warnUnrated = draft.meter === 'credits' && unrated > 0;
  const programKinds = useMemo(() => [...new Set([...(harnesses.data ?? []).map((kind) => kind.kind), ...draft.harnessKinds])], [harnesses.data, draft.harnessKinds]);
  const selectedPrograms = useMemo(() => new Set(draft.harnessKinds), [draft.harnessKinds]);
  const tierChoices = [...new Set([...known.tiers, ...draft.tiers])];

  const submit = () => {
    const found = check(draft, t);
    setFaults(found);
    if (editing === undefined && draft.who !== 'everyone' && draft.who !== 'each' && chosenHolderName === null) { found.subject = t('caps.form.faults.holder'); setFaults(found); }
    if (Object.keys(found).length > 0) return;
    if (editing !== undefined && !isChange(updateOf(draft, editing))) {
      onClose();
      return;
    }
    make.mutate();
  };

  const slots: Record<string, ReactNode> = {
    who: (
      <Select
        size="sm"
        label={t('caps.form.who')}
        hideLabel
        value={draft.who}
        disabled={editing !== undefined}
        onValueChange={(next: Who) => set('who', next)}
        options={WHO.map((one) => ({
          value: one,
          label: t(`caps.form.whoChoices.${one}`),
        }))}
        fieldClassName="w-auto"
      />
    ),
    amount: (
      <Input
        label={t('caps.form.amount')}
        hideLabel
        value={draft.noLimit ? '' : draft.amount}
        disabled={draft.noLimit}
        inputMode="decimal"
        placeholder={draft.noLimit ? t('caps.form.noLimitShort') : draft.meter === 'tokens' ? t('caps.form.amountExample') : '10'}
        onChange={(event) => set('amount', event.target.value)}
        error={faults.amount}
        fieldClassName="w-[9rem]"
        className="h-(--control-sm) px-2 text-sm"
      />
    ),
    unit: (
      <Select
        size="sm"
        label={t('caps.form.unit')}
        hideLabel
        value={draft.meter}
        disabled={editing !== undefined}
        onValueChange={(next) => set('meter', next)}
        options={[
          { value: 'tokens' as const, label: t('caps.form.units.tokens') },
          { value: 'credits' as const, label: t('caps.form.units.credits') },
          { value: 'running' as const, label: t('caps.form.units.running') },
        ]}
        fieldClassName="w-auto"
      />
    ),
    per: (
      <Select
        size="sm"
        label={t('caps.form.per')}
        hideLabel
        value={draft.period}
        disabled={editing !== undefined}
        onValueChange={(next) => set('period', next)}
        options={[
          { value: 'day' as const, label: t('caps.form.periods.day') },
          { value: 'week' as const, label: t('caps.form.periods.week') },
          { value: 'month' as const, label: t('caps.form.periods.month') },
          { value: 'total' as const, label: t('caps.form.periods.total') },
        ]}
        fieldClassName="w-auto"
      />
    ),
    models: <ModelsField models={draft.models} known={known} onChange={(next) => set('models', next)} error={faults.models} />,
    where: <span className="text-text">{whereProvider !== null && whereNode === null ? t('caps.where.provider', { name: whereProvider.name }) : whereNode !== null ? placeWords(whereNode, t) : t('caps.where.system')}</span>,
  };

  const loading = (nodeId !== undefined && node.isPending) || (providerId !== undefined && provider.isPending);
  const failed = node.isError ? node.error : provider.isError ? provider.error : null;
  const sentence = draft.meter === 'running' ? t('caps.form.sentence.run') : t('caps.form.sentence.use');

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => !next && onClose()}
      title={editing === undefined ? t('caps.form.title') : t('caps.form.editTitle')}
      description={editing === undefined ? t('caps.form.about') : t('caps.form.editAbout')}
      size="lg"
      dismissible={!make.isPending}
      footer={
        <>
          <Button variant="ghost" disabled={make.isPending} onClick={onClose}>
            {t('common.cancel')}
          </Button>
          <Button variant="primary" pending={make.isPending} disabled={loading || failed !== null} onClick={submit}>
            {editing === undefined ? t('caps.form.make') : t('caps.form.save')}
          </Button>
        </>
      }
    >
      {failed !== null ? (
        <ErrorAlert error={failed} />
      ) : (
        <form
          className="flex flex-col gap-4"
          onSubmit={(event) => {
            event.preventDefault();
            submit();
          }}
        >
          <fieldset className="flex flex-col gap-3" aria-label={t('caps.form.sentenceLabel')}>
            <Sentence template={sentence} slots={slots} />
            {draft.who !== 'everyone' && draft.who !== 'each' && (
              <div className="flex flex-col gap-2">{holders.placePicker}
              <Combobox
                label={t('caps.form.holder')}
                query={typedPerson}
                onQueryChange={setTypedPerson}
                groups={holders.groups}
                onSelect={(option) => {
                  const chosen = holders.byValue.get(option.value);
                  if (chosen === undefined) return;
                  const input = chosen.input;
                  setDraft((before) => input.kind === 'user' ? { ...before, who: 'user', userId: input.id }
                    : input.kind === 'group' ? { ...before, who: before.who === 'eachGroup' ? 'eachGroup' : 'group', groupId: input.id }
                    : input.kind === 'members' ? { ...before, who: before.who === 'eachMembers' || before.who === 'eachGroup' ? 'eachMembers' : 'members', membersId: input.id }
                    : input.kind === 'role' ? { ...before, who: 'role', role: input.role, roleAt: input.at ?? nodeId ?? rootOrgId ?? '' } : before);
                  setChosenHolderName(chosen.name);
                  setTypedPerson('');
                  setFaults((before) => without(before, 'subject'));
                }}
                disabled={editing !== undefined}
                loading={holders.loading}
                error={faults.subject}
                hint={chosenHolderName ?? undefined}
                empty={t('access.candidates.none')}
                fieldClassName="max-w-[22rem]"
              />
              </div>
            )}
            <div className="flex flex-wrap items-start gap-x-6 gap-y-2">
              <Checkbox label={t('caps.form.noLimit')} hint={t('caps.form.noLimitAbout')} checked={draft.noLimit} onCheckedChange={(on) => set('noLimit', on)} />
              {draft.meter !== 'running' && draft.period === 'total' && (
                <DateField
                  label={t('caps.form.countedFrom')}
                  optional
                  value={draft.countedFrom}
                  onValueChange={(value) => set('countedFrom', value)}
                  error={faults.counted_from}
                  hint={t('caps.form.countedFromAbout')}
                  fieldClassName="w-[12rem]"
                />
              )}
            </div>
            {draft.meter === 'running' && <p className="text-sm text-text-muted">{t('caps.form.runningAbout')}</p>}
          </fieldset>

          {warnUnrated && (
            <InlineAlert tone="warning" title={t('caps.form.unratedTitle', { count: unrated })}>
              {t('caps.form.unratedAbout')}
            </InlineAlert>
          )}

          <p role="status" className="rounded-md bg-bg-subtle px-3 py-2 text-sm text-text-muted">
            <span className="font-medium text-text">{t('caps.form.itSays')} </span>
            {preview}
          </p>

          <div className="flex flex-col gap-4 rounded-lg border border-border p-4">
                <fieldset className="flex flex-col gap-2">
                  <legend className="text-sm font-medium text-text">{t('caps.form.agents')}</legend>
                  <p className="text-sm text-text-muted">{t('caps.form.agentsAbout')}</p>
                  {harnesses.isPending && <StateLine compact message={t('programList.loadingNames')} />}
                  {harnesses.isError && <ErrorState compact error={harnesses.error} onRetry={() => void harnesses.refetch()} />}
                  <CurrentDetailPage count={programKinds.length}>{(from, to) => <div className="flex flex-wrap gap-x-6 gap-y-2">
                    {programKinds.slice(from, to).map((kind) => (
                      <Checkbox
                        key={kind}
                        label={programs.title(kind)}
                        checked={selectedPrograms.has(kind)}
                        onCheckedChange={(on) => set('harnessKinds', on ? [...draft.harnessKinds, kind] : draft.harnessKinds.filter((one) => one !== kind))}
                      />
                    ))}
                  </div>}</CurrentDetailPage>
                </fieldset>
                <fieldset className="flex flex-col gap-2">
                  <legend className="text-sm font-medium text-text">{t('caps.form.speeds')}</legend>
                  <p className="text-sm text-text-muted">{t('caps.form.speedsAbout')}</p>
                  <div className="flex flex-wrap gap-x-6 gap-y-2">
                    {tierChoices.length === 0 && <span className="text-sm text-text-muted">{t('caps.form.speedsNone')}</span>}
                    {tierChoices.map((tier) => (
                      <Checkbox
                        key={tier}
                        label={tier.charAt(0).toUpperCase() + tier.slice(1)}
                        checked={draft.tiers.includes(tier)}
                        onCheckedChange={(on) => set('tiers', on ? [...draft.tiers, tier] : draft.tiers.filter((one) => one !== tier))}
                      />
                    ))}
                  </div>
                </fieldset>
                {draft.meter !== 'running' && (
                  <>
                    <Input
                      label={t('caps.form.warn')}
                      optional
                      inputMode="numeric"
                      value={draft.warn}
                      onChange={(event) => set('warn', event.target.value)}
                      error={faults.warn_percent}
                      hint={t('caps.form.warnAbout')}
                      fieldClassName="w-[12rem]"
                    />
                    <RadioGroup
                      variant="list"
                      label={t('caps.form.atLimit')}
                      value={draft.atLimit}
                      onValueChange={(next) => set('atLimit', next)}
                      options={[
                        { value: 'stop' as const, ...choiceOption(t, 'capAction.stop'), description: t('caps.form.atLimitStopAbout') },
                        { value: 'finish_turn' as const, ...choiceOption(t, 'capAction.finish'), description: t('caps.form.atLimitFinishAbout') },
                      ]}
                    />
                    <RadioGroup
                      variant="list"
                      label={t('caps.form.uncounted')}
                      value={draft.uncounted}
                      onValueChange={(next) => set('uncounted', next)}
                      options={[
                        { value: 'allow' as const, ...choiceOption(t, 'capAction.allow'), description: t('caps.form.uncountedAllowAbout') },
                        { value: 'block' as const, ...choiceOption(t, 'capAction.block'), description: t('caps.form.uncountedBlockAbout') },
                      ]}
                    />
                  </>
                )}
                <Textarea label={t('caps.form.note')} optional value={draft.note} onChange={(event) => set('note', event.target.value)} error={faults.note} rows={2} />
          </div>

          {make.isError && Object.keys(faults).length === 0 && (
            <ErrorAlert
              error={make.error}
              message={make.error instanceof ApiError && make.error.is('conflict.exists') ? t('caps.form.exists') : make.error instanceof ApiError && make.error.is('conflict.stale_revision') ? t('caps.form.stale') : undefined}
            />
          )}
        </form>
      )}
    </Dialog>
  );
}
