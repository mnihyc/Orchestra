/** The dialog that edits an agent profile: its name, the build of its harness it runs and whether it is on; the harness it was made for, shown as fixed, and its profile, changed on its own tab.
 *
 * What a profile lets be changed after it was made is what the request takes: a name, a profile, the
 * members of its descriptor that are overridden, and whether it is switched off. The name, the version
 * pin that lies inside the profile and the switch are here; the rest of the profile is JSON and is changed on
 * its tab, where it is read in full. Whoever opens it as Rename sees the name alone.
 */
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { adminSpec } from '@/ui/AdminAct';
import { choiceOption } from '@/i18n/choices';

import { agentKey, harnessKindsQuery, updateAgent } from '@/api/resources';
import { resourceImpactQuery } from '@/api/resource-impact';
import { ImpactCounts } from '@/features/storage/Impact';
import { versionPin } from '@/features/resources/agents/descriptor';
import { ChangedOn, EditDialog, Fixed } from '@/features/resources/EditDialog';
import { useHarnessTitle } from '@/features/resources/harnesses';
import { Hint } from '@/features/help/Hint';
import type { Agent } from '@/generated/api-types';
import { Input } from '@/ui/Input';
import { ChoiceLocator } from '@/ui/ChoiceLocator';
import { Switch } from '@/ui/Switch';
import { Checkbox } from '@/ui/Checkbox';
import { toast } from '@/ui/Toast';
import { ErrorState } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';

/** The sentinel of the version menu for "the default build": a menu cannot hold an empty value. */
export const DEFAULT_BUILD = '\u0000default';

export interface EditAgentProps {
  agent: Agent;
  /** `name` shows the name alone, as Rename does. */
  only?: 'name';
  /** Opens the tab where the rest of the profile is changed. */
  onProfile: () => void;
  onClose: () => void;
}

export function EditAgent({ agent, only, onProfile, onClose }: EditAgentProps) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const harnessTitle = useHarnessTitle();
  const kinds = useQuery(harnessKindsQuery);
  const builds = kinds.data?.find((entry) => entry.kind === agent.harness_kind)?.builds ?? [];
  const pinned = versionPin(agent.profile) ?? DEFAULT_BUILD;
  const [name, setName] = useState(agent.name);
  const [pin, setPin] = useState(pinned);
  const [on, setOn] = useState(!agent.disabled);
  const [note,setNote]=useState('');
  const [runWaiting,setRunWaiting]=useState(false);
  const profile = typeof agent.profile === 'object' && agent.profile !== null && !Array.isArray(agent.profile) ? (agent.profile as Record<string, unknown>) : {};

  const nameChanged = name.trim() !== agent.name;
  const pinChanged = only === undefined && pin !== pinned;
  const onChanged = only === undefined && on !== !agent.disabled;
  const impact=useQuery({...resourceImpactQuery({object:{kind:'agent',id:agent.id},model_id:null}),enabled:onChanged});
  const save = useMutation({
    mutationFn: () => {
      const patch={
        expected_revision: agent.revision,
        ...(nameChanged ? { name: name.trim() } : {}),
        ...(pinChanged ? { profile: { ...profile, harness_version: pin === DEFAULT_BUILD ? null : pin } } : {}),
        ...(onChanged ? { disabled: !on } : {}),
      };
      if(onChanged&&on)return updateAgent(agent.id,patch,undefined,runWaiting);
      return onChanged&&!on&&note.trim()!==''?updateAgent(agent.id,patch,note.trim()):updateAgent(agent.id,patch);
    },
    onSuccess: (updated) => {
      toast({ tone: 'success', title: t('resources.edit.saved', { name: updated.name }) });
      onClose();
    },
    // Somebody else changed it first: it is read again before the next try.
    onError: () => void queryClient.invalidateQueries({ queryKey: agentKey(agent.id) }),
  });

  return (
    <EditDialog
      admin={adminSpec(agent, 'agent.manage')}
      title={only === 'name' ? t('resources.edit.renameTitle', { name: agent.name }) : t('resources.edit.title', { name: agent.name })}
      changed={nameChanged || pinChanged || onChanged}
      valid={name.trim() !== ''&&(!onChanged||impact.isSuccess&&!impact.data.stopping)}
      pending={save.isPending}
      error={save.error}
      onSave={() => save.mutate()}
      onClose={onClose}
      {...(only === 'name'
        ? {}
        : {
            rest: (
              <>
                <Fixed label={t('agents.overview.harness')} value={harnessTitle(agent.harness_kind)} why={t('agents.edit.harnessWhy')} />
                <Fixed
                  label={t('agents.overview.usable')}
                  value={agent.machine === null ? t('agents.list.anyRuntime') : agent.machine.name}
                  why={agent.machine === null ? t('agents.edit.usableWhy') : t('agents.edit.usableMachineWhy', { machine: agent.machine.name })}
                />
                <ChangedOn label={t('agents.profile.json')} value={t('agents.edit.profileValue')} tab={t('agents.tabs.profile')} onOpen={onProfile} />
              </>
            ),
          })}
    >
      <Input label={t('resources.edit.name')} value={name} disabled={save.isPending} onChange={(event) => setName(event.target.value)} autoComplete="off" data-autofocus />
      {only === undefined && (
        <>
          <ChoiceLocator
            label={t('agents.profile.pin')}
            help={<Hint id="agent-pin" />}
            value={pin}
            disabled={save.isPending}
            onValueChange={setPin}
            options={[
              { value: DEFAULT_BUILD, ...choiceOption(t, 'sourceKind.default'), description: t('agents.list.pinDefault') },
              ...builds.map((build) => ({ value: build.version, ...choiceOption(t, 'dynamic.value', { name: build.version, context: t('agents.profile.pin') }), description: build.is_default ? t('agents.overview.buildDefault', { version: build.version }) : t('agents.profile.pinHint') })),
              ...(pin !== DEFAULT_BUILD && !builds.some((build) => build.version === pin) ? [{ value: pin, ...choiceOption(t, 'dynamic.value', { name: pin, context: t('agents.profile.pin') }) }] : []),
            ]}
          />
          <Switch label={t('agents.edit.on')} hint={t('agents.edit.onHint')} checked={on} disabled={save.isPending} onCheckedChange={setOn} />
          {onChanged && <div className="flex flex-col gap-2">
            {impact.isPending?<SkeletonRows rows={2} />:impact.isError?<ErrorState compact error={impact.error} onRetry={()=>void impact.refetch()} />:<ImpactCounts impact={impact.data} kind={on?'enable':'disable'} />}
            {!on&&<Input optional label={t('resourceImpact.note')} value={note} onChange={(event)=>setNote(event.target.value)} disabled={save.isPending} />}
            {on&&<Checkbox label={t('resourceImpact.runWaiting')} checked={runWaiting} onCheckedChange={setRunWaiting} disabled={save.isPending} />}
          </div>}
        </>
      )}
    </EditDialog>
  );
}
