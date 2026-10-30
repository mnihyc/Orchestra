/** One quiet saved mark and the inverse of the last confirmed settings change. */
import { LoaderCircle, RotateCcw } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { Button } from '@/ui/Button';
import { Tooltip } from '@/ui/Tooltip';

export function SaveStatus({ state, onUndo, undoDisabled = false, name }: {
  state: 'saving' | 'saved' | 'undone' | null;
  onUndo?: (() => void) | undefined;
  undoDisabled?: boolean;
  name?: string | undefined;
}) {
  const { t } = useTranslation();
  if (state === null) return null;
  return <div role="status" className="flex min-h-(--control-sm) flex-wrap items-center gap-2 text-xs text-text-muted">
    {state === 'saving' && <LoaderCircle className="size-3 animate-spin" aria-hidden />}
    <span>{t(state === 'saving' ? 'settings.auto.saving' : state === 'saved' ? 'settings.compact.saved' : 'settings.compact.undone')}</span>
    {state === 'saved' && onUndo !== undefined && <Tooltip content={undoDisabled ? t('settings.compact.undoStale') : undefined}>
      <Button size="sm" variant="ghost" icon={<RotateCcw aria-hidden />} aria-label={name === undefined ? undefined : t('settings.compact.undoNamed', { name })} blocked={undoDisabled} onClick={onUndo}>{t('settings.compact.undo')}</Button>
    </Tooltip>}
  </div>;
}
