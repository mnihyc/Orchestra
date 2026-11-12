/** One entry for the rail, shortcuts, finder commands and summary strips. */
import { revealView } from '@/features/workbench/openView';
import { clearAskDraftSeed, queueAskDraft } from './seed';

function revealAsk(): void {
  revealView('ask', {});
  requestAnimationFrame(() => document.querySelector<HTMLTextAreaElement>('[data-ask-composer] textarea')?.focus());
}

export function openAsk(): void { clearAskDraftSeed(); revealAsk(); }

/** Visible context enters the same panel draft; private sources pin its actual root. */
export function openAskWithDraft(context: { text: string; root?: string }): void { queueAskDraft(context.text, context.root ?? null); revealAsk(); }

/** Administrative thread inspection can open the same panel at an explicit, server-authorized ID. */
export function openAskConversation(id: string): void { revealView('ask', { conversation: id, inspect: '1' }); }
