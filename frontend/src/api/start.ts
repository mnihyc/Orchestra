/** API client for what the dialog that starts a thread chooses among, beyond the lists every screen of resources reads: the environments the person manages, and a topic's default environment.
 *
 * What a thread made in a place could be bound to is asked of the server before the thread exists, once
 * for the dialog and the inline starter (`startOptionsQuery`): the providers with the reason one cannot be
 * used, their profiles and models, every arrangement with whether it can be had, the environments the person
 * manages and whether they may make one. The lists the screens of resources read (`api/resources`) only add
 * what is drawn beside a choice, never the choice. A default that is missing is mended by writing the setting
 * (`setStartDefault`) where the person may. The directory of a connected machine becomes an environment when it
 * is registered (`registerEnvironment` of `api/environments`), which a thread then joins.
 */
import { queryOptions } from '@tanstack/react-query';

import { environmentsKey } from '@/api/environments';
import { api, ok } from '@/api/client';
import { resourceRows } from '@/api/listed';
import type { BindingOptions, Environment, ResolvedSetting, Topic } from '@/generated/api-types';

/** What a thread of a place could be bound to, as `GET /nodes/{id}/binding/options` answers. */
export type StartOptions = BindingOptions;

/** The environments the person manages: the ones a thread can join. */
export const managedEnvironmentsQuery = queryOptions({
  queryKey: [...environmentsKey, 'manage'] as const,
  queryFn: ({ signal }): Promise<Environment[]> => ok(api.GET('/environments', { params: { query: { holding: 'manage' } }, signal })).then(resourceRows<Environment>),
});

export const startOptionsKey = ['start-options'] as const;

/**
 * What a thread of a topic, or of a topic made in a project, could be bound to. The arrangements are judged against
 * the environment named, or against the one the server would take when none is.
 */
export const startOptionsQuery = (nodeId: string, environmentId: string | null) =>
  queryOptions({
    queryKey: [...startOptionsKey, nodeId, environmentId ?? ''] as const,
    queryFn: ({ signal }): Promise<StartOptions> =>
      ok(
        api.GET('/nodes/{id}/binding/options', {
          signal,
          params: {
            path: { id: nodeId },
            query: environmentId === 'new'
              ? { environment_new: true }
              : { environment_new: false, ...(environmentId === null ? {} : { environment_id: environmentId }) },
          },
        }),
      ),
  });

/** Tells the server that a topic's default environment is to be this one: for the person who manages the topic. */
export function setDefaultEnvironment(topicId: string, environmentId: string | null): Promise<Topic> {
  return ok(api.PUT('/topics/{id}/default-environment', { params: { path: { id: topicId } }, body: { environment_id: environmentId } }));
}

/** Writes one setting at a place, as the default for what is made there: a provider, an agent profile, a model, an effort. */
export function setStartDefault(nodeId: string, key: string, value: unknown): Promise<ResolvedSetting> {
  return ok(api.PUT('/nodes/{id}/settings/{key}', { params: { path: { id: nodeId, key } }, body: { value } }));
}
