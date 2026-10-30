/** What is on screen as CSV: the series, or the records.
 *
 * The series is one line for each point of the chart; the records are the ledger rows behind the
 * figures, page after page of the newest first. Both are what the span and the filters on screen ask for,
 * saved as a file with the days in its name. A file of records holds at most as many as a browser can keep
 * whole, and says so when it stops short.
 */
import { Download, FileSpreadsheet, ScrollText } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { EXPORT_RECORDS_MOST, fetchRecordsCsv, fetchSeriesCsv, type EventParams, type UsageParams } from '@/api/usage';
import { saveFile, usageFileName } from '@/features/usage/saveFile';
import type { Span, UsageScope } from '@/features/usage/search';
import { formatNumber } from '@/lib/format';
import { useFormatContext } from '@/lib/appearance';
import { Button } from '@/ui/Button';
import { Menu } from '@/ui/Menu';
import { toast } from '@/ui/Toast';
import { useErrorText } from '@/ui/errorText';

export interface ExportProps {
  scope: UsageScope;
  span: Span;
  figures: UsageParams;
  records: EventParams;
}

export function Export({ scope, span, figures, records }: ExportProps) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  const describe = useErrorText();
  const [busy, setBusy] = useState<'series' | 'records' | null>(null);
  const [rows, setRows] = useState(0);

  const run = async (what: 'series' | 'records') => {
    setBusy(what);
    setRows(0);
    try {
      if (what === 'series') {
        saveFile(await fetchSeriesCsv(figures), usageFileName(scope, span, 'series'));
        toast({ tone: 'success', title: t('usage.export.saved', { name: usageFileName(scope, span, 'series') }) });
      } else {
        const file = await fetchRecordsCsv(records, setRows);
        saveFile(file.text, usageFileName(scope, span, 'records'));
        toast({
          tone: file.cut ? 'info' : 'success',
          title: t('usage.export.saved', { name: usageFileName(scope, span, 'records') }),
          ...(file.cut ? { description: t('usage.export.cut', { count: formatNumber(EXPORT_RECORDS_MOST, locale) }) } : {}),
        });
      }
    } catch (error) {
      toast({ tone: 'danger', title: t('usage.export.failed'), description: describe(error).message });
    } finally {
      setBusy(null);
    }
  };

  return (
    <Menu
      label={t('usage.export.label')}
      align="end"
      trigger={
        <Button size="sm" icon={<Download aria-hidden />} pending={busy !== null}>
          {busy === 'records' && rows > 0 ? t('usage.export.progress', { count: formatNumber(rows, locale) }) : t('usage.export.title')}
        </Button>
      }
      entries={[
        { id: 'series', name: t('usagePerf.choices.exports.series.name'), description: t('usagePerf.choices.exports.series.description'), icon: <FileSpreadsheet aria-hidden />, disabled: busy !== null ? t('usage.export.busy') : false, onSelect: () => void run('series') },
        { id: 'records', name: t('usagePerf.choices.exports.records.name'), description: t('usagePerf.choices.exports.records.description'), icon: <ScrollText aria-hidden />, disabled: busy !== null ? t('usage.export.busy') : false, onSelect: () => void run('records') },
      ]}
    />
  );
}
