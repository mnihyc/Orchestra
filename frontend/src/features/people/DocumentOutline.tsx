/** Live document headings as a movable Outline, with real heading jumps and no inserted content. */
import { ListTree } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { buildTree, plainText, preOrder } from '@/features/documents/blocks';
import { client } from '@/api/client';
import { writeViewChoice } from '@/lib/viewChoices';
import { useShape } from '@/features/workbench/Narrow';
import { revealDocumentBlock, useDocumentMarksScope, useDocumentMarksStore } from '@/features/documents/marks';
import { openDocumentBlock } from '@/features/documents/blockNavigation';
import { useViewInstance } from '@/features/workbench/instance';
import { findTab } from '@/features/workbench/layout';
import { contentAddress, contentTarget } from '@/features/workbench/contentAddress';
import { useWorkbench } from '@/features/workbench/store';
import { reloadDocument } from '@/realtime/live';
import { documentStateNow, useDocumentState } from '@/realtime/stores/document';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorState } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';

export function DocumentOutline({ documentId }: { documentId: string }) {
  const { t } = useTranslation();
  const state = useDocumentState(documentId);
  const phone = useShape() === 'small';
  const instance = useViewInstance(), scope = useDocumentMarksScope(), marks = useDocumentMarksStore();
  if (state.ended !== null) return <EmptyState compact icon={<ListTree aria-hidden />} title={t(state.ended === 'deleted' ? 'documents.page.endedDeleted' : 'documents.page.endedRevoked')} />;
  if (state.error !== null) return <ErrorState compact error={state.error} onRetry={() => reloadDocument(documentId)} />;
  if (!state.loaded) return <SkeletonRows rows={3} />;
  const headings = preOrder(buildTree(state.blocks.values())).flatMap((id) => { const block = state.blocks.get(id); return block?.type === 'heading' && !block.removed ? [block] : []; });
  const jump = (id: string) => {
    if (instance === null || scope?.documentId !== documentId) return;
    const workbench = useWorkbench.getState(), panel = findTab(workbench.layout, instance.id)?.tab;
    if (panel === undefined) return;
    const fixed = panel.scope?.mode === 'fixed';
    const owner = fixed ? undefined : scope.owner;
    const content = owner === undefined ? undefined : findTab(workbench.layout, owner)?.tab;
    const address = content === undefined ? null : contentAddress(content);
    const current = () => {
      const now = useWorkbench.getState(), held = findTab(now.layout, instance.id)?.tab;
      const live = documentStateNow(documentId);
      if (held === undefined || !live.loaded || live.document === null || live.error !== null || live.ended !== null) return false;
      if (fixed) return held.scope?.mode === 'fixed' && held.scope.target.kind === 'document' && held.scope.target.id === documentId;
      if (held.scope?.mode === 'fixed' || now.activeContent !== owner) return false;
      const content = findTab(now.layout, owner)?.tab, target = content === undefined ? null : contentTarget(content);
      return target?.kind === 'document' && target.id === documentId && content !== undefined && contentAddress(content) === address;
    };
    if (!current()) return;
    if (!(owner !== undefined && revealDocumentBlock(id, marks, current)) && openDocumentBlock(documentId, id, owner) === null) return;
    useWorkbench.getState().setOverlay(null);
    if (phone) writeViewChoice(client.session.getState().userId, 'global', 'mobileView', '');
  };
  return headings.length === 0 ? <EmptyState compact icon={<ListTree aria-hidden />} title={t('people.noHeadings')} /> : <nav aria-label={t('people.outline')} className="h-full overflow-y-auto p-2"><ol className="flex flex-col gap-1">{headings.map((heading) => {
    const level = 'level' in heading.props ? heading.props.level : 1;
    return <li key={heading.id}><button type="button" className="focus-inset flex w-full items-start gap-2 rounded-md px-2 py-2 text-left text-sm text-text hover:bg-bg-muted" style={{ paddingInlineStart: `${level * 0.5}rem` }} onClick={() => jump(heading.id)}><span className="sr-only">{t('people.headingLevel', { level })}: </span><span className="break-words">{plainText(heading.content) || t('people.headingLevel', { level })}</span></button></li>;
  })}</ol></nav>;
}
