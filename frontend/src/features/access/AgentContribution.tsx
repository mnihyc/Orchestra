/** Requested ceilings and an exact contribution's assessed area rights are separate facts. */
import { useTranslation } from 'react-i18next';
import type { AgentShare, Entry, ShareParts } from '@/generated/api-types';
import { AREAS, stepOf } from './agentShares';
import { AREA_WORDS, stepWords } from './agentShareWords';

export function AgentPartsSummary({ parts }: { parts: ShareParts }) {
  const { t } = useTranslation();
  const areas = AREAS.filter((area) => stepOf(area, parts) > 0);
  return areas.length === 0 ? <span>{t('agentContributions.none')}</span>
    : <ul className="flex flex-wrap gap-x-3 gap-y-1">{areas.map((area) => <li key={area}>{t(AREA_WORDS[area])}: {stepWords(t, area, stepOf(area, parts))}</li>)}</ul>;
}

export function CurrentAgentContribution({ current, proposed = false }: {
  current: AgentShare['current']; proposed?: boolean;
}) {
  const { t } = useTranslation();
  return <div className="flex flex-col gap-1 text-sm">
    <p className="font-medium">{t(proposed ? 'agentContributions.proposed' : 'agentContributions.current')}</p>
    {current === undefined || current === null ? <p className="text-text-muted">{t('agentContributions.unassessed')}</p>
      : <>
        <p>{t(proposed ? `agentContributions.proposedState.${current.state}` : `agentContributions.state.${current.state}`)}</p>
        {current.parts !== null && <AgentPartsSummary parts={current.parts} />}
        {current.reason !== null && <p className="text-text-muted">{t(`agentContributions.reason.${current.reason}`)}</p>}
      </>}
    <p className="text-xs text-text-muted">{t('agentContributions.separate')}</p>
  </div>;
}

/** Ordinary viewer entries are stored grants, not an inference about every person's effective access. */
export function ViewerReadStanding({ entries }: { entries: readonly Entry[] | undefined }) {
  const { t } = useTranslation();
  if (entries === undefined || entries.length === 0) return null;
  return <p className="text-sm text-text-muted">{t('agentContributions.viewerStanding')}</p>;
}
