/** Shared version mark, refusal and Update now action for physical machines and nodes. */
import { useMutation } from '@tanstack/react-query';
import { Download } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { updateHostNow, updateMachineNow } from '@/api/resources';
import { ArticleGuideLink } from '@/features/help/ArticleGuideLink';
import type { Act } from '@/features/resources/acts';
import { CommandBox } from '@/features/resources/Facts';
import type { ComponentUpdate } from '@/generated/api-types';
import { Badge } from '@/ui/Badge';
import { Button } from '@/ui/Button';
import { InlineAlert } from '@/ui/InlineAlert';
import type { AdminActSpec } from '@/ui/AdminAct';
import { toast } from '@/ui/Toast';

export function updatePending(update: ComponentUpdate | null | undefined): boolean {
  return update?.status === 'queued' || update?.status === 'updating';
}

function version(value: string): string { return value.startsWith('v') ? value : `v${value}`; }

/** Names stay first in a row; this follows them at every width. */
export function UpdateMark({ update }: { update: ComponentUpdate | null | undefined }) {
  const { t } = useTranslation();
  if (update === null || update === undefined) return null;
  if (updatePending(update)) return <span role="status" className="text-sm text-text-muted">{t('componentUpdates.updating')}</span>;
  if (update.detail !== null && (update.status === 'refused' || update.status === 'failed')) {
    return <span role="status" className="text-sm wrap-anywhere text-text-muted">{update.detail}</span>;
  }
  return update.behind ? <Badge tone="warning">{t('componentUpdates.available')}</Badge> : null;
}

/** The page's version sentence, with the same action its row menu uses. */
export function UpdateNotice({ update, action }: { update: ComponentUpdate | null | undefined; action: Act | null }) {
  const { t } = useTranslation();
  if (update === null || update === undefined || (!update.behind && !updatePending(update))) return null;
  return <InlineAlert tone="warning" title={updatePending(update) ? t('componentUpdates.updating') : t('componentUpdates.versions', {
    recommended: version(update.recommended_version), running: version(update.running_version),
  })}>
    <div className="flex flex-col items-start gap-2">
      <p>{t('componentUpdates.idle')}</p>
      {update.detail !== null && <p role="status" className="wrap-anywhere">{update.detail}</p>}
      {action !== null && <Button size="sm" {...(action.admin === undefined ? {} : {admin:action.admin})} pending={action.pending ?? false} disabled={action.disabled !== false && action.disabled !== undefined} onClick={action.run}>{action.label}</Button>}
      {typeof action?.disabled === 'string' && <p>{action.disabled}</p>}
      {(update.status === 'refused' || update.status === 'failed' || (typeof action?.disabled === 'string' && !updatePending(update))) && <CommandBox command={update.command} label={t('componentUpdates.copyCommand')} />}
      <ArticleGuideLink article="updates" title={t('componentUpdates.guideTitle')} />
    </div>
  </InlineAlert>;
}

interface Target { kind: 'machine' | 'host'; id: string; update: ComponentUpdate | null | undefined; manage: boolean; online: boolean; admin?: AdminActSpec | undefined }

/** The page button and list menu resolve through one permission/idle/progress rule. */
export function useComponentUpdateAct(): (target: Target) => Act | null {
  const { t } = useTranslation();
  const request = useMutation({
    mutationFn: ({ kind, id }: Target) => kind === 'host' ? updateHostNow(id) : updateMachineNow(id),
    onError: () => toast({ tone: 'danger', title: t('componentUpdates.failed') }),
  });
  return (target) => {
    if (!target.manage || target.update === null || target.update === undefined || (!target.update.behind && !updatePending(target.update))) return null;
    const pending = updatePending(target.update) || (request.isPending && request.variables.id === target.id);
    return {
      ...(target.admin === undefined ? {} : {admin:target.admin}),
      id: 'update-now', label: t(pending ? 'componentUpdates.updating' : 'componentUpdates.action'), icon: <Download aria-hidden />,
      description: t('componentUpdates.idle'), disabled: pending ? t('componentUpdates.updating') : !target.online ? t('componentUpdates.offline') : false,
      pending, run: () => request.mutate(target),
    };
  };
}
