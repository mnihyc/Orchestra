/** People in cap captions, attribution and per-person shares use the shared person chip. */
import { Trans,useTranslation } from 'react-i18next';
import type { UsageCap,UserRef } from '@/generated/api-types';
import { UserChip } from '@/ui/UserChip';
import { capTitle,modelsWords,narrowingWords,whereWords, type ProgramTitle } from './words';

export function CapTitle({cap,programTitle}:{cap:UsageCap;programTitle?:ProgramTitle}) {
  const {t}=useTranslation();
  return cap.subject.kind==='user' ? <Trans i18nKey="caps.titlePerson" values={{models:[modelsWords(cap.models,t),...narrowingWords(cap,t,programTitle)].join(' '),where:whereWords(cap,t)}} components={{person:<UserChip user={cap.subject.user} />}} /> : <>{capTitle(cap,t,programTitle)}</>;
}

export function CapSetter({user}:{user:UserRef}) {
  return <span className="inline-flex flex-wrap items-center gap-1"><Trans i18nKey="caps.row.setBy" components={{person:<UserChip user={user} />}} /></span>;
}

export function CapShare({user}:{user:UserRef}) {
  return <span className="inline-flex flex-wrap items-center gap-1"><Trans i18nKey="caps.row.share" components={{person:<UserChip user={user} />}} /></span>;
}
