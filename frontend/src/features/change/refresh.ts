/** Asks again for what a job that is over may have changed: where environments are, what runs in them, which threads use which provider.
 *
 * A fork, a switch, an update or a move changes records that other screens hold: the page of the environment that was updated says it
 * is up to date only once it has been read again, and the host it moved to shows one more environment. The job knows when it is over;
 * this is the one place that says what to read again then, for each of the wizards.
 */
import { useQueryClient } from '@tanstack/react-query';
import { useEffect } from 'react';
import { harnessUsageQuery } from '@/api/harnesses';

/** The queries that hold what a change moves: environments and hosts, each list and each page, and what is said of providers' threads. */
const KEYS = [['environments'], ['environment'], ['hosts'], ['host'], ['providers'], ['provider'], ['provider-threads']] as const;

/** Reads again what the job may have changed, once, when it is over; `over` is whether the job (or the last of a batch) has ended. */
export function useRefreshWhenOver(over: boolean, jobId: string | undefined, environmentIds?: readonly string[]): void {
  const queryClient = useQueryClient();
  const programEnvironments = environmentIds === undefined ? null : JSON.stringify([...new Set(environmentIds)].sort());
  useEffect(() => {
    if (!over) return;
    for (const queryKey of KEYS) void queryClient.invalidateQueries({ queryKey });
    if (programEnvironments === null) return;
    void queryClient.invalidateQueries({ queryKey: harnessUsageQuery.queryKey, exact: true });
    const ids = JSON.parse(programEnvironments) as string[];
    for (const id of ids) {
      // The generation belongs to the reader's key. Refresh only this job's environment projections.
      for (const query of queryClient.getQueryCache().findAll({ queryKey: ['environment-programs', id] })) {
        void queryClient.invalidateQueries({ queryKey: query.queryKey, exact: true });
      }
    }
  }, [over, jobId, queryClient, programEnvironments]);
}
