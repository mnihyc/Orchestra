/** The next turn's controls beside its message: attachments, location, binding, model, effort, speed and access program, permission mode, context and delivery, each a compact control of its own. */
import { useCompactControls } from '@/features/workbench/Narrow';
import { cx } from '@/ui/cx';
import { type ReactNode, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { ApprovalModePicker, ConfinementPicker } from '@/features/composer/ApprovalMode';
import { forceLabel, type SpeedOrAccess } from '@/features/composer/modelState';
import { ModelEffort } from '@/features/composer/ModelEffort';
import { useRunsWith } from '@/features/composer/runsWith';
import { effortDisplayName, modelDisplayName } from '@/features/models/names';
import { ThreadAgentChip } from '@/features/composer/ThreadAgent';
import { WhereChip } from '@/features/thread/WhereChip';
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
  /** Where the box opens the context's list by a command of its own. */
  contextOpen?: boolean;
  onContextOpenChange?: (open: boolean) => void;
}

/** Compact controls wrap within their pane; full values stay on focus/hover and delivery stays together. */
export function ComposerControls({ thread, readOnly = false, attachment, model, notices, delivery, plan, access, contextOpen, onContextOpenChange }: ComposerControlsProps) {
  const compact = useCompactControls();
  const { t } = useTranslation();
  const [settingUp, setSettingUp] = useState(false);
  const mayDrive = holds(thread.access, 'thread.interact');
  const canBind = !readOnly && mayDrive && !thread.archived && thread.side_of === null && thread.lock === null && holds(thread.access, 'thread.manage');
  const bindingReason = readOnly || !mayDrive ? t('composer.row.readOnly') : thread.archived ? t('composer.row.archived')
    : thread.side_of !== null ? t('composer.row.sideBinding') : !holds(thread.access, 'thread.manage') ? t('composer.row.bindingReadOnly', { who: thread.owner.display_name }) : thread.lock !== null ? t('threadMove.jobMustEnd') : undefined;
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
      <span className="inline-flex min-w-0 max-w-full" data-composer-control="where"><WhereChip thread={thread} /></span>
      <span className="inline-flex min-w-0 max-w-full" data-composer-control="agent"><ThreadAgentChip thread={thread} canBind={canBind} reason={bindingReason} onSetUp={() => setSettingUp(true)} /></span>
      <span className="min-w-0 max-w-full flex-[1_1_auto] @max-sm/composer:basis-full" data-composer-control="model">{model ?? <ModelEffort thread={thread} variant="composer" compact readOnly={!mayDrive} />}</span>
    </div>
    <span className="min-w-0 max-w-full" data-composer-control="mode"><ApprovalModePicker thread={thread} readOnly={!mayDrive} sandbox={<ConfinementPicker thread={thread} readOnly={!mayDrive} />} /></span>
    {plan !== undefined && <span className="shrink-0" data-composer-control="plan">{plan}</span>}
    <div className={cx('ml-auto flex shrink-0 items-center', compact ? 'gap-1' : 'gap-2')} data-composer-delivery>
      {notices}
      <span className="@max-sm/composer:[&_[data-context-ring]>span]:hidden [&_[data-context-ring]]:min-h-(--control-sm)" data-composer-control="context"><ContextRing thread={thread} side="top" readOnly={!mayDrive} {...(contextOpen === undefined ? {} : { open: contextOpen })} {...(onContextOpenChange === undefined ? {} : { onOpenChange: onContextOpenChange })} /></span>
      <span data-composer-control="send">{delivery ?? <Button size="sm" variant="primary" disabled>{t(thread.state === 'running' || thread.state === 'waiting_approval' ? 'composer.stop' : 'composer.send')}</Button>}</span>
    </div>
  </ComposerControlRow>{settingUp && canBind && <SetUpThread thread={thread} onClose={() => setSettingUp(false)} />}</>;
}

/** An explicitly read-only inspection keeps its existing notice. */
export function ReadOnlyComposerControls() {
  const { t } = useTranslation();
  return <p role="status" className="text-sm text-text-muted">{t('composer.row.readOnly')}</p>;
}

/** One thing a thread runs with, in the words its control has, for a thread that is only looked at. It cannot be pressed. */
function StaticChip({ what, value, control, title }: { what: string; value: string; control: string; title?: string }) {
  const { t } = useTranslation();
  return <span className="inline-flex min-h-(--control-sm) max-w-(--chip-width) min-w-0 items-center truncate rounded-md px-2 text-sm text-text-muted" data-readonly-chip={control}
    title={title === undefined ? t('composer.chip.value', { what, value }) : `${t('composer.chip.value', { what, value })} · ${title}`}>{value}</span>;
}

/**
 * What a thread runs with, where it has no box of its own to show it: an archived thread, and one that is read at an earlier
 * point of its history. The same things in the same order as the box's controls (location, agent, model, effort, speed and
 * access program, then the context figure), none of them to be changed. The provider is the model's, told on hover.
 */
export function ThreadRunsLine({ thread }: { thread: Thread }) {
  const { t } = useTranslation();
  const runs = useRunsWith(thread, false);
  if (thread.binding === null) return null;
  const parameters = runs.parameters;
  const model = parameters?.model.options.find((option) => option.model_name === (parameters.model.value ?? thread.model));
  const effort = parameters?.effort.value ?? thread.effort;
  const speed = runs.states.find((state): state is SpeedOrAccess => state.kind === 'speed');
  const access = runs.states.find((state): state is SpeedOrAccess => state.kind === 'access');
  const speedWords = speed === undefined ? null : forceLabel(speed);
  const accessWords = access === undefined ? null : forceLabel(access);
  const name = thread.model === null ? '' : modelDisplayName(model?.display_name ?? thread.model_display);
  return <div role="group" aria-label={t('composer.row.runsWith')} className="flex min-w-0 flex-wrap items-center gap-1" data-runs-line>
    <WhereChip thread={thread} readOnly />
    <StaticChip what={t('composer.row.agentName')} value={thread.binding.agent.name} control="agent" />
    {name !== '' && <StaticChip what={t('composer.model.chip')} value={name} control="model" title={thread.binding.provider.name} />}
    {effort !== null && <StaticChip what={t('composer.effort.title')} value={effortDisplayName(effort, t)} control="effort" />}
    {speedWords !== null && <StaticChip what={t('composer.model.speed.title')} value={speedWords} control="speed" />}
    {accessWords !== null && <StaticChip what={t('composer.model.access.title')} value={accessWords} control="access" />}
    <span className="ml-auto" data-readonly-chip="context"><ContextRing thread={thread} side="top" readOnly /></span>
  </div>;
}
