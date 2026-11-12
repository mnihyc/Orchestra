/** Plain summary text with the same wrapping in a paragraph or a compact result row. */
import { cx } from '@/ui/cx';

export function SummaryText({ text, inline = false, id, className }: { text: string | null | undefined; inline?: boolean; id?: string; className?: string }) {
  if (text === undefined || text === null || text.trim() === '') return null;
  const Tag = inline ? 'span' : 'p';
  return <Tag id={id} data-summary-text className={cx('block max-w-full text-sm whitespace-pre-line wrap-anywhere text-text-muted', className)}>{text}</Tag>;
}
