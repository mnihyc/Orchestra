/** Where a resource comes from, said as a person says it: a machine of one's own by its name, a container by the host it is on.
 * The name of a container's machine is its hostname and is never shown; the server hands over the host's name instead. */
import { Trans, useTranslation } from 'react-i18next';
import type { MachineRef } from '@/generated/api-types';
import { MachineLink } from './MachineLink';

export function MachineOrigin({ machine }: { machine: Pick<MachineRef, 'id' | 'name' | 'placement' | 'host_id'> }) {
  const { t } = useTranslation();
  if (machine.placement === 'managed') return <Trans i18nKey="resourceGroups.fromContainer" components={{ host: <MachineLink machine={machine} /> }} />;
  return <><span>{t('resourceGroups.from')}</span><MachineLink machine={machine} /></>;
}
