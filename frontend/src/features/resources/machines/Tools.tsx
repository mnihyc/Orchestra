/** A machine's discoveries are imported in a named environment, which supplies their home and enablement. */
import { Link } from '@tanstack/react-router';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { FoundMcp, NO_ENVIRONMENT } from '@/features/resources/mcp/Import';
import type { Machine } from '@/generated/api-types';
import { Button } from '@/ui/Button';
import { EmptyState } from '@/ui/EmptyState';
import { Select } from '@/ui/Select';

export function MachineTools({ machine }: { machine: Pick<Machine, 'id' | 'name' | 'environments'> }) {
  const { t } = useTranslation();
  const [choice, setChoice] = useState('');
  const selected = choice === '' && machine.environments.length === 1 ? machine.environments[0] : machine.environments.find((environment) => environment.id === choice);
  return <div className="flex min-w-0 flex-col gap-4">
    {machine.environments.length === 0 ? <EmptyState compact title={t('mcpImport.noEnvironment')}
      action={<Button asChild><Link to="/resources/environments">{t('mcpImport.environments')}</Link></Button>} /> : <>
      <Select label={t('mcpImport.environment')} value={selected?.id ?? NO_ENVIRONMENT} onValueChange={setChoice}
        options={[{ value: NO_ENVIRONMENT, label: t('mcpImport.chooseEnvironment'), disabled: true }, ...machine.environments.map((environment) => ({ value: environment.id, label: environment.name }))]} />
      {selected !== undefined && <FoundMcp key={selected.id} environment={selected} machineId={machine.id} />}
    </>}
  </div>;
}
