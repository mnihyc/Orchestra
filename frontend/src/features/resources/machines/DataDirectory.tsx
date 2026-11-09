/** The machine's observed directory metadata never opens contents or turns opaque entries into paths. */
import { useTranslation } from 'react-i18next';
import type { MachineDataInventory, MachineDataPart } from '@/generated/api-types';
import { useFormatContext } from '@/lib/appearance';
import { formatBytes, formatNumber } from '@/lib/format';
import { Section } from '@/shell/Page';
import { RelativeTime } from '@/ui/RelativeTime';
import { StateLine } from '@/ui/StateLine';
import { Table, type Column } from '@/ui/Table';

const COLUMNS = { kind: 'kind', state: 'state' } as const;

export function MachineDataDirectory({ inventory }: { inventory?: MachineDataInventory | null }) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  if (inventory === undefined || inventory === null) return <Section id="machine-data-directory" title={t('machineData.title')}><StateLine compact message={t('machineData.unavailable')} /></Section>;
  const ordinary = inventory.parts.filter((part) => part.kind !== 'sealed_store');
  const sealed = inventory.parts.filter((part) => part.kind === 'sealed_store');
  const notes = [!inventory.complete ? t('machineData.incomplete') : null, inventory.problem === null ? null : t(`machineData.problems.${inventory.problem}`), inventory.unlisted_parts > 0 ? t('machineData.unlisted', { number: formatNumber(inventory.unlisted_parts, locale) }) : null].filter((word) => word !== null);
  const size = (part: MachineDataPart) => part.bytes === null ? '–' : formatBytes(part.bytes, locale);
  const columns: Column<MachineDataPart>[] = [
    { id: 'name', header: t('machineData.name'), grow: true, cell: (part, shown) => <div className="flex min-w-0 flex-col gap-1"><span className="wrap-anywhere">{part.name}</span>
      {!shown.has(COLUMNS.kind) && <span className="text-sm text-text-muted">{t(`machineData.kinds.${part.kind}`)}</span>}
      {!shown.has(COLUMNS.state) && part.problem !== null && <span className="text-sm text-warning">{t(`machineData.problems.${part.problem}`)}</span>}
    </div> },
    { id: COLUMNS.kind, header: t('machineData.kind'), from: 'sm', cell: (part) => t(`machineData.kinds.${part.kind}`) },
    { id: 'size', header: t('machineData.size'), align: 'end', cell: size },
    { id: COLUMNS.state, header: t('machineData.state'), from: 'md', cell: (part) => part.problem === null ? '–' : t(`machineData.problems.${part.problem}`) },
  ];
  return <Section id="machine-data-directory" title={t('machineData.title')} action={<span className="flex flex-wrap items-baseline gap-1 text-sm text-text-muted"><span>{t('machineData.observed')}</span><RelativeTime at={inventory.observed_at} /></span>}>
    <div className="flex min-w-0 flex-col gap-3">
      {notes.length > 0 && <StateLine compact tone="warning" message={notes.join(' · ')} />}
      {inventory.complete && inventory.parts.length === 0 && <StateLine compact message={t('machineData.empty')} />}
      {ordinary.length > 0 && <Table label={t('machineData.parts')} rows={ordinary} rowKey={(part) => part.id} columns={columns} />}
      {sealed.length > 0 && <Table label={t('machineData.sealed')} rows={sealed} rowKey={(part) => part.name} columns={[
        { id: 'name', header: t('machineData.name'), grow: true, cell: (part) => <span className="wrap-anywhere">{part.name}</span> },
        { id: 'size', header: t('machineData.size'), align: 'end', cell: size },
      ]} />}
    </div>
  </Section>;
}
