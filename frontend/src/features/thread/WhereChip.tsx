/** Where a thread works, in one chip beside its composer controls: "<environment> · <directory>", which opens the change of the environment and the directory.
 *
 * It is where the environment's state shows, so that the place a thread works and the directory it starts in can be found and changed
 * from the thread itself; `/directory` opens the same change through `openWhere`.
 */
import { Box } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { openWhere } from '@/features/thread/openWhere';
import type { Thread } from '@/generated/api-types';
import { holds } from '@/lib/permissions';
import { Button } from '@/ui/Button';

export function WhereChip({ thread, readOnly = false }: { thread: Thread; readOnly?: boolean }) {
  const { t } = useTranslation();
  const environment = thread.environment;
  if (environment === null || thread.binding === null) return null;
  const may = !readOnly && holds(thread.access, 'thread.manage') && !thread.archived && thread.side_of === null && thread.lock === null;
  const why = readOnly ? t('composer.row.readOnly') : !holds(thread.access, 'thread.manage') ? t('composer.row.bindingReadOnly', { who: thread.owner.display_name }) : thread.archived ? t('composer.row.archived')
    : thread.side_of !== null ? t('composer.row.sideBinding') : thread.lock !== null ? t('threadMove.jobMustEnd') : undefined;
  const where = thread.directory === '' ? environment.name : `${environment.name} · ${thread.directory}`;
  const hint = why === undefined ? where : `${where} · ${why}`;
  return <Button size="sm" variant="ghost" bounded blocked={!may} reason={hint} icon={<Box aria-hidden />} aria-label={t('composer.where.label', { where })} data-where-chip
    className="min-w-0 max-w-(--chip-width-wide) px-2 text-sm" onClick={() => { if (may) openWhere(thread); }}>
    <span className="min-w-0 truncate">{where}</span>
  </Button>;
}
