/** Four ordinary choices; optional setup stays under Options. The native answer still decides every permission and value. */
import { useQuery } from '@tanstack/react-query';
import { PlugZap } from 'lucide-react';
import { type ReactNode, useEffect, useId, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { EmptyState } from '@/ui/EmptyState';
import { choiceOption, offeredChoice } from '@/i18n/choices';

import { machinesQuery, providersQuery } from '@/api/resources';
import { ResourceHint, resourcePreview, resourceChoice } from '@/features/resource-facts/ResourceFacts';
import { useHarnessTitle } from '@/features/resources/harnesses';
import { ResourceCreationActions } from '@/features/resources/ResourceCreation';
import { providerPickerLabels } from '@/features/resources/providers/labels';
import { useKnownPlaces } from '@/features/navigation/placeLabels';
import { RepositoryChoice } from '@/features/source/RepositoryChoice';
import { nodeQuery } from '@/api/nodes';
import { NewPage, type NewPagePreset } from '@/features/access/NewPage';
import type { PlaceRef } from '@/features/navigation/places';
import { Button } from '@/ui/Button';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';
import { Input } from '@/ui/Input';
import { PromptScope, usePrompt } from '@/ui/PromptScope';
import { Select } from '@/ui/Select';
import { Textarea } from '@/ui/Textarea';

import { ArrangementSection } from './Arrangement';
import { type Mode, modelName, NEW_ENVIRONMENT, NO_PICKS, type Picks } from './choices';
import { useStartOptions } from './options';
import { useStart } from './start';
import { DirectorySign } from './DirectorySign';
import { Where, WhereOptions } from './Where';
import { TemporaryChoice } from '@/features/resources/environments/Temporary';
import { agentWhy, bindingReason, providerWhy, reasonWords } from './words';

/** What the person typed and chose in the inline starter, which the dialog opens with. */
export interface StartDraft {
  picks: Picks;
  message: string;
}

/** A dialog suggests the first of each choice where the place has no default: the person is there to choose. */
const SUGGEST: Mode = { suggest: true };

function Step({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section className="flex flex-col gap-2 border-t border-border pt-3 first:border-t-0 first:pt-0">
      <div className="flex flex-col">
        <h3 className="flex items-center gap-1 text-base font-semibold text-text">
          {title}
        </h3>
      </div>
      {children}
    </section>
  );
}

interface NewThreadDialogProps { parent: PlaceRef; open: boolean; onClose: () => void; draft?: StartDraft }

export function NewThreadDialog(props: NewThreadDialogProps) {
  const layer = usePrompt(props.open, 'scope');
  return <PromptScope layer={layer}><ThreadSetup {...props} /></PromptScope>;
}

function ThreadSetup({ parent, open, onClose, draft }: NewThreadDialogProps) {
  const { t } = useTranslation();
  const optionsId = useId();
  const harnessTitle = useHarnessTitle(open);
  const [picks, setPicks] = useState<Picks>(draft?.picks ?? NO_PICKS);
  const [access, setAccess] = useState<NewPagePreset>('defaults');
  const [letIntoEnvironment, setLetIntoEnvironment] = useState(true);
  const [name, setName] = useState('');
  const [directory, setDirectory] = useState('');
  const [prepareSource, setPrepareSource] = useState(true);
  const [temporary, setTemporary] = useState(false);
  const [message, setMessage] = useState(draft?.message ?? '');
  const [advanced, setAdvanced] = useState(false);
  const making = parent.kind !== 'topic';

  const { options, resolved, loading, failed, refreshing, retry } = useStartOptions(parent.id, picks, SUGGEST);
  const extraOpen = advanced || resolved?.missing.some((row) => row.kind === 'arrangement') === true;
  // Drawn beside a choice and never the choice: what a provider is, where it runs and how much of it is used.
  const providers = useQuery({ ...providersQuery('all'), enabled: open });
  const places = useKnownPlaces({ trees: false });
  const machines = useQuery({ ...machinesQuery, enabled: open && extraOpen });
  const place = useQuery(nodeQuery(parent.id));
  const known = new Map((providers.data ?? []).map((provider) => [provider.id, provider]));
  const providerNames=providerPickerLabels(resolved?.providers.map((row)=>row.entry.provider) ?? [],providers.data ?? [],places);

  const make = useStart(() => onClose());
  const refusal = useRef<HTMLDivElement>(null);
  const refused = make.error;
  // A refusal stands at the end of a form that scrolls: it is brought into view.
  useEffect(() => {
    if (refused !== null) refusal.current?.scrollIntoView({ block: 'nearest' });
  }, [refused]);

  const pick = (patch: Partial<Picks>) => setPicks((before) => ({ ...before, ...patch }));

  const why: string | null =
    options === null || resolved === null
      ? loading
        ? t('start.why.loading')
        : t('start.why.failed')
      : refreshing || resolved.settling
        ? t('start.why.checking')
        : resolved.missing[0] === undefined
          ? null
          : t(`start.why.${resolved.missing[0].kind === 'agent' && resolved.missing[0].unreported ? 'agentUnreported' : resolved.missing[0].kind}`);

  const create = () => {
    if (options === null || resolved === null) return;
    make.start({
      place: parent,
      options,
      resolved,
      name: name.trim(),
      directory: directory.trim(),
      prepareSource,
      temporary,
      privately: access === 'private',
      generalAccess: access,
      letIntoEnvironment,
      message: message.trim(),
    });
  };

  const whereValue = resolved === null ? null : resolved.fresh ? NEW_ENVIRONMENT : (resolved.environment?.environment.id ?? picks.environment);
  const footerReason = make.pending && make.step !== null ? t(`start.making.${make.step}`) : !loading && (options === null || resolved === null) ? null : why;
  return (
    <Dialog
      open={open}
      onOpenChange={(next) => !next && !make.pending && onClose()}
      size="lg"
      title={t(making ? 'start.titleTopic' : 'start.title', { parent: parent.name })}
      dismissible={!make.pending}
      footer={
        <>
          {footerReason !== null && <span className="basis-full text-sm text-text-muted">{footerReason}</span>}
          <div role="group" aria-label={t('changeCompact.startActions')} className="flex basis-full flex-wrap justify-end gap-2" data-start-actions>
            <Button disabled={make.pending} onClick={onClose}>
              {t('common.cancel')}
            </Button>
            <Button variant="primary" pending={make.pending} disabled={why !== null} onClick={create}>
              {t('start.create')}
            </Button>
          </div>
        </>
      }
    >
      <div className="flex flex-col gap-4 py-2 outline-none" data-new-thread data-autofocus tabIndex={-1}>
        {options === null || resolved === null ? (
          loading ? null : (
            <><ErrorState compact error={failed} onRetry={retry} />
              {picks.environment !== null && <Button size="sm" className="self-start" onClick={() => pick({ environment: null, arrangement: null })}>{t('startRecovery.elsewhere')}</Button>}
            </>
          )
        ) : (
          <>
            <Step title={t('start.provider.title')}>
              {resolved.providers.length === 0 ? (
                <InlineAlert tone="warning">{t('start.provider.none')}</InlineAlert>
              ) : (
                <Select
                  label={t('start.provider.title')}
                  hideLabel
                  value={resolved.provider?.id ?? ''}
                  disabled={options.defaults.locked}
                  onValueChange={(next) => pick({ provider: next, agent: null, model: null, effort: null, arrangement: null })}
                  options={resolved.providers
                    .map((row) => {
                      const full = known.get(row.id);
                      const ref = row.entry.provider;
                      return {
                        value: row.id,
                        preview: resourcePreview({ kind: 'provider', provider: full ?? ref }),
                        disabled: !row.choosable,
                        label: (
                          <span className="flex flex-wrap items-center gap-2">
                            <PlugZap aria-hidden />
                            {providerNames.get(ref.id) ?? ref.name}
                            {row.isDefault && <span className="rounded-full border border-accent/40 px-2 text-xs font-normal text-accent-text">{t('start.topicDefault')}</span>}
                          </span>
                        ),
                        description: (
                          <span className="flex flex-wrap gap-x-3 gap-y-1">
                            <ResourceHint subject={{ kind: 'provider', provider: full ?? ref }} />
                            {!row.choosable && <span>{providerWhy(row, full?.owner.display_name ?? null, t)}</span>}
                          </span>
                        ),
                      };
                    })}
                />
              )}
              {options.defaults.locked && <p className="text-sm text-text-muted">{t('start.provider.locked')}</p>}
            </Step>

            <Step title={t('start.agent.title')}>
              {resolved.agents.length === 0 ? (
                <EmptyState compact title={t('start.agent.none')} />
              ) : (
                <Select
                  label={t('start.agent.title')}
                  hideLabel
                  value={resolved.agent?.id ?? ''}
                  onValueChange={(next) => pick({ agent: next, arrangement: null })}
                  options={resolved.agents.map((row) => ({
                    value: row.id,
                    label: row.entry.agent.name,
                    preview: resourcePreview({ kind: 'agent', agent: row.entry.agent, provider: known.get(resolved.provider?.id ?? ''), models: resolved.models }),
                    description: row.startable ? harnessTitle(row.entry.agent.harness_kind) : agentWhy(row, t),
                    disabled: !row.startable,
                  }))}
                />
              )}
            </Step>

            {(resolved.providers.every((row) => !row.choosable) || resolved.agents.every((row) => !row.startable)) && <ResourceCreationActions onReadChoices={retry} />}
            <Step title={t('start.model.model')}>
              {resolved.provider === null ? (
                <p className="text-sm text-text-muted">{t('start.model.pickProvider')}</p>
              ) : resolved.models.length === 0 ? (
                <InlineAlert tone="warning">{t('start.model.none')}</InlineAlert>
              ) : (
                <div className="flex flex-wrap items-start gap-3">
                  <Select
                    label={t('start.model.model')}
                    hideLabel
                    value={resolved.model?.model_name ?? ''}
                    onValueChange={(next) => pick({ model: next, effort: null })}
                    options={[
                      ...resolved.models.map((model) => ({ value: model.model_name, label: modelName(model), ...resourceChoice({ kind: 'model', model, providerId: resolved.provider?.id ?? '', usage: known.get(resolved.provider?.id ?? '')?.choice_usage }) })),
                      // What the provider lists and the person may not use is shown, off, with why.
                      ...resolved.closedModels.map((model) => ({ value: model.model_name, label: modelName(model), ...resourceChoice({ kind: 'model', model, providerId: resolved.provider?.id ?? '', usage: known.get(resolved.provider?.id ?? '')?.choice_usage }), description: model.reason === null ? t('start.model.closed') : reasonWords(model.reason), disabled: true })),
                    ]}
                    fieldClassName="min-w-(--field-wide) flex-1"
                  />
                </div>
              )}
            </Step>

            <Step title={t('start.where.title')}>
              <Where compact place={parent} options={options} value={whereValue} onChange={(next) => pick({ environment: next, arrangement: null })} machines={machines.data ?? []} homeId={place.data?.org_id ?? ''} />
              {resolved.fresh && <TemporaryChoice checked={temporary} onChange={setTemporary} disabled={make.pending} />}
            </Step>
          </>
        )}

        <Button variant="ghost" className="self-start" aria-expanded={extraOpen} aria-controls={extraOpen ? optionsId : undefined} onClick={() => setAdvanced((value) => !value)}>{t('start.options.title')}</Button>
        {extraOpen && <div id={optionsId} role="region" aria-label={t('start.options.title')} className="flex flex-col gap-3">
          {resolved !== null && options !== null && <>
              {resolved.efforts.length > 0 && <Select label={t('start.model.effort')} value={resolved.effort ?? ''} onValueChange={(next) => pick({ effort: next })} options={resolved.efforts.map((value) => ({ value, ...(offeredChoice(t, 'effort', value) ?? choiceOption(t, 'dynamic.value', { name: value, context: t('start.model.effort') })) }))} />}
              <WhereOptions place={parent} options={options} value={whereValue} onChange={(next) => pick({ environment: next, arrangement: null })} machines={machines.data ?? []} machinesLoading={machines.isPending} homeId={place.data?.org_id ?? ''} />
              <Input
                label={t('start.where.directory')}
                optional
                hint={resolved.environment !== null && resolved.environment.directory !== '' ? t('start.where.directoryIs', { directory: resolved.environment.directory }) : t('start.where.directoryHint')}
                aside={<DirectorySign environmentId={resolved.fresh ? null : (resolved.environment?.environment.id ?? null)} directory={directory.trim()} />}
                value={directory}
                onChange={(event) => setDirectory(event.target.value)}
                autoComplete="off"
              />
              <RepositoryChoice source={options.source} checked={prepareSource} onChange={setPrepareSource} />
              <ArrangementSection resolved={resolved} refreshing={refreshing} onChange={(type) => pick({ arrangement: type })} />
          <NewPage place={parent} placement={resolved.environment?.environment.placement ?? 'managed'} preset={access} onPresetChange={setAccess}
            letIntoEnvironment={letIntoEnvironment} onLetIntoEnvironmentChange={setLetIntoEnvironment}
            summary={resolved.environment?.environment.placement === 'connected' ? options.access.connected : options.access.managed}
            {...(making ? { audienceName: t('places.project.newTopic') } : {})} />
          </>}
          <Input label={t('start.options.name')} optional hint={t('start.options.nameHint')} value={name} onChange={(event) => setName(event.target.value)} autoComplete="off" />
          <Textarea label={t('start.message.title')} rows={3} value={message} onChange={(event) => setMessage(event.target.value)} />
        </div>}

        {make.error !== null && (
          <div ref={refusal} className="flex flex-col gap-2">
            <ErrorAlert error={make.error} message={bindingReason(make.error, t) ?? undefined} />
            {(make.standing.topic !== null || make.standing.environment !== null) && (
              <InlineAlert tone="info">
                {[make.standing.topic === null ? null : t('start.made.topic', { name: make.standing.topic.name }), make.standing.environment === null ? null : t('start.made.environment', { name: make.standing.environment.name })]
                  .filter((line): line is string => line !== null)
                  .join(' ')}
              </InlineAlert>
            )}
          </div>
        )}
      </div>
    </Dialog>
  );
}
