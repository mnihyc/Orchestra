/** API client for a workspace's own harness configuration: the answer given or changed for one harness by whoever
 * manages the environment. What each harness found, and the answer given, are the environment's `config_trust`.
 */
import { api, ok } from '@/api/client';
import type { ConfigTrustAnswer, Environment } from '@/generated/api-types';

/** Answers, or changes the answer, for one harness; the environment as it then stands. */
export function setConfigTrust(environmentId: string, harness: string, answer: ConfigTrustAnswer): Promise<Environment> {
  return ok(
    api.PUT('/environments/{id}/config-trust/{harness}', {
      params: { path: { id: environmentId, harness } },
      body: { answer },
    }),
  );
}
