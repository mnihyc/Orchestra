/** Initial local choices for an imported thread, using the same placement answer as a new thread. */
import { useMutation } from '@tanstack/react-query';
import { useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { bindUnboundThread, placeUnboundThread } from '@/api/threadSetup';
import { createEnvironment } from '@/api/environments';
import { useActKey } from '@/lib/forms';
import { resourceChoice, resourcePreview } from '@/features/resource-facts/ResourceFacts';
import type { PlaceRef } from '@/features/navigation/places';
import type { Environment, Thread } from '@/generated/api-types';
import { Button } from '@/ui/Button';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { Select } from '@/ui/Select';
import { InlineAlert } from '@/ui/InlineAlert';
import { ArrangementSection } from './Arrangement';
import { modelName, NEW_ENVIRONMENT, NO_PICKS, type Picks } from './choices';
import { useStartOptions } from './options';
import { Where } from './Where';
import { agentWhy, bindingReason, providerWhy } from './words';

const SUGGEST = { suggest: true };

export function SetUpThread({ thread, onClose }: { thread: Thread; onClose: () => void }) {
  const { t } = useTranslation();
  const actKey = useActKey();
  const [created, setCreated] = useState<Environment | null>(null);
  const [picks, setPicks] = useState<Picks>({ ...NO_PICKS, environment: thread.environment?.id ?? null });
  const { options, resolved, loading, failed, refreshing, retry } = useStartOptions(thread.topic_id, picks, SUGGEST);
  // A binding refusal leaves the accepted environment standing. Retry that same choice without making another.
  const made = useRef<Environment | null>(null);
  const placed = useRef<{ choice: string; environment: string } | null>(null);
  const parent = thread.path.find((part) => part.id === thread.topic_id);
  const place: PlaceRef = { id: thread.topic_id, kind: 'topic', name: parent?.name ?? '', parentId: null, archived: false, status: null, folder: false };
  const pick = (patch: Partial<Picks>) => setPicks((before) => ({ ...before, ...patch }));
  const save = useMutation({
    mutationFn: async () => {
      if (resolved === null) return;
      if (resolved.provider === null || resolved.agent === null || resolved.model === null || resolved.arrangement === null) return;
      const choice = resolved.fresh ? NEW_ENVIRONMENT : resolved.environment?.environment.id;
      if (choice === undefined) return;
      let environment = choice;
      if (choice === NEW_ENVIRONMENT) {
        if (made.current === null) {
          const body = { home_node_id: thread.topic_id };
          made.current = await createEnvironment(body, actKey(body));
          setCreated(made.current);
        }
        environment = made.current.id;
      }
      const held = placed.current;
      if (held?.choice !== choice) {
        const answer = await placeUnboundThread(thread.id, {
          use: environment,
          ...(resolved.environment?.directory ? { directory: resolved.environment.directory } : {}),
        });
        if (answer.environment !== null) placed.current = { choice, environment: answer.environment.id };
      }
      await bindUnboundThread(thread.id, {
        agent_id: resolved.agent.id, provider_id: resolved.provider.id, arrangement: resolved.arrangement,
        model: resolved.model.model_name, ...(resolved.effort === null ? {} : { effort: resolved.effort }),
      });
    },
    onSuccess: onClose,
  });
  const missing = resolved?.missing[0];
  const why = loading ? t('start.why.loading') : refreshing || resolved?.settling ? t('start.why.checking')
    : missing === undefined ? null : t(`start.why.${missing.kind === 'agent' && missing.unreported ? 'agentUnreported' : missing.kind}`);
  const unavailable = resolved === null || options === null || why !== null;
  return <Dialog open onOpenChange={(open) => { if (!open && !save.isPending) onClose(); }} dismissible={!save.isPending}
    title={t('threadSetup.title')} footer={<>
      {why !== null && <span className="mr-auto text-sm text-text-muted">{why}</span>}
      <Button disabled={save.isPending} onClick={onClose}>{t('common.cancel')}</Button>
      <Button variant="primary" pending={save.isPending} disabled={unavailable} onClick={() => save.mutate()}>{t('threadSetup.save')}</Button>
    </>}>
    <div className="flex flex-col gap-4" data-thread-setup>
      {options === null || resolved === null ? loading ? <div className="h-32 animate-pulse rounded-md bg-bg-muted" aria-label={t('start.why.loading')} /> : <ErrorState compact error={failed} onRetry={retry} /> : <>
        <Select label={t('start.provider.title')} value={resolved.provider?.id ?? ''} disabled={options.defaults.locked || save.isPending}
          onValueChange={(provider) => pick({ provider, agent: null, model: null, effort: null, arrangement: null })}
          options={resolved.providers.map((row) => ({ value: row.id, label: row.entry.provider.name, preview: resourcePreview({ kind: 'provider', provider: row.entry.provider }), disabled: !row.choosable, description: row.choosable ? undefined : providerWhy(row, null, t) }))} />
        <Select label={t('start.agent.title')} value={resolved.agent?.id ?? ''} disabled={save.isPending}
          onValueChange={(agent) => pick({ agent, arrangement: null })}
          options={resolved.agents.map((row) => ({ value: row.id, label: row.entry.agent.name, preview: resourcePreview({ kind: 'agent', agent: row.entry.agent }), disabled: !row.startable, description: row.startable ? undefined : agentWhy(row, t) }))} />
        <Select label={t('start.model.model')} value={resolved.model?.model_name ?? ''} disabled={save.isPending}
          onValueChange={(model) => pick({ model, effort: null })}
          options={resolved.models.map((model) => ({ value: model.model_name, label: modelName(model), ...resourceChoice({ kind: 'model', model, providerId: resolved.provider?.id ?? '' }) }))} />
        <section className="flex flex-col gap-2"><h3 className="text-sm font-medium">{t('start.where.title')}</h3>
          <Where compact place={place} options={options} value={resolved.fresh ? NEW_ENVIRONMENT : resolved.environment?.environment.id ?? null}
            onChange={(environment) => { if (!save.isPending) pick({ environment, arrangement: null }); }} machines={[]} homeId={thread.org_id} />
        </section>
        <ArrangementSection resolved={resolved} refreshing={refreshing} onChange={(arrangement) => { if (!save.isPending) pick({ arrangement }); }} />
      </>}
      {save.error !== null && <><ErrorAlert error={save.error} message={bindingReason(save.error, t) ?? undefined} />
        {created !== null && <InlineAlert tone="info">{t('start.made.environment', { name: created.name })}</InlineAlert>}
      </>}
    </div>
  </Dialog>;
}
