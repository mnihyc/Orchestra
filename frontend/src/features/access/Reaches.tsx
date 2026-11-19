/** What reaches into an environment: a thread's files, terminal, browsers and displays are its environment's, how many people look into it, what making the thread private leaves open; and, on an environment's page, who can reach it.
 *
 * A thread's directory is where it works and fences nothing: whoever looks into the environment it runs in reads
 * all of its workspace, the other threads' directories among it. So a thread's page says whose environment that is and
 * how many people look into it, with the way to its page, and making such a thread private says what stays open through
 * it. An environment's own page counts everybody who can reach into it now, and opens the list of whom and how.
 */
import { useQuery } from '@tanstack/react-query';
import type { TFunction } from 'i18next';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { type AccessTarget, environmentReachQuery } from '@/api/access';
import type { PermissionPage, ReachPerson, ReachThrough } from '@/generated/api-types';
import { Button } from '@/ui/Button';
import { Dialog } from '@/ui/Dialog';
import { ErrorState } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';
import { UserChip } from '@/ui/UserChip';

import { usePermissionName } from './names';

/** The sentence that says whose files, terminal, browsers and displays a thread's are, and how many look into them; nothing where the page has no environment. */
export function ReachesLine({ page, onOpen }: { page: PermissionPage; onOpen?: ((target: AccessTarget) => void) | undefined }) {
  const { t } = useTranslation();
  const reaches = page.reaches;
  if (reaches === null) return null;
  const environment: AccessTarget = { kind: 'environment', id: reaches.environment.id, name: reaches.environment.name };
  return (
    <p className="flex flex-wrap items-baseline gap-x-2 text-sm text-text-muted">
      <span>{t('access.reaches.environment', { name: reaches.environment.name, count: reaches.people })}</span>
      {onOpen !== undefined && (
        <Button size="sm" variant="ghost" onClick={() => onOpen(environment)}>
          {t('access.reaches.open', { name: reaches.environment.name })}
        </Button>
      )}
    </p>
  );
}

/** What making a thread private leaves open, in words; `null` where nothing is left open. */
export function privateWarning(t: TFunction, page: PermissionPage): string | null {
  const reaches = page.reaches;
  if (reaches === null || reaches.people === 0) return null;
  return t('access.reaches.privateWarning', { name: reaches.environment.name, count: reaches.people });
}

/** One way a person reaches into an environment, in words. */
function Way({ through }: { through: ReachThrough }) {
  const { t } = useTranslation();
  const permissionName = usePermissionName();
  if (through.kind === 'source_owner') return <span>{t('access.sourceOwner')}</span>;
  if (through.kind === 'owner') return <span>{t('access.reaches.way.owner')}</span>;
  if (through.kind === 'page') return <span>{permissionName(through.permission)}</span>;
  return <span>{t('access.reaches.way.thread', { name: through.thread.name ?? t('access.place.hidden') })}</span>;
}

/** How many of the threads a person talks to the agents of are named: a person who talks to the agents of a hundred threads is not listed a hundred lines long. */
const THREADS_NAMED = 3;

function Person({ person }: { person: ReachPerson }) {
  const { t } = useTranslation();
  // Each way is listed as the server gave it and never reordered; of the threads, the first few are named and the rest counted.
  let threads = 0;
  const named = person.through.filter((through) => through.kind !== 'thread' || ++threads <= THREADS_NAMED);
  const more = person.through.length - named.length;
  return (
    <li className="flex flex-wrap items-center gap-x-3 gap-y-1 py-2">
      <UserChip user={person.user} size={24} />
      <span className="flex min-w-0 flex-wrap gap-x-2 text-sm text-text-muted">
        {named.map((through, index) => (
          <span key={index} className="after:content-['·'] after:ml-2 last:after:content-none">
            <Way through={through} />
          </span>
        ))}
        {more > 0 && <span>{t('access.reaches.way.more', { count: more })}</span>}
      </span>
    </li>
  );
}

/** On an environment's page, one sentence about who talks to the agents that work there, and the count of everybody who reaches into it, which opens the list. */
export function EnvironmentReachLine({ target }: { target: AccessTarget }) {
  const { t } = useTranslation();
  const reach = useQuery(environmentReachQuery(target.id));
  const [open, setOpen] = useState(false);
  const count = reach.data === undefined ? null : reach.data.people.length + reach.data.hidden;
  return (
    <div className="flex flex-wrap items-baseline gap-x-2 text-sm text-text-muted">
      <span>{t('access.reaches.talking')}</span>
      {count !== null && (
        <Button size="sm" variant="ghost" onClick={() => setOpen(true)}>
          {t('access.reaches.whoReaches', { count })}
        </Button>
      )}
      {open && (
        <Dialog open onOpenChange={(next) => !next && setOpen(false)} size="md" title={t('access.reaches.whoTitle', { name: target.name })} description={t('access.reaches.whoLead')}>
          {reach.isPending ? (
            <SkeletonRows rows={3} />
          ) : reach.isError ? (
            <ErrorState compact error={reach.error} retrying={reach.isRefetching} onRetry={() => void reach.refetch()} />
          ) : (
            // The list may be longer than the dialog is tall: the region takes the keyboard, so that it can be read by the arrows.
            <div role="region" aria-label={t('access.reaches.whoTitle', { name: target.name })} tabIndex={0} className="flex flex-col gap-2 py-2">
              <ul className="flex flex-col divide-y divide-border">
                {reach.data.people.map((person) => (
                  <Person key={person.user.id} person={person} />
                ))}
              </ul>
              {reach.data.hidden > 0 && <p className="text-sm text-text-muted">{t('access.reaches.hidden', { count: reach.data.hidden })}</p>}
            </div>
          )}
        </Dialog>
      )}
    </div>
  );
}
