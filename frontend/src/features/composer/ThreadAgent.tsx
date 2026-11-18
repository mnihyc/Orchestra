/** The agent of a thread, as the one control of the box that is not a model's: its name alone, and a list of the agents that work with the provider the thread is on, the rest greyed under "Does not work with …".
 *
 * Choosing another agent is a change of what the thread runs on, which the wizard of a switch plans, so it opens with that agent chosen.
 * A thread that is bound to nothing yet opens the dialog that sets it up.
 */
import { useQuery } from '@tanstack/react-query';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { threadBindingOptionsQuery } from '@/api/start';
import { type AgentChoice, AgentChip, agentChoicesOf } from '@/features/composer/BindingLists';
import { openChange } from '@/features/change/target';
import type { Thread } from '@/generated/api-types';
import { Button } from '@/ui/Button';

export function ThreadAgentChip({ thread, canBind, reason, onSetUp }: { thread: Thread; canBind: boolean; reason: string | undefined; onSetUp: () => void }) {
  const { t } = useTranslation();
  const [opened, setOpened] = useState(false);
  const binding = thread.binding;
  // The agents of every provider are read once the list is opened.
  const options = useQuery({ ...threadBindingOptionsQuery(thread.id), enabled: opened && canBind && binding !== null });
  if (binding === null) {
    return <Button size="sm" variant="ghost" bounded blocked={!canBind} reason={reason} aria-label={t('composer.row.agent', { name: t('composer.row.unbound') })} className="min-w-0 max-w-full px-2 text-sm"
      onClick={() => { if (canBind) onSetUp(); }}><span className="min-w-0 truncate">{t('composer.row.unbound')}</span></Button>;
  }
  const own: AgentChoice = { id: binding.agent.id, name: binding.agent.name, fits: true, usable: true, reason: null };
  const choices = options.data === undefined ? [own] : agentChoicesOf(options.data, binding.provider.id);
  return <AgentChip choices={choices} value={binding.agent.id} name={binding.agent.name} provider={binding.provider.name} disabled={!canBind} reason={reason}
    pending={opened && options.isPending} loaded={options.data !== undefined} onOpenChange={(next) => { if (next) setOpened(true); }}
    onChoose={(choice) => openChange({ kind: 'thread', id: thread.id, name: thread.name }, { kind: 'switch', focus: 'provider', agent: choice.id })} />;
}
