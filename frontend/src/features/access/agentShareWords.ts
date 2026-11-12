/** One set of area names and nested level words for requested and currently assessed contributions. */
import type { TFunction } from 'i18next';
import { choiceOption } from '@/i18n/choices';
import type { Area } from './agentShares';

/** What a level is called in an area, in the words of the form: conversations and chats are read, or read and written; documents are read, read and edited, or managed. */
export function stepWords(t: TFunction, area: Area, step: number): string {
  if (step === 0) return t('access.agents.step.none');
  if (step === 1) return t('access.agents.step.read');
  if (step === 2) {
    if (area === 'documents') return t('access.agents.step.edit');
    return t(area === 'chat' ? 'access.agents.step.post' : 'access.agents.step.send');
  }
  return t('access.agents.step.manage');
}

/** Compact ladder names; detailed consequences stay in help. */
export function stepOption(t: TFunction, area: Area, step: number) {
  const word = step === 0 ? 'none' : step === 1 ? 'view' : step === 2 ? area === 'documents' ? 'edit' : area === 'chat' ? 'post' : 'send' : 'manage';
  return { name: choiceOption(t, `agentStep.${word}`).name };
}

/** The areas of a share, named as the columns of the form are. */
export const AREA_WORDS = { timeline: 'access.agents.area.timeline', chat: 'access.agents.area.chat', documents: 'access.agents.area.documents' } as const;
