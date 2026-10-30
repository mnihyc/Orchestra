/** Read-only prompt-version navigation; the caller chooses a view, never an execution branch. */
import { ChevronLeft, ChevronRight } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { useId, type Ref } from 'react';
import { IconButton } from '@/ui/IconButton';

export interface PromptVersionsProps {
  current: number;
  total: number;
  pending?: boolean;
  groupId?: string;
  ref?: Ref<HTMLSpanElement>;
  onPrevious: () => void;
  onNext: () => void;
}

/** Only edited prompts have a version strip. It belongs beside their existing message actions. */
export function PromptVersions({ current, total, pending = false, groupId, ref, onPrevious, onNext }: PromptVersionsProps) {
  const { t } = useTranslation();
  const counter = useId();
  if (total < 2) return null;
  return <span ref={ref} data-prompt-group={groupId} role="group" aria-label={t('messageVersions.label')} aria-busy={pending || undefined}
    className="inline-flex shrink-0 items-center gap-0.5 text-xs text-text-muted">
    <IconButton size="sm" label={t('messageVersions.previous')} icon={<ChevronLeft aria-hidden />}
      aria-describedby={counter} disabled={pending || current <= 1} onClick={onPrevious} />
    <span id={counter} role="status" aria-live="polite" aria-atomic="true" className="tabular-nums">
      <span aria-hidden>{t('messageVersions.counter', { current, total })}</span>
      <span className="sr-only">{t('messageVersions.position', { current, total })}</span>
    </span>
    <IconButton size="sm" label={t('messageVersions.next')} icon={<ChevronRight aria-hidden />}
      aria-describedby={counter} disabled={pending || current >= total} onClick={onNext} />
  </span>;
}
