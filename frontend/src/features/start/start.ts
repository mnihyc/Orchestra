/** Starts the exact named setup with one repeatable creation request. The native action admits the thread,
 * its first input, any new environment and a project's new topic together, before provisioning can begin.
 * Optional sharing runs after acceptance; retrying it reuses the accepted thread.
 */
import { useMutation } from '@tanstack/react-query';
import { useNavigate } from '@tanstack/react-router';
import { useRef, useState } from 'react';

import { ApiError, client } from '@/api/client';
import { permissionsQuery, readPage } from '@/api/access';
import { queryClient } from '@/api/query';
import { createTopicAndThread } from '@/api/nodes';
import { createThread } from '@/api/threads';
import { letThreadViewersIn } from '@/features/access/doors';
import { generalAccessChange, type GeneralAccessPreset, indexCatalogue } from '@/features/access/model';
import type { PlaceRef } from '@/features/navigation/places';
import type { CreateThread, Thread } from '@/generated/api-types';
import { newActKey } from '@/lib/requests';
import { i18n } from '@/i18n';

import type { Resolved } from './choices';
import { rememberSetup } from './lastSetup';
import { checkStart } from './admission';
import { startOptionsKey, type StartOptions } from '@/api/start';

/** Which request of the act is under way. */
export type Step = 'check' | 'thread';

/** What the accepted act made, retained if an optional sharing change is refused. */
export interface Standing {
  topic: { id: string; name: string } | null;
  environment: { id: string; name: string } | null;
}

export interface StartPlan {
  /** Where the thread is made: a topic, or a project, which gets a topic first. */
  place: PlaceRef;
  options: StartOptions;
  resolved: Resolved;
  name: string;
  directory: string;
  /** Override missing-directory repository preparation, consistently on every machine. */
  prepareSource?: boolean;
  /** A newly made environment expires with this thread or its idle limit. */
  temporary?: boolean;
  privately: boolean;
  generalAccess?: GeneralAccessPreset;
  letIntoEnvironment?: boolean;
  /** What was typed: the first message. */
  message: string;
}

export interface Starting {
  start: (plan: StartPlan) => void;
  pending: boolean;
  step: Step | null;
  error: Error | null;
  standing: Standing;
  /** Forgets what was made and what went wrong: for a dialog that was closed or a composer that was cleared. */
  reset: () => void;
}

/** Makes what a thread needs and then the thread, and opens it. `onStarted` is told once the thread exists. */
export function useStart(onStarted: (thread: Thread) => void): Starting {
  const navigate = useNavigate();
  const keys = useRef(new Map<string, string>());
  const firstInput = useRef(newActKey());
  const [step, setStep] = useState<Step | null>(null);
  const [standing, setStanding] = useState<Standing>({ topic: null, environment: null });
  const made = useRef<Standing>({ topic: null, environment: null });
  const accepted = useRef<{ intent: string; thread: Thread } | null>(null);

  /** The key of one request: the same for the same content, which is what makes trying again safe. */
  const keyOf = (content: unknown): string => {
    const text = JSON.stringify(content);
    const held = keys.current.get(text);
    if (held !== undefined) return held;
    const fresh = newActKey();
    keys.current.set(text, fresh);
    return fresh;
  };

  const run = useMutation({
    mutationFn: async (plan: StartPlan): Promise<Thread> => {
      const person = client.session.getState().userId;
      const { place, resolved } = plan;
      if (resolved.provider === null || resolved.agent === null || resolved.model === null || resolved.arrangement === null) {
        throw new Error('the choices are not whole');
      }
      // Freeze the values named by the visible line before topic/environment requests can update caches.
      const chosen = { agent: resolved.agent.id, provider: resolved.provider.id, model: resolved.model.model_name,
        arrangement: resolved.arrangement, effort: resolved.effort,
        environment: resolved.environment === null ? null : { id: resolved.environment.environment.id, name: resolved.environment.environment.name } };
      const intent = JSON.stringify({ place: place.id, provider: chosen.provider, agent: chosen.agent, model: chosen.model,
        effort: chosen.effort, arrangement: chosen.arrangement, environment: chosen.environment?.id ?? null,
        name: plan.name, directory: plan.directory, temporary: plan.temporary, prepareSource: plan.prepareSource,
        privately: plan.privately, generalAccess: plan.generalAccess, message: plan.message });
      let thread = accepted.current?.intent === intent ? accepted.current.thread : null;
      if (thread === null) {
        setStep('check');
        checkStart(plan.options, chosen);
        setStep('thread');
        const access = plan.generalAccess === undefined ? plan.privately ? 'private' as const : undefined
          : plan.generalAccess === 'defaults' ? undefined
          : generalAccessChange(indexCatalogue(await queryClient.query(permissionsQuery)), 'thread', place.id, plan.generalAccess);
        const body: CreateThread = {
          ...(plan.name === '' ? {} : { name: plan.name }),
          binding: { agent_id: chosen.agent, provider_id: chosen.provider, arrangement: chosen.arrangement },
          model: chosen.model,
          ...(chosen.effort === null ? {} : { effort: chosen.effort }),
          environment: chosen.environment === null ? { new: { ...(plan.temporary === true ? { config: { 'env.lifetime': 'thread' } } : {}) } } : { use: chosen.environment.id },
          ...(plan.directory === '' ? {} : { directory: plan.directory }),
          ...(plan.prepareSource === undefined ? {} : { prepare_source: plan.prepareSource }),
          ...(access === undefined ? {} : { access }),
          ...(plan.message === '' ? {} : { first_input: { content: [{ type: 'text' as const, text: plan.message }], client_id: firstInput.current } }),
        };
        const key = keyOf({ step: 'thread', place: place.id, body });
        thread = place.kind === 'topic' ? await createThread(place.id, body, key) : await createTopicAndThread(place.id, body, key);
        accepted.current = { intent, thread };
      }
      const environment = thread.environment;
      if (environment === null) throw new Error(i18n.t('startPreflight.changed'));
      made.current = { ...made.current, environment: { id: environment.id, name: environment.name } };
      setStanding(made.current);
      if (plan.letIntoEnvironment === true) {
        const page = await readPage({ kind: 'thread', id: thread.id });
        if (!await letThreadViewersIn(page)) throw new Error(i18n.t('access.general.cannotLetIn', { name: environment.name }));
      }
      rememberSetup(person, { place: thread.topic_id, picks: { provider: chosen.provider, agent: chosen.agent, model: chosen.model, effort: chosen.effort, environment: environment.id, arrangement: chosen.arrangement } });
      return thread;
    },
    onSuccess: (thread) => {
      onStarted(thread);
      void navigate({ to: '/thread/$threadId', params: { threadId: thread.id } });
    },
    onError: (error, plan) => {
      // A current refusal refreshes only this chooser; a successful start needs no second preview request.
      if (error instanceof ApiError && error.status >= 400 && error.status < 500) {
        return queryClient.invalidateQueries({ queryKey: [...startOptionsKey, plan.place.id] });
      }
    },
    onSettled: () => setStep(null),
  });

  return {
    start: (plan) => run.mutate(plan),
    pending: run.isPending,
    step: run.isPending ? step : null,
    error: run.error,
    standing,
    reset: () => {
      run.reset();
      accepted.current = null;
      made.current = { topic: null, environment: null };
      setStanding(made.current);
      keys.current.clear();
      firstInput.current = newActKey();
    },
  };
}

