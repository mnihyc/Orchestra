/** The same exact Share areas, confirmed as a creation choice or edited as its inherited preset. */
import { useId, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import type { AgentAccessChoice, AgentAccessWhere, AgentLevel, ShareParts } from '@/generated/api-types';
import { choiceOption } from '@/i18n/choices';
import { Button } from '@/ui/Button';
import { Select } from '@/ui/Select';
import { ValueControl, type ValueControlProps } from '@/features/settings/ValueControl';
import { AREA_WORDS, stepOption } from './agentShareWords';
import { AREAS, LEVELS, creationAreas, creationChoice, levelOfParts, partsOfLevel, sharePartsValue, stepOf, stepsOf, withStep } from './agentShares';

export const AGENT_ACCESS_SETTING = 'threads.agent_access';
const REACHES: readonly AgentAccessWhere[] = ['nothing', 'thread', 'topic', 'project'];
const record = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value);

/** An invalid or newer value remains visible in the ordinary value control instead of being replaced with a grant. */
export function agentAccessValue(value: unknown): AgentAccessChoice | null {
  if (!record(value) || Object.keys(value).some((key) => key !== 'where' && key !== 'parts') || !record(value.parts)) return null;
  const where = REACHES.find((entry) => entry === value.where);
  if (where === undefined || where === 'nothing' && Object.keys(value.parts).length > 0 || where === 'thread' && Object.hasOwn(value.parts, 'timeline')) return null;
  const parts = sharePartsValue(value.parts);
  if (parts === null) return null;
  if (AREAS.some((area) => !creationAreas(where).includes(area) && stepOf(area, parts) > 0)) return null;
  return { where, parts };
}

export function AgentAccessSummary({ value }: { value: AgentAccessChoice }) {
  const { t } = useTranslation();
  const shared = creationAreas(value.where).filter((area) => stepOf(area, value.parts) > 0);
  return <span className="min-w-0 wrap-anywhere">{t(`agentAccess.where.${value.where}`)}
    {shared.map((area) => <span key={area}> · {t(AREA_WORDS[area])}: {stepOption(t, area, stepOf(area, value.parts)).name}</span>)}
    {value.where !== 'nothing' && shared.length === 0 && <> · {t('agentAccess.noContribution')}</>}
  </span>;
}

/** Editing is local until confirmed; opening, cancelling and area disclosure never change the choice. */
export function AgentAccessControl({ value, onChange, disabled = false, available, onReachChange, onClose, label }: {
  value: AgentAccessChoice; onChange: (choice: AgentAccessChoice) => void; disabled?: boolean;
  /** Ordinary source bounds for the currently edited reach; undefined is not a known grant. Preset editors omit this. */
  available?: (where: AgentAccessWhere) => ShareParts | undefined;
  onReachChange?: (where: AgentAccessWhere) => void;
  onClose?: () => void;
  label?: string;
}) {
  const { t } = useTranslation();
  const editorId = useId();
  const trigger = useRef<HTMLButtonElement>(null);
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(value);
  const [byArea, setByArea] = useState(false);
  const areas = creationAreas(draft.where);
  const level = levelOfParts(draft.parts, areas);
  const mayChoose = (parts: ShareParts) => {
    if (available === undefined || draft.where === 'nothing') return true;
    const bounds = available(draft.where);
    return bounds !== undefined && areas.every((area) => stepOf(area, parts) <= stepOf(area, bounds));
  };
  const permitted = mayChoose(draft.parts);
  const close = () => { setEditing(false); onClose?.(); trigger.current?.focus(); };

  return <section aria-label={label ?? t('agentAccess.title')} className="flex min-w-0 flex-col gap-2">
    <div className="flex flex-wrap items-center gap-2 text-sm">
      <AgentAccessSummary value={value} />
      <Button ref={trigger} size="sm" variant="ghost" disabled={disabled} aria-expanded={editing} aria-controls={editing ? editorId : undefined} onClick={() => { setDraft(value); onReachChange?.(value.where); setByArea(false); setEditing(true); }}>{t('agentAccess.change')}</Button>
    </div>
    {editing && <div id={editorId} className="flex flex-col gap-3 rounded-md border border-border p-3">
      <Select<AgentAccessWhere> label={t('agentAccess.scope')} value={draft.where} disabled={disabled}
        onValueChange={(where) => { setDraft(creationChoice(where, draft.parts)); onReachChange?.(where); }}
        options={REACHES.map((where) => ({ value: where, name: t(`agentAccess.where.${where}`), description: t(`agentAccess.reach.${where}`) }))} />
      {areas.length > 0 && <>
        <div className="flex flex-wrap items-end gap-2">
          <Select<AgentLevel | 'custom'> label={t('access.agents.dialog.what')} value={level ?? 'custom'}
            disabled={disabled || byArea} fieldClassName="min-w-[12rem] flex-[1_1_12rem]"
            onValueChange={(next) => { if (next !== 'custom') setDraft({ ...draft, parts: partsOfLevel(next, areas) }); }}
            options={[...LEVELS.map((entry) => ({ value: entry, ...choiceOption(t, `accessLevel.${entry}`), description: t(`agentAccess.levels.${entry}`), disabled: !mayChoose(partsOfLevel(entry, areas)) })),
              ...(level === null ? [{ value: 'custom' as const, ...choiceOption(t, 'generalAccess.custom'), disabled: true }] : [])]} />
          <Button size="sm" variant="ghost" disabled={disabled} aria-pressed={byArea} onClick={() => setByArea((before) => !before)}>{t('access.agents.dialog.byArea')}</Button>
        </div>
        {byArea && <div role="group" aria-label={t('access.agents.dialog.byAreaLabel')} className="flex flex-wrap gap-x-4 gap-y-2">
          {areas.map((area) => <Select<string> key={area} size="sm" label={t(AREA_WORDS[area])} value={String(stepOf(area, draft.parts))}
            fieldClassName="min-w-[9rem] flex-[1_1_9rem]" disabled={disabled}
            onValueChange={(step) => setDraft({ ...draft, parts: withStep(draft.parts, area, Number(step)) })}
            options={stepsOf(area).map((step) => ({ value: String(step), ...stepOption(t, area, step), disabled: step > 0 && !mayChoose(withStep({}, area, step)) }))} />)}
        </div>}
        {draft.parts.documents === 'manage' && <p className="text-sm text-text-muted">{t('access.agents.manageNote')}</p>}
      </>}
      {draft.where === 'thread' && <p className="text-sm text-text-muted">{t('agentAccess.reach.thread')}</p>}
      {!permitted && <p className="text-sm text-text-muted">{t('agentAccess.unavailable')}</p>}
      <div className="flex flex-wrap justify-end gap-2">
        <Button size="sm" disabled={disabled} onClick={close}>{t('common.cancel')}</Button>
        <Button size="sm" variant="primary" disabled={disabled} blocked={!permitted} onClick={() => { onChange(creationChoice(draft.where, draft.parts)); close(); }}>{t('agentAccess.confirm')}</Button>
      </div>
    </div>}
  </section>;
}

export function AgentAccessSetting({ value, onChange, onEdited, disabled, label, ...control }: ValueControlProps) {
  const { t } = useTranslation();
  const choice = agentAccessValue(value);
  return choice === null ? <ValueControl {...control} label={label} value={value} onChange={onChange} onEdited={onEdited} disabled={disabled} />
    : <div className="flex flex-col gap-2"><AgentAccessControl value={choice} label={label} disabled={disabled} onChange={(next) => { onEdited?.(); onChange(next); }} /><p className="text-sm text-text-muted">{t('agentAccess.preset')}</p></div>;
}
