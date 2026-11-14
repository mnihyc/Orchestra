/** One entry for the rail, shortcuts, finder commands and summary strips. */
import { allTabs } from '@/features/workbench/layout';
import { revealView } from '@/features/workbench/openView';
import { useWorkbench } from '@/features/workbench/store';
import { shownViewOpening } from '@/features/workbench/viewOpening';
import { clearAskDraftSeed, queueAskDraft } from './seed';

/** What was in focus when Ask was asked for: closing it gives the focus back there, or to the button that opens it. */
let opener: HTMLElement | null = null;
const rememberOpener = () => {
  const active = document.activeElement;
  opener = active instanceof HTMLElement && active !== document.body && active.closest('[data-view-window="ask"]') === null ? active : null;
};

function revealAsk(): void {
  rememberOpener();
  revealView('ask', {});
  requestAnimationFrame(() => document.querySelector<HTMLTextAreaElement>('[data-ask-composer] textarea')?.focus());
}

export function openAsk(): void { clearAskDraftSeed(); revealAsk(); }

/** Ask is shown, in a bar, in a floating window or in the phone's sheet. */
export const askShown = (): boolean => shownViewOpening('ask') !== null;

/** Closes Ask where it is: its section is hidden and its view is kept, with what is typed in it, for the next opening. */
export function closeAsk(): void {
  const state = useWorkbench.getState();
  const tab = allTabs(state.layout).find((candidate) => candidate.view === 'ask');
  if (tab !== undefined) state.close({ kind: 'tab', tab: tab.id });
  const back = opener?.isConnected === true ? opener : document.querySelector<HTMLElement>('[data-ask-opener]');
  opener = null;
  requestAnimationFrame(() => back?.focus());
}

/** The button of Ask in the rail and in the phone's title bar: pressed when Ask is shown, it closes it. */
export function toggleAsk(): void { if (askShown()) closeAsk(); else openAsk(); }

/** Visible context enters the same panel draft; private sources pin its actual root. */
export function openAskWithDraft(context: { text: string; root?: string }): void { queueAskDraft(context.text, context.root ?? null); revealAsk(); }

/** Administrative thread inspection can open the same panel at an explicit, server-authorized ID. */
export function openAskConversation(id: string): void { revealView('ask', { conversation: id, inspect: '1' }); }
