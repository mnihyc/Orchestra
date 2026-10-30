/** Compact role names and each selected role's explicit full-access choice. */
import type { TFunction } from 'i18next';
import { useTranslation } from 'react-i18next';

import { Checkbox } from '@/ui/Checkbox';
import { Switch } from '@/ui/Switch';
import { Badge } from '@/ui/Badge';

export const CONNECTOR_ROLES = ['environment', 'agent', 'provider'] as const;
export type ConnectorRole = typeof CONNECTOR_ROLES[number];

export function roleName(t: TFunction, role: string): string {
  switch (role) {
    case 'environment': return t('machines.add.roleChoices.environment.name');
    case 'agent': return t('machines.add.roleChoices.agent.name');
    case 'provider': return t('machines.add.roleChoices.provider.name');
    case 'node': return t('machines.add.roleChoices.node.name');
    default: return role;
  }
}

/** Lists and machine pages use the same names as the enrollment choices. */
export function MachineRoleChips({ roles }: { roles: readonly string[] }) {
  const { t } = useTranslation();
  return <span className="flex flex-wrap gap-1">{roles.map((role) => <Badge key={role}>{roleName(t, role)}</Badge>)}</span>;
}

interface Props {
  roles: readonly ConnectorRole[];
  delegated: readonly ConnectorRole[];
  node: boolean;
  nodeDelegated: boolean;
  onRole: (role: ConnectorRole, checked: boolean) => void;
  onDelegate: (role: ConnectorRole, checked: boolean) => void;
  onNode: (checked: boolean) => void;
  onNodeDelegate: (checked: boolean) => void;
}

export function EnrollmentRoles({ roles, delegated, node, nodeDelegated, onRole, onDelegate, onNode, onNodeDelegate }: Props) {
  const { t } = useTranslation();
  const nodeName = roleName(t, 'node');
  return <section aria-label={t('machines.add.rolesTitle')} className="flex flex-col gap-3" data-help-control="machine-delegate">
    <span className="text-sm font-medium">{t('machines.add.rolesTitle')}</span>
    {CONNECTOR_ROLES.map((role) => <fieldset key={role} aria-label={roleName(t, role)} className="grid grid-cols-1 items-start gap-3 sm:grid-cols-2">
      <Checkbox label={roleName(t, role)}
        description={t(`machines.add.roleChoices.${role}.description`)}
        checked={!node && roles.includes(role)} onCheckedChange={(checked) => onRole(role, checked)} />
      {!node && roles.includes(role) && <Switch name={t('machines.add.fullAccessState', { state: t(delegated.includes(role) ? 'common.on' : 'common.off') })}
        description={t('machines.add.fullAccessDescription')}
        checked={delegated.includes(role)} onCheckedChange={(checked) => onDelegate(role, checked)} />}
    </fieldset>)}
    <fieldset aria-label={nodeName} className="grid grid-cols-1 items-start gap-3 sm:grid-cols-2">
      <Checkbox label={nodeName}
        description={t('machines.add.roleChoices.node.description')}
        checked={node} onCheckedChange={onNode} />
      {node && <Switch name={t('machines.add.fullAccessState', { state: t(nodeDelegated ? 'common.on' : 'common.off') })}
        description={t('machines.add.fullAccessDescription')}
        checked={nodeDelegated} onCheckedChange={onNodeDelegate} />}
    </fieldset>
  </section>;
}
