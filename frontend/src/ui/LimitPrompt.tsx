/** Shared LimitPrompt component (usage): what somebody reads when a limit is reached: each limit with what is used of how much, who can let work past it, and for those who can, Proceed anyway.
 *
 * Read when a message would go past a limit: each limit that is reached, with how much of it is used,
 * when it starts over, and who can let work past it. Somebody who may pass every limit in the way reads
 * what they pass and goes on; anybody else reads who could let them through, and may still send, in
 * which case the message waits as it would have. Nothing is passed unless the person says so here.
 */
import { useTranslation } from 'react-i18next';

import type { LimitBypassRequest, LimitReached, ThreadLimits } from '@/generated/api-types';
import { LimitLine as UsageLimitLine } from '@/features/usage/PassLimit';
import { ViewLimits } from '@/features/usage/ThreadUsage';
import { Button } from '@/ui/Button';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';

export interface LimitPromptProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  limits: ThreadLimits;
  /** Sends the message: past the limits the person may pass (`bypass`), or as it is, to wait. */
  onSend: (bypass: LimitBypassRequest | undefined) => void;
  pending: boolean;
  error?: unknown;
  threadId?: string;
}

function LimitLine({ limit }: { limit: LimitReached }) { return <UsageLimitLine limit={limit} />; }

/** What the person lets past: the caps and the provider's limit they hold the bypass row for. */
export function bypassOf(limits: ThreadLimits): LimitBypassRequest | undefined {
  const caps = limits.reached.flatMap((limit) => (limit.can_bypass && limit.cap !== null && limit.passed_by === null ? [limit.cap.cap.id] : []));
  const provider = limits.reached.some((limit) => limit.can_bypass && limit.provider_limit !== null && limit.passed_by === null);
  if (caps.length === 0 && !provider) return undefined;
  return { ...(caps.length > 0 ? { caps } : {}), ...(provider ? { provider_limit: true } : {}) };
}

export function LimitPrompt({ open, onOpenChange, limits, onSend, pending, error, threadId }: LimitPromptProps) {
  const { t } = useTranslation();
  const bypass = bypassOf(limits);
  const all = limits.bypass_all;
  return (
    <Dialog
      open={open}
      onOpenChange={onOpenChange}
      title={t('ui.limit.title')}
      description={bypass !== undefined ? t('ui.limit.aboutYouCan') : t('ui.limit.aboutWait')}
      dismissible={!pending}
      footer={
        <>
          <Button variant="ghost" disabled={pending} onClick={() => onOpenChange(false)}>
            {t('common.cancel')}
          </Button>
          {bypass === undefined || !all ? (
            <Button variant="secondary" pending={pending && bypass === undefined} disabled={pending} onClick={() => onSend(undefined)}>
              {t('ui.limit.sendAndWait')}
            </Button>
          ) : null}
          {bypass !== undefined && (
            <Button variant="primary" pending={pending} onClick={() => onSend(bypass)}>
              {t(all ? 'ui.limit.proceed' : 'caps.prompt.passMine')}
            </Button>
          )}
        </>
      }
    >
      <div className="flex flex-col gap-3">
        {threadId !== undefined && <ViewLimits thread={{ id:threadId,name:'',binding:null }} />}
        <ul className="flex flex-col gap-2">
          {limits.reached.map((limit, index) => (
            <LimitLine key={limit.cap?.cap.id ?? `provider-${index}`} limit={limit} />
          ))}
        </ul>
        {bypass !== undefined && !all && <InlineAlert tone="warning">{t('ui.limit.partial')}</InlineAlert>}
        {error !== undefined && error !== null && <ErrorAlert error={error} />}
      </div>
    </Dialog>
  );
}
