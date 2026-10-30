/** Adopt…: a session a harness already holds, taken in as a thread of a topic the person chooses, with what would happen said before anything does.
 *
 * The topic is chosen with the place picker every form uses, offering topics alone. As soon as one is chosen the
 * server is asked what adopting would do, and the answer is written out: the environment registered for the
 * session's folder and the thread's directory in it, or the folder to register first; the model the thread will
 * run on, or why a person will choose one; how many of the session's earlier turns come with it, marked as from
 * the session before Orch and never sent to the agent again, or why none do. Nothing happens until Adopt is
 * pressed, and a session that is a thread already says so and links to it.
 */
import { useMutation, useQuery } from '@tanstack/react-query';
import { Link, useNavigate } from '@tanstack/react-router';
import { useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { adoptionPlanQuery, adoptSession } from '@/api/adoption';
import { placePath } from '@/features/navigation/places';
import { Facts } from '@/features/resources/Facts';
import { PlacePicker, type PickedPlace } from '@/features/settings/PlacePicker';
import type { AdoptPlan, NativeSession } from '@/generated/api-types';
import { newActKey } from '@/lib/requests';
import { Button } from '@/ui/Button';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';
import { toast } from '@/ui/Toast';
import { ACTIONS } from '@/ui/actions';

export interface AdoptDialogProps {
  session: NativeSession;
  /** The provider whose harness holds it, and the agent profile of that harness. */
  providerId: string;
  agentId: string;
  onClose: () => void;
}

/** The name a session goes by where it has none of its own. */
export function sessionName(session: NativeSession, untitled: string): string {
  const title = session.title?.trim() ?? '';
  return title === '' ? untitled : title;
}

/** What the plan says, as the rows of the facts it is shown with. */
function PlanFacts({ plan }: { plan: AdoptPlan }) {
  const { t } = useTranslation();
  const where =
    plan.environment !== null
      ? t('adopt.dialog.inEnvironment', {
          environment: plan.environment.name,
          directory: plan.directory === null || plan.directory === '' ? t('adopt.dialog.root') : plan.directory,
        })
      : plan.register_first !== null
        ? t('adopt.dialog.registerFirst', { folder: plan.register_first })
        : t('adopt.dialog.unknownFolder');
  const model =
    plan.model !== null
      ? t('adopt.dialog.modelTaken', { model: plan.model })
      : t(`adopt.dialog.modelReason.${plan.model_reason ?? 'not_reported'}`, { model: plan.session_model ?? '' });
  const history =
    plan.history_missing !== null
      ? t(`adopt.dialog.historyMissing.${plan.history_missing}`)
      : plan.turns === 0
        ? t('adopt.dialog.historyNone')
        : t('adopt.dialog.historyTurns', { count: plan.turns });
  return (
    <Facts
      label={t('adopt.dialog.plan')}
      rows={[
        { name: t('adopt.dialog.name'), value: plan.name },
        { name: t('adopt.dialog.where'), value: where },
        { name: t('adopt.dialog.model'), value: model },
        { name: t('adopt.dialog.history'), value: history },
      ]}
    />
  );
}

export function AdoptDialog({ session, providerId, agentId, onClose }: AdoptDialogProps) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const [topic, setTopic] = useState<PickedPlace | null>(null);
  const key = useRef(newActKey());
  const body = { provider_id: providerId, agent_id: agentId, native_session_ref: session.native_session_ref };
  const plan = useQuery({ ...adoptionPlanQuery(topic?.id ?? '', body), enabled: topic !== null });
  const adopt = useMutation({
    mutationFn: (topicId: string) => adoptSession(topicId, body, key.current),
    onSuccess: (thread) => {
      toast({
        tone: 'success',
        title: t('adopt.dialog.done', { name: thread.name }),
        action: { label: t(ACTIONS.openThread.label), icon: <ACTIONS.openThread.Icon aria-hidden />, run: () => void navigate({ to: placePath({ id: thread.id, kind: 'thread' }) }) },
      });
      onClose();
    },
  });
  const planned = plan.data;
  const already = planned?.adopted_thread_id ?? session.adopted_thread_id;
  const ready = topic !== null && planned !== undefined && planned.environment !== null && already === null && !plan.isFetching;
  const name = sessionName(session, t('adopt.untitled'));

  const footer = (
    <>
      <Button variant="ghost" onClick={onClose} disabled={adopt.isPending}>
        {t('common.cancel')}
      </Button>
      <Button
        icon={<ACTIONS.adoptSession.Icon aria-hidden />}
        pending={adopt.isPending}
        disabled={!ready}
        onClick={() => {
          if (topic !== null) adopt.mutate(topic.id);
        }}
      >
        {t('adopt.dialog.confirm')}
      </Button>
    </>
  );

  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open && !adopt.isPending) onClose();
      }}
      title={t('adopt.dialog.title', { name })}
      description={t('adopt.dialog.lead')}
      size="lg"
      dismissible={!adopt.isPending}
      footer={footer}
    >
      <div className="flex flex-col gap-4">
        {already !== null ? (
          <InlineAlert
            tone="info"
            title={t('adopt.dialog.already')}
            action={
              <Link to={placePath({ id: already, kind: 'thread' })} className="flex items-center gap-1 text-sm font-medium text-accent-text hover:underline">
                <ACTIONS.openThread.Icon aria-hidden /> {t(ACTIONS.openThread.label)}
              </Link>
            }
          />
        ) : (
          <PlacePicker
            label={t('adopt.dialog.topic')}
            hint={t('adopt.dialog.topicHint')}
            value={topic}
            only="topic"
            disabled={adopt.isPending}
            onChange={(chosen) => {
              key.current = newActKey();
              adopt.reset();
              setTopic(chosen);
            }}
          />
        )}
        {plan.isFetching && (
          <p role="status" className="text-sm text-text-muted">
            {t('adopt.dialog.planning')}
          </p>
        )}
        {plan.error !== null && <ErrorAlert error={plan.error} />}
        {planned !== undefined && !plan.isFetching && <PlanFacts plan={planned} />}
        {planned?.environment === null && already === null && (
          <InlineAlert tone="warning">{t('adopt.dialog.registerHow')}</InlineAlert>
        )}
        {adopt.error !== null && <ErrorAlert error={adopt.error} />}
      </div>
    </Dialog>
  );
}
