/** What the agents of a place know: what was shared with the agents of this place or of a place above it, each with where it comes from and who shared it.
 *
 * It is the same knowledge a thread's Knowledge panel shows as a tree. Here it is a list for the place: the
 * thing that was shared, how far (to read, to write), whose agents it was shared with, and by whom and when.
 * What the caller may not see themself comes without a name. Nothing here changes anything: a share is
 * changed and taken back on the page of what was shared.
 */
import { useQuery } from '@tanstack/react-query';
import { FileText, MessageSquare, Network } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { EmptyState } from '@/ui/EmptyState';

import type { AccessTarget } from '@/api/access';
import { agentSharesQuery } from '@/api/share';
import type { AgentShare } from '@/generated/api-types';
import { useAppearance } from '@/lib/appearance';
import { Skeleton } from '@/ui/Skeleton';

import { AgentPartsSummary, CurrentAgentContribution, ViewerReadStanding } from './AgentContribution';
import { shortDate } from './holders';

/** What was shared, by its name, and the mark of its kind. */
function whatOf(share: AgentShare): { name: string; Icon: typeof FileText } {
  const what = share.what as { name?: string | null; title?: string | null; kind?: string };
  const name = what.name ?? what.title ?? '';
  const Icon = what.kind === undefined ? (what.title === undefined ? MessageSquare : FileText) : Network;
  return { name, Icon };
}

export function KnownToAgents({ target }: { target: AccessTarget }) {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  const known = useQuery(agentSharesQuery(target, 'to'));

  return (
    <section aria-label={t('access.agents.know.title')} className="flex flex-col gap-2">
      <h3 className="text-sm font-medium text-text">{t('access.agents.know.title')}</h3>
      {known.isPending ? (
        <Skeleton className="h-10 w-full" />
      ) : known.isError ? (
        <p className="text-sm text-text-muted">{t('access.agents.know.notLoaded')}</p>
      ) : known.data.length === 0 ? (
        <EmptyState compact title={t('access.agents.know.none', { name: target.name })} />
      ) : (
        <ul className="flex flex-col divide-y divide-border rounded-lg border border-border">
          {known.data.map((share) => {
            const { name, Icon } = whatOf(share);
            const from = share.to.id === target.id ? null : (share.to.name ?? t('access.place.hidden'));
            return (
              <li key={share.id} className="flex flex-col gap-1 px-3 py-2 text-sm">
                <span className="flex min-w-0 items-center gap-2 text-text">
                  <Icon className="shrink-0 text-text-muted" size={16} aria-hidden />
                  <span className="min-w-0 font-medium wrap-anywhere">{name === '' ? t('access.agents.know.unseen') : name}</span>
                </span>
                <div className="text-text-muted">
                  {t('agentContributions.requested')}: <AgentPartsSummary parts={share.parts} />
                  {share.mode === 'snapshot' && <span className="ml-3">{t('access.agents.know.copy')}</span>}
                </div>
                {share.mode === 'live' && <CurrentAgentContribution current={share.current} />}
                <ViewerReadStanding entries={share.viewer_entries} />
                <span className="text-text-muted">
                  {from === null ? t('access.agents.know.here') : t('access.agents.know.from', { place: from })}{' '}{t('access.agents.by', { name: (share.by?.display_name ?? '').length > 0 ? share.by?.display_name : t('agentContributions.giverUnavailable'), date: shortDate(locale, share.at) })}
                </span>
              </li>
            );
          })}
        </ul>
      )}
    </section>
  );
}
