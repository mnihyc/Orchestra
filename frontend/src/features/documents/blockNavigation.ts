/** Document block links use the normal content opener, retaining the exact selected pane and its version. */
import { allTabs, findTab } from '@/features/workbench/layout';
import { contentAddress, contentTarget, pageHref, parsePageAddress } from '@/features/workbench/contentAddress';
import { openContent } from '@/features/workbench/contentNavigation';
import { useWorkbench } from '@/features/workbench/store';
import { documentStateNow } from '@/realtime/stores/document';
import { documentMarksFor, revealDocumentBlock } from './marks';

/** A following inspector supplies its exact owner; a fixed resource may activate or open a matching page. */
export function openDocumentBlock(documentId: string, block: string, owner?: string): string | null {
  const readable = () => {
    const state = documentStateNow(documentId);
    return state.loaded && state.document !== null && state.error === null && state.ended === null;
  };
  if (!readable()) return null;
  const state = useWorkbench.getState();
  const matches = allTabs(state.layout).filter((tab) => { const target = contentTarget(tab); return target?.kind === 'document' && target.id === documentId; });
  const selected = owner === undefined ? matches.find((tab) => tab.id === state.activeContent) ?? matches[0] : matches.find((tab) => tab.id === owner);
  // A stale following callback must not recreate a closed pane or choose another duplicate.
  if (owner !== undefined && selected === undefined) return null;
  const before = selected === undefined ? null : contentAddress(selected);
  const target = parsePageAddress(before ?? `/doc/${encodeURIComponent(documentId)}`);
  if (target === null) return null;
  const href = pageHref({ ...target, search: { ...target.search, block } });
  if (selected !== undefined) state.focus(selected.id);
  const id = openContent(href, 'normal', 'replace');
  if (id === null) return null;
  // A changed/new block link is handled by DocumentPage readiness. Repeating the same link still scrolls.
  if (before === href) requestAnimationFrame(() => {
    const current = () => {
      const tab = findTab(useWorkbench.getState().layout, id)?.tab;
      return readable() && tab !== undefined && contentAddress(tab) === href;
    };
    if (current()) revealDocumentBlock(block, documentMarksFor(id, documentId), current);
  });
  return id;
}
