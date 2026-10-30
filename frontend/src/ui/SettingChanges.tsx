/** A refused resource draft is rebased only after the person opens its current values. */
import { useTranslation } from 'react-i18next';
import { Button } from '@/ui/Button';
import { ValueSummary } from '@/ui/ValueSummary';
import { Table } from '@/ui/Table';
import { Tooltip } from '@/ui/Tooltip';

export interface SettingChange {
  name: string;
  current: unknown;
  draft: unknown;
}

export function SettingChanges({ rows, onKeep, disabled = false, reason }: { rows: readonly SettingChange[]; onKeep: () => void; disabled?: boolean; reason?: string }) {
  const { t } = useTranslation();
  const value = (current: unknown) => <ValueSummary value={current} />;
  return <details className="min-w-0 text-sm">
    <summary className="focus-inset min-h-6 cursor-pointer rounded-sm text-accent-text underline underline-offset-2">{t('settings.compact.review')}</summary>
    <div className="mt-2 flex min-w-0 flex-col gap-2">
      <Table label={t('settings.compact.review')} rows={rows} rowKey={(row) => row.name} columns={[
        { id: 'name', header: t('settings.compact.setting'), cell: (row) => row.name },
        { id: 'current', header: t('settings.compact.current'), cell: (row) => value(row.current), className: 'whitespace-normal' },
        { id: 'draft', header: t('settings.compact.proposed'), cell: (row) => value(row.draft), className: 'whitespace-normal' },
      ]} />
      <Tooltip content={reason ?? (disabled ? t('settings.compact.readCurrent') : undefined)}><Button size="sm" className="self-start" blocked={disabled || reason !== undefined}
        aria-description={reason ?? (disabled ? t('settings.compact.readCurrent') : undefined)} onClick={onKeep}>{t('settings.compact.keepDraft')}</Button></Tooltip>
    </div>
  </details>;
}
