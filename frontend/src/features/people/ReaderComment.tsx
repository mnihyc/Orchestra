/** A native read-only text selection can start a discussion without ever becoming editable. */
import { MessageSquarePlus } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { createPortal } from 'react-dom';

import { useMarks } from '@/features/documents/marks';
import { Button } from '@/ui/Button';
import { FLOATING_GAP, floatingClasses, floatingPoint } from '@/ui/Popover';

/** The retained quotation uses the same length bound as the discussion request. */
const QUOTE_MAX = 400;
export function ReaderComment({ docId }: { docId: string }) {
  const { t } = useTranslation();
  const [selection, setSelection] = useState<{ block: string; quote: string; left: number; top: number } | null>(null);
  const saved = useRef(selection);
  useEffect(() => { saved.current = selection; }, [selection]);
  useEffect(() => {
    const select = () => {
      const current = document.getSelection();
      if (current === null || current.isCollapsed || current.rangeCount === 0) { setSelection(null); return; }
      const range = current.getRangeAt(0);
      const element = range.startContainer.nodeType === Node.ELEMENT_NODE ? range.startContainer as Element : range.startContainer.parentElement;
      const block = element?.closest<HTMLElement>('[data-id]');
      if (!block?.dataset.id || block.closest(`[data-document="${CSS.escape(docId)}"]`) === null || !block.contains(range.endContainer)) { setSelection(null); return; }
      const rect = range.getBoundingClientRect();
      setSelection({ block: block.dataset.id, quote: Array.from(current.toString()).slice(0, QUOTE_MAX).join(''), left: rect.left, top: rect.top - FLOATING_GAP });
    };
    const key = (event: KeyboardEvent) => {
      if ((event.metaKey || event.ctrlKey) && event.shiftKey && event.key.toLowerCase() === 'm' && saved.current !== null) {
        event.preventDefault(); useMarks.setState({ compose: { block: saved.current.block, quote: saved.current.quote } });
      }
    };
    document.addEventListener('selectionchange', select); document.addEventListener('keydown', key);
    document.addEventListener('scroll', select, true); window.addEventListener('resize', select);
    return () => { document.removeEventListener('selectionchange', select); document.removeEventListener('keydown', key); document.removeEventListener('scroll', select, true); window.removeEventListener('resize', select); };
  }, [docId]);
  if (selection === null) return null;
  return createPortal(<div role="toolbar" aria-label={t('people.readerSelection')} className={`fixed z-(--layer-dropdown) overflow-y-auto rounded-lg border border-border bg-surface-raised p-1 shadow-md ${floatingClasses}`} style={floatingPoint(selection.left, selection.top, true)}>
    <Button size="sm" icon={<MessageSquarePlus aria-hidden />} onMouseDown={(event) => event.preventDefault()} onClick={() => useMarks.setState({ compose: { block: selection.block, quote: selection.quote } })}>{t('documents.bar.comment')}</Button>
  </div>, document.body);
}
