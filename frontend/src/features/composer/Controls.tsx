/** The next turn's controls beside its message: attachments, binding, model, permission mode, context and delivery. */
import { useCompactControls } from '@/features/workbench/Narrow';
import { cx } from '@/ui/cx';
import { ChevronDown } from 'lucide-react';
import { type ReactNode, useId, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { ApprovalModePicker, ConfinementPicker } from '@/features/composer/ApprovalMode';
import { ModelEffort } from '@/features/composer/ModelEffort';
import { openChange } from '@/features/change/target';
import { ResourceChip } from '@/features/resource-facts/ResourceFacts';
import { SetUpThread } from '@/features/start/SetUpThread';
import { ContextRing } from '@/features/usage/context/ContextRing';
import type { Thread } from '@/generated/api-types';
import { holds } from '@/lib/permissions';
import { Button } from '@/ui/Button';
import { Tooltip } from '@/ui/Tooltip';
import { ComposerControlRow } from './ComposerFrame';

export interface ComposerControlsProps {
  thread: Thread;
  /** An explicitly read-only inspection; a live viewer instead keeps the native controls disabled. */
  readOnly?: boolean;
  attachment: ReactNode;
  model?: ReactNode;
  notices?: ReactNode;
  delivery?: ReactNode;
  /** The Plan switch, where the thread's agent program can plan. */
  plan?: ReactNode;
  /** One compact missing-access line inside the same controls row. */
  access?: ReactNode;
}

/** Compact controls wrap within their pane; full values stay on focus/hover and delivery stays together. */
export function ComposerControls({ thread, readOnly = false, attachment, model, notices, delivery, plan, access }: ComposerControlsProps) {
  const compact = useCompactControls();
  const { t } = useTranslation();
  const [settingUp, setSettingUp] = useState(false);
  const mayDrive = holds(thread.access, 'thread.interact');
  const agent = thread.binding?.agent.name ?? t('composer.row.unbound');
  const provider = thread.binding?.provider;
  const bindingWords = provider === undefined ? agent : `${agent} · ${provider.name}`;
  const bindingHelp = useId();
  const canBind = !readOnly && mayDrive && !thread.archived && thread.side_of === null && thread.lock === null && holds(thread.access, 'thread.manage');
  const bindingReason = readOnly || !mayDrive ? t('composer.row.readOnly') : thread.archived ? t('composer.row.archived')
    : thread.side_of !== null ? t('composer.row.sideBinding') : !holds(thread.access, 'thread.manage') ? t('composer.row.bindingReadOnly', { who: thread.owner.display_name }) : thread.lock !== null ? t('threadMove.jobMustEnd') : undefined;
  const bindingDescription = [provider === undefined ? null : t('composer.row.provider', { name: provider.name }), bindingReason].filter((part) => typeof part === 'string').join(' · ');
  if (readOnly) return <ReadOnlyComposerControls />;
  if (thread.program?.id === 'orch') return <ComposerControlRow label={t('composer.row.label')}>
    {access !== undefined && <div className="min-w-0 basis-full">{access}</div>}
    <span className="inline-flex shrink-0 self-start" data-composer-control="attach">{attachment}</span>
    <Tooltip content={t('ask.threadProgramDescription')}><span tabIndex={0} className="min-w-0 text-sm text-text-muted" data-composer-control="program">{t('ask.threadProgram')}</span></Tooltip>
    {model}
    <div className="ml-auto flex shrink-0 items-center gap-2" data-composer-delivery>{notices}<span data-composer-control="send">{delivery}</span></div>
  </ComposerControlRow>;
  return <><ComposerControlRow label={t('composer.row.label')}>
    {access !== undefined && <div className="min-w-0 basis-full">{access}</div>}
    <div className="flex min-w-0 flex-[1_1_auto] items-start gap-1 @max-sm/composer:basis-full @max-sm/composer:flex-wrap">
      <span className="inline-flex shrink-0 self-start" data-composer-control="attach">{attachment}</span>
      <Tooltip content={bindingReason === undefined ? bindingWords : `${bindingWords}: ${bindingReason}`}><button type="button" aria-label={t('composer.row.agent', { name: agent })} aria-describedby={bindingDescription === '' ? undefined : bindingHelp} aria-disabled={!canBind}
        className={cx('flex min-h-(--control-sm) min-w-0 max-w-full items-center gap-1 rounded-md px-1 py-0 leading-(--control-sm) text-text-muted hover:bg-bg-muted @max-sm/composer:flex-1 @max-sm/composer:flex-wrap', compact ? 'text-xs' : 'text-sm')} data-composer-control="agent"
        onClick={() => { if (canBind) { if (thread.binding === null) setSettingUp(true); else openChange({ kind: 'thread', id: thread.id, name: thread.name }, { kind: 'switch', focus: 'provider' }); } }}>
        <span className="min-w-0 truncate @max-sm/composer:basis-full @max-sm/composer:text-left">{agent}</span>
        {provider !== undefined && <ResourceChip subject={{ kind: 'provider', provider }} card={false} hint={false} className="min-w-0 max-w-(--chip-width) text-inherit @max-sm/composer:max-w-full" />}
        {bindingDescription !== '' && <span id={bindingHelp} className="sr-only">{bindingDescription}</span>}
        <ChevronDown aria-hidden className={cx('size-4 shrink-0', compact && 'hidden')} />
      </button></Tooltip>
      <span className="min-w-0 max-w-(--chip-width-wide) flex-[1_1_auto] @max-sm/composer:basis-full @max-sm/composer:max-w-full" data-composer-control="model">{model ?? <ModelEffort thread={thread} variant="composer" compact readOnly={!mayDrive} />}</span>
    </div>
    <span className="min-w-0 max-w-full" data-composer-control="mode"><ApprovalModePicker thread={thread} readOnly={!mayDrive} /></span>
    <span className="min-w-0 max-w-full" data-composer-control="sandbox"><ConfinementPicker thread={thread} readOnly={!mayDrive} /></span>
    {plan !== undefined && <span className="shrink-0" data-composer-control="plan">{plan}</span>}
    <div className={cx('ml-auto flex shrink-0 items-center', compact ? 'gap-1' : 'gap-2')} data-composer-delivery>
      {notices}
      <span className="@max-sm/composer:[&_[data-context-ring]>span]:hidden [&_[data-context-ring]]:min-h-(--control-sm)" data-composer-control="context"><ContextRing thread={thread} side="top" /></span>
      <span data-composer-control="send">{delivery ?? <Button size="sm" variant="primary" disabled>{t(thread.state === 'running' || thread.state === 'waiting_approval' ? 'composer.stop' : 'composer.send')}</Button>}</span>
    </div>
  </ComposerControlRow>{settingUp && canBind && <SetUpThread thread={thread} onClose={() => setSettingUp(false)} />}</>;
}

/** An explicitly read-only inspection keeps its existing notice. */
export function ReadOnlyComposerControls() {
  const { t } = useTranslation();
  return <p role="status" className="text-sm text-text-muted">{t('composer.row.readOnly')}</p>;
}
