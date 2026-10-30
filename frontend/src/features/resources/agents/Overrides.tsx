/** A profile's declared descriptor overrides, saved without turning reported defaults into overrides. */
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { agentKey, updateAgent } from '@/api/resources';
import { descriptorOverrides, readObject } from '@/features/resources/jsonObject';
import type { Agent } from '@/generated/api-types';
import { Button } from '@/ui/Button';
import { adminSpec } from '@/ui/AdminAct';
import { ErrorAlert } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';
import { Textarea } from '@/ui/Textarea';
import { SettingEditor } from '@/ui/SettingEditor';
import { SettingChanges } from '@/ui/SettingChanges';
import { ValueSummary } from '@/ui/ValueSummary';
import { SettingRow } from '@/ui/SettingRow';
import { ApiError } from '@/api/client';

export function DescriptorOverrides({ agent, manage }: { agent: Agent; manage: boolean }) {
  const { t } = useTranslation();
  const client = useQueryClient();
  const [base, setBase] = useState(agent);
  const [observed, setObserved] = useState(agent.revision);
  const [open, setOpen] = useState(false);
  const initial = JSON.stringify(descriptorOverrides(base.descriptor, base.descriptor_overridden), null, 2);
  const current = JSON.stringify(descriptorOverrides(agent.descriptor, agent.descriptor_overridden), null, 2);
  const [text, setText] = useState(initial);
  const changed = text !== initial;
  if (observed !== agent.revision) {
    setObserved(agent.revision);
    if (!changed) { setBase(agent); setText(current); }
  }
  const value = readObject(text);
  const save = useMutation({ mutationFn: () => {
    if (value === null) throw new Error(t('resourceDetails.invalidObject'));
    return updateAgent(agent.id, { expected_revision: base.revision, descriptor_overrides: value });
  }, onSuccess: (updated) => {
    setBase(updated); setText(JSON.stringify(descriptorOverrides(updated.descriptor, updated.descriptor_overridden), null, 2));
    setOpen(false); client.setQueryData(agentKey(updated.id), updated);
  }, onError: (error) => {
    if (error instanceof ApiError && error.is('conflict.stale_revision')) void client.invalidateQueries({ queryKey: agentKey(agent.id), exact: true });
  } });
  const stale = save.error instanceof ApiError && save.error.is('conflict.stale_revision') || changed && agent.revision !== base.revision;
  return <ul id="agent-overrides" aria-label={t('resourceDetails.overrides.title')} className="min-w-0 rounded-lg border border-border bg-surface">
    {!manage ? <SettingRow name={t('resourceDetails.overrides.title')} readOnly value={<ValueSummary value={descriptorOverrides(agent.descriptor, agent.descriptor_overridden)} />} />
      : <SettingEditor name={t('resourceDetails.overrides.title')} description={t('resourceDetails.overrides.help')}
      summary={<ValueSummary inControl value={descriptorOverrides(agent.descriptor, agent.descriptor_overridden)} />} label={t('settings.compact.editValue', { name: t('resourceDetails.overrides.title') })}
      open={open} onOpenChange={setOpen} disabled={save.isPending}
      actions={changed ? <><Button size="sm" admin={adminSpec(agent, 'agent.manage')} pending={save.isPending} disabled={value === null || stale} onClick={() => save.mutate()}>{t('resources.edit.save')}</Button>
        <Button size="sm" disabled={save.isPending} onClick={() => { setText(current); setBase(agent); setOpen(false); save.reset(); }}>{t('common.cancel')}</Button></> : undefined}
      status={stale ? <InlineAlert tone="warning"><p>{t('settings.compact.changedReason')}</p><SettingChanges rows={[
        { name: t('resourceDetails.overrides.title'), current: descriptorOverrides(agent.descriptor, agent.descriptor_overridden), draft: value ?? text },
      ]} disabled={save.isPending || agent.revision === base.revision} reason={value === null ? t('resourceDetails.invalidObject') : undefined} onKeep={() => { setBase(agent); save.reset(); }} /></InlineAlert>
        : save.error !== null ? <ErrorAlert error={save.error} /> : save.isSuccess && !changed ? <InlineAlert tone="info">{t('resourceDetails.overrides.saved')}</InlineAlert>
          : !open && changed ? <p role="status" className="text-xs text-text-muted">{t('settings.compact.draft')}</p> : undefined}>
      <Textarea label={t('resourceDetails.overrides.label')} hideLabel value={text} disabled={save.isPending} onChange={(event) => setText(event.target.value)} error={value === null ? t('resourceDetails.invalidObject') : undefined} className="font-mono" />
    </SettingEditor>}
  </ul>;
}
