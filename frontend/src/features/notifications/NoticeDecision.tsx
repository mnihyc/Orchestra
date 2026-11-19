/** Only the own decision captured by the authorized turn/item projection is described. */
import { useTranslation } from 'react-i18next';
import type { NoticeDecision } from '@/generated/api-types';
import { FactDisclosure } from '@/ui/FactDisclosure';
import { RelativeTime } from '@/ui/RelativeTime';

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
