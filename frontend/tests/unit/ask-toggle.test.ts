/** The rail's Ask button closes the Ask that is on screen, and closing Ask leaves no empty bar behind (C1122). */
import { afterEach, beforeAll, expect, it } from 'vitest';
import { askShown, closeAsk, openAsk, toggleAsk } from '@/features/ask/open';
import { registerAskView } from '@/features/ask/register';
import { allTabs, tabHidden } from '@/features/workbench/layout';
import { useWorkbench } from '@/features/workbench/store';
import { registerShippedViews } from '@/features/workbench/shippedViews';
import { clearViewOpenings, showViewOpening } from '@/features/workbench/viewOpening';

const askTab = () => allTabs(useWorkbench.getState().layout).find((tab) => tab.view === 'ask');
const settled = () => new Promise<void>((resolve) => { queueMicrotask(resolve); });
let shown: (() => void) | null = null;
/** What the board does when Ask's section, docked or floating, is drawn: its slot is keyed by the tab. */
async function drawAsk(): Promise<string> {
  const tab = askTab();
  expect(tab).toBeDefined();
  shown = showViewOpening(tab?.id ?? '');
  await settled();
  return tab?.id ?? '';
}
beforeAll(() => { registerShippedViews(() => null); registerAskView(); });
afterEach(() => { shown?.(); shown = null; closeAsk(); clearViewOpenings(); });

it('knows Ask is shown from the slot of its own tab', async () => {
  openAsk();
  expect(askShown()).toBe(false);
  await drawAsk();
  expect(askShown()).toBe(true);
});

it('closes a shown Ask from the button that opened it', async () => {
  toggleAsk();
  const id = await drawAsk();
  toggleAsk();
  expect(tabHidden(useWorkbench.getState().layout, id)).toBe(true);
});

it('closes the bar that held only Ask instead of showing it empty', async () => {
  openAsk();
  const id = await drawAsk();
  const bar = (['left', 'right'] as const).find((side) => useWorkbench.getState().layout[side].containers
    .some((container) => container.sections.some((section) => section.tab.id === id)));
  expect(bar).toBeDefined();
  const side = bar ?? 'right';
  const others = useWorkbench.getState().layout[side].containers.filter((container) => !container.sections.some((section) => section.tab.id === id));
  closeAsk();
  const after = useWorkbench.getState().layout[side];
  const active = after.containers.find((container) => container.name === after.active);
  if (others.some((container) => container.sections.some((section) => !section.hidden))) {
    expect(active?.sections.some((section) => !section.hidden)).toBe(true);
  } else {
    expect(after.open).toBe(false);
  }
});
