import { BindingPickerScope } from '@/features/resources/PickerContext';
/** Initial local choices for an imported thread, using the same placement answer as a new thread. */
import { useMutation } from '@tanstack/react-query';
import { useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { bindUnboundThread, placeUnboundThread } from '@/api/threadSetup';
import { createEnvironment } from '@/api/environments';
import { useActKey } from '@/lib/forms';
import type { PlaceRef } from '@/features/navigation/places';
import type { BindingEnvironment, Environment, Thread } from '@/generated/api-types';
import { Button } from '@/ui/Button';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';
import { ArrangementSection } from './Arrangement';
import { NEW_ENVIRONMENT, NO_PICKS, type Picks } from './choices';
import { StartAgentChoice, StartModelChoice } from './ModelChoice';
import { useStartOptions } from './options';
import { Where } from './Where';
import { TemporaryChoice } from '@/features/resources/environments/Temporary';
import { bindingReason } from './words';

const SUGGEST = { suggest: true };

export function SetUpThread({ thread, onClose }: { thread: Thread; onClose: () => void }) {
  const { t } = useTranslation();
  const actKey = useActKey();
  const [created, setCreated] = useState<Environment | null>(null);
  const [picks, setPicks] = useState<Picks>({ ...NO_PICKS, environment: thread.environment?.id ?? null });
  const [requesting, setRequesting] = useState<BindingEnvironment | null>(null);
  const [temporary, setTemporary] = useState(false);
  // The line that says where a choice of model changed the agent.
  const [agentNotice, setAgentNotice] = useState<string | null>(null);
  const { options, resolved, loading, failed, refreshing, retry } = useStartOptions(thread.topic_id, picks, SUGGEST);
  // A binding refusal leaves the accepted environment standing. Retry that same choice without making another.
  const made = useRef<{ intent: string; environment: Environment } | null>(null);
  const placed = useRef<{ choice: string; environment: string } | null>(null);
  const parent = thread.path.find((part) => part.id === thread.topic_id);
  const place: PlaceRef = { id: thread.topic_id, kind: 'topic', name: parent?.name ?? '', parentId: null, archived: false, status: null, folder: false };
  const pick = (patch: Partial<Picks>) => setPicks((before) => ({ ...before, ...patch }));
  const providerNames = new Map((resolved?.providers ?? []).map((row) => [row.id, row.entry.provider.name] as const));
  const save = useMutation({
    mutationFn: async () => {
      if (resolved === null || requesting !== null) return;
      if (resolved.provider === null || resolved.agent === null || resolved.model === null || resolved.arrangement === null) return;
      const choice = resolved.fresh ? NEW_ENVIRONMENT : resolved.environment?.environment.id;
      if (choice === undefined) return;
      let environment = choice;
      const body = { ...resolved.newEnvironment, home_node_id: thread.topic_id, ...(temporary ? { config: { 'env.lifetime': 'thread' } } : {}) };
      const intent = choice === NEW_ENVIRONMENT ? JSON.stringify(body) : choice;
      if (choice === NEW_ENVIRONMENT) {
        if (made.current?.intent !== intent) {
          const environment = await createEnvironment(body, actKey(body));
          made.current = { intent, environment };
          setCreated(environment);
        }
        environment = made.current.environment.id;
      }
      const held = placed.current;
      if (held?.choice !== intent) {
        const answer = await placeUnboundThread(thread.id, {
          use: environment,
          ...(resolved.environment?.directory ? { directory: resolved.environment.directory } : {}),
        });
        if (answer.environment !== null) placed.current = { choice: intent, environment: answer.environment.id };
      }
      await bindUnboundThread(thread.id, {
        agent_id: resolved.agent.id, provider_id: resolved.provider.id, arrangement: resolved.arrangement,
        model: resolved.model.model_name, ...(resolved.effort === null ? {} : { effort: resolved.effort }),
      });
    },
    onSuccess: onClose,
  });
  const missing = resolved?.missing[0];
  const why = loading ? t('start.why.loading') : requesting !== null ? t('start.why.environment') : refreshing || resolved?.settling ? t('start.why.checking')
    : missing === undefined ? null : t(`start.why.${missing.kind === 'agent' && missing.unreported ? 'agentUnreported' : missing.kind}`);
  const unavailable = resolved === null || options === null || why !== null;
  return <BindingPickerScope options={options} screen="thread-initial-setup" currentHomes={[thread.topic_id,...thread.path.map((node)=>node.id).reverse()]}>{<Dialog open onOpenChange={(open) => { if (!open && !save.isPending) onClose(); }} dismissible={!save.isPending}
    title={t('threadSetup.title')} footer={<>
      {why !== null && <span className="mr-auto text-sm text-text-muted">{why}</span>}
      <Button disabled={save.isPending} onClick={onClose}>{t('common.cancel')}</Button>
      <Button variant="primary" pending={save.isPending} disabled={unavailable} onClick={() => save.mutate()}>{t('threadSetup.save')}</Button>
    </>}>
    <div className="flex flex-col gap-4 outline-none" data-thread-setup data-autofocus tabIndex={-1}>
      {options === null || resolved === null ? loading ? <div className="h-32 animate-pulse rounded-md bg-bg-muted" aria-label={t('start.why.loading')} /> : <ErrorState compact error={failed} onRetry={retry} /> : <>
        <section className="flex flex-col gap-2"><h3 className="text-sm font-medium">{t('start.model.model')}</h3>
          <div className="flex flex-wrap items-center gap-2"><StartModelChoice options={options} resolved={resolved} names={providerNames} disabled={save.isPending || options.defaults.locked} onPick={(patch, notice) => { setAgentNotice(notice); pick(patch); }} /></div>
        </section>
        <section className="flex flex-col gap-2"><h3 className="text-sm font-medium">{t('start.agent.title')}</h3>
          <div className="flex flex-wrap items-center gap-2"><StartAgentChoice options={options} resolved={resolved} names={providerNames} disabled={save.isPending} onPick={(agent) => { setAgentNotice(null); pick({ agent, arrangement: null }); }} /></div>
          {agentNotice !== null && <p role="status" className="text-sm text-text-muted" data-agent-notice>{agentNotice}</p>}
        </section>
        <section className="flex flex-col gap-2"><h3 className="text-sm font-medium">{t('start.where.title')}</h3>
          <Where compact place={place} options={options} value={resolved.fresh ? NEW_ENVIRONMENT : resolved.environment?.environment.id ?? null}
            onChange={(environment) => { if (!save.isPending) pick({ environment, arrangement: null }); }} machines={[]} homeId={thread.org_id} requesting={requesting} onRequestingChange={setRequesting} onRefresh={retry}
            newEnvironment={picks.newEnvironment} onNewEnvironmentChange={(newEnvironment) => { if (!save.isPending) pick({ newEnvironment }); }}
            note={resolved.environmentNote} agentHere={resolved.agentHere} />
          {resolved.fresh && requesting === null && <TemporaryChoice checked={temporary} onChange={setTemporary} disabled={save.isPending} forThread />}
        </section>
        {requesting === null && <ArrangementSection resolved={resolved} refreshing={refreshing} onChange={(arrangement) => { if (!save.isPending) pick({ arrangement }); }} />}
      </>}
      {save.error !== null && <><ErrorAlert error={save.error} message={bindingReason(save.error, t) ?? undefined} />
        {created !== null && <InlineAlert tone="info">{t('start.made.environment', { name: created.name })}</InlineAlert>}
      </>}
    </div>
  </Dialog>}</BindingPickerScope>;
}
