/** Best-effort plan windows, credits and age, and the tightest usage cap.
 */
import { ChartNoAxesColumn } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import type { Thread } from '@/generated/api-types';
import { useFormatContext } from '@/lib/appearance';
import { formatTokens } from '@/lib/format';
import { open as openView } from '@/features/workbench/layout';
import { useWorkbench } from '@/features/workbench/store';
import { Button } from '@/ui/Button';

export function UsageChip({ thread }: { thread: Thread }) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  const tokens = thread.usage?.total_tokens;
  const blocked = thread.state === 'blocked' ? thread.blocked?.reason : undefined;
  return <Button variant="ghost" size="sm" icon={<ChartNoAxesColumn aria-hidden />} onClick={() => {
    const workbench = useWorkbench.getState();
    workbench.change((layout) => openView(layout, 'usage', 'right'));
    workbench.setOverlay('right');
  }}>
    <span>{t('providerPlan.threadTitle')}</span>{tokens !== undefined && <span>{t('usage.board.tokens',{tokens:formatTokens(tokens, locale)})}</span>}
    {blocked === 'provider_exhausted' && <span>{t('providerPlan.recovery.limit')}</span>}
    {blocked === 'quota_exceeded' && <span>{t('providerPlan.limits')}</span>}
  </Button>;
}
