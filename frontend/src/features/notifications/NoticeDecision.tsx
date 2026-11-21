/** Only the own decision captured by the authorized turn/item projection is described. */
import { useTranslation } from 'react-i18next';
import type { NoticeDecision } from '@/generated/api-types';
import { FactDisclosure } from '@/ui/FactDisclosure';
import { RelativeTime } from '@/ui/RelativeTime';

/** In ordinary conversation only a delivered Inbox notice or a missed explicit request gets a line. */
export function TurnNotice({ decision, diagnostics }: { decision: NoticeDecision | null | undefined; diagnostics: boolean }) {
  const { t } = useTranslation();
  if (diagnostics) return <NoticeDecisionFacts decision={decision} />;
  if (decision?.outcome === 'eligible') {
    return <p className="text-sm text-text-muted">{t('placesMail.sentToInbox')}</p>;
  }
  if (decision?.outcome === 'suppressed' && decision.requested_but_suppressed === true
      && decision.reason !== null && decision.reason !== 'already_watching') {
    return <p className="text-sm text-text-muted">{t('placesMail.suppressed', { reason: t(`placesMail.reasons.${decision.reason}`) })}</p>;
  }
  return null;
}

export function NoticeDecisionFacts({decision}:{decision?:NoticeDecision|null|undefined}) {
  const {t}=useTranslation();
  const fact=decision??undefined;
  const reason=fact?.reason===undefined||fact.reason===null?undefined:t(`placesMail.reasons.${fact.reason}`);
  return <FactDisclosure fragment={decision?.outcome==='suppressed'&&reason!==undefined?t('placesMail.suppressed',{reason}):undefined}
    groups={[{key:'notice',kind:'recorded_outcome',facts:[
      {key:'own',label:t('placesMail.forYou'),value:fact===undefined?undefined:t(`placesMail.outcomes.${fact.outcome}`)},
      {key:'at',label:t('placesMail.decision'),value:fact===undefined?undefined:<RelativeTime at={fact.evaluated_at}/>},
    ]}]}/>;
}
