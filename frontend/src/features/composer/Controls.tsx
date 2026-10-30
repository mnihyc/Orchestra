/** The next turn's controls beside its message: attachments, binding, model, permission mode, context and delivery. */
import { useCompactControls } from '@/features/workbench/Narrow';
import { cx } from '@/ui/cx';
import { ChevronDown } from 'lucide-react';
import { type ReactNode, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { ApprovalModePicker } from '@/features/composer/ApprovalMode';
import { ModelEffort } from '@/features/composer/ModelEffort';
import { openChange } from '@/features/change/target';
import { SetUpThread } from '@/features/start/SetUpThread';
import { ContextRing } from '@/features/usage/context/ContextRing';
import type { Thread } from '@/generated/api-types';
import { holds } from '@/lib/permissions';
import { Button } from '@/ui/Button';
import { Tooltip } from '@/ui/Tooltip';

export interface ComposerControlsProps {
  thread: Thread;
  readOnly?: boolean;
  attachment: ReactNode;
  model?: ReactNode;
  notices?: ReactNode;
  delivery?: ReactNode;
  /** The Plan switch, where the thread's agent program can plan. */
  plan?: ReactNode;
}

/** Compact controls wrap within their pane; full values stay on focus/hover and delivery stays together. */
export function ComposerControls({ thread, readOnly = false, attachment, model, notices, delivery, plan }: ComposerControlsProps) {
  const compact = useCompactControls();
  const { t } = useTranslation();
  const [settingUp, setSettingUp] = useState(false);
  const agent = thread.binding?.agent.name ?? t('composer.row.unbound');
  const canBind = !readOnly && !thread.archived && thread.side_of === null && thread.lock === null && holds(thread.access, 'thread.manage');
  const bindingReason = readOnly ? t('composer.row.readOnly') : !holds(thread.access, 'thread.manage') ? t('composer.row.bindingReadOnly', { who: thread.owner.display_name }) : thread.lock !== null ? t('threadMove.jobMustEnd') : undefined;
  if (readOnly) return <ReadOnlyComposerControls />;
  return <><div role="group" aria-label={t('composer.row.label')} className={cx('@container/composer flex min-w-0 flex-wrap items-center', compact ? 'gap-1' : 'gap-2')} data-composer-controls>
    <div className="flex min-w-0 flex-[1_1_auto] items-center gap-1">
      <span className="shrink-0" data-composer-control="attach">{attachment}</span>
      <Tooltip content={bindingReason === undefined ? agent : `${agent}: ${bindingReason}`}><button type="button" aria-label={t('composer.row.agent', { name: agent })} aria-disabled={!canBind}
        className={cx('flex min-h-(--control-sm) min-w-0 max-w-full items-center gap-1 rounded-md px-1 py-1 text-text-muted hover:bg-bg-muted', compact ? 'text-xs' : 'text-sm')} data-composer-control="agent"
        onClick={() => { if (canBind) { if (thread.binding === null) setSettingUp(true); else openChange({ kind: 'thread', id: thread.id, name: thread.name }, { kind: 'switch', focus: 'provider' }); } }}>
        <span className="min-w-0 truncate">{agent}</span><ChevronDown aria-hidden className={cx('size-4 shrink-0', compact && 'hidden')} />
      </button></Tooltip>
      <span className="min-w-0 max-w-(--chip-width-wide) flex-[1_1_auto]" data-composer-control="model">{model ?? <ModelEffort thread={thread} variant="composer" compact readOnly={readOnly} />}</span>
    </div>
    <span className="shrink-0" data-composer-control="mode"><ApprovalModePicker thread={thread} readOnly={readOnly} /></span>
    {plan !== undefined && <span className="shrink-0" data-composer-control="plan">{plan}</span>}
    <div className={cx('ml-auto flex shrink-0 items-center', compact ? 'gap-1' : 'gap-2')} data-composer-delivery>
      {notices}
      <span className="@max-sm/composer:[&_[data-context-ring]>span]:hidden [&_[data-context-ring]]:min-h-(--control-sm)" data-composer-control="context"><ContextRing thread={thread} side="top" /></span>
      <span data-composer-control="send">{delivery ?? <Button size="sm" variant="primary" disabled>{t(thread.state === 'running' || thread.state === 'waiting_approval' ? 'composer.stop' : 'composer.send')}</Button>}</span>
    </div>
  </div>{settingUp && <SetUpThread thread={thread} onClose={() => setSettingUp(false)} />}</>;
}

/** A watch-only reader gets the notice rather than a disabled set of driving controls. */
export function ReadOnlyComposerControls() {
  const { t } = useTranslation();
  return <p role="status" className="text-sm text-text-muted">{t('composer.row.readOnly')}</p>;
}
