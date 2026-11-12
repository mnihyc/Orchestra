/** Read-only discovery facts stay separate from the options a provider manager enables. */
import { useTranslation } from 'react-i18next';
import { useAppearance } from '@/lib/appearance';
import { formatNumber } from '@/lib/format';
import { formatExact } from '@/lib/time';

const record = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value);
const text = (value: unknown): value is string => typeof value === 'string' && value.trim() !== '';
const tokens = (value: unknown): value is number => typeof value === 'number' && Number.isSafeInteger(value) && value > 0;
const reportWord = (dictionary: unknown, key: string): string => {
  const value = record(dictionary) && Object.hasOwn(dictionary, key) ? dictionary[key] : undefined;
  return typeof value === 'string' ? value : key;
};

/** Interpret only explicit support booleans; unknown payload members are retained by storage, never enabled here. */
function supportRows(value: unknown): { path: string[]; supported: boolean }[] {
  if (!record(value)) return [];
  const rows: { path: string[]; supported: boolean }[] = [];
  const pending = Object.entries(value).map(([key, value]) => ({ path: [key], value }));
  for (const { path, value: next } of pending) {
    if (!record(next)) continue;
    if (typeof next.supported === 'boolean') rows.push({ path, supported: next.supported });
    for (const [key, value] of Object.entries(next)) {
      if (key !== 'supported' && record(value)) pending.push({ path: [...path, key], value });
    }
  }
  return rows;
}

export function ModelReport({ offered, source }: { offered: unknown; source: string }) {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  const timeZone = useAppearance((state) => state.timeZone);
  if (!record(offered) || !record(offered.report)) return null;
  const report = offered.report;
  const support = supportRows(report.capabilities);
  const labels: Record<string, string> = {
    image_input: t('modelReport.capabilities.image_input'),
    pdf_input: t('modelReport.capabilities.pdf_input'),
    server_tools: t('modelReport.capabilities.server_tools'),
    web_search: t('modelReport.capabilities.web_search'),
    code_execution: t('modelReport.capabilities.code_execution'),
    effort: t('modelReport.capabilities.effort'),
    low: t('modelReport.capabilities.low'),
    medium: t('modelReport.capabilities.medium'),
    high: t('modelReport.capabilities.high'),
    xhigh: t('modelReport.capabilities.xhigh'),
    max: t('modelReport.capabilities.max'),
    thinking: t('modelReport.capabilities.thinking'),
    types: t('modelReport.capabilities.types'),
    adaptive: t('modelReport.capabilities.adaptive'),
    enabled: t('modelReport.capabilities.enabled'),
    disabled: t('modelReport.capabilities.disabled'),
    between_tools: t('modelReport.capabilities.between_tools'),
    structured_outputs: t('modelReport.capabilities.structured_outputs'),
    context_management: t('modelReport.capabilities.context_management'),
    citations: t('modelReport.capabilities.citations'),
    batch: t('modelReport.capabilities.batch'),
  };
  const stages: Record<string, string> = {
    active: t('modelReport.stages.active'),
    deprecated: t('modelReport.stages.deprecated'),
    retired: t('modelReport.stages.retired'),
  };
  const dates = ([
    { key: 'deprecated_at', label: t('modelReport.deprecated_at') },
    { key: 'retires_at', label: t('modelReport.retires_at') },
  ] as const).flatMap(({ key, label }) => {
    const value = report[key];
    return text(value) && Number.isFinite(Date.parse(value)) ? [{ key, label, value }] : [];
  });
  const hasValues = support.length > 0 || tokens(offered.context_max) || tokens(report.output_max) || text(report.lifecycle) || text(report.line) || dates.length > 0;
  if (!hasValues) return null;
  return <details className="min-w-0 rounded border border-border" data-testid="model-report">
    <summary className="cursor-pointer px-3 py-2 text-sm font-medium text-text focus-visible:outline focus-visible:outline-2 focus-visible:outline-accent">{t('modelReport.title')}</summary>
    <div className="flex min-w-0 flex-col gap-2 px-3 pb-3 text-sm">
      <span className="text-text-muted">{t('modelReport.source', { source })}</span>
      <dl className="grid min-w-0 grid-cols-[minmax(0,1fr)_minmax(0,1fr)] gap-x-3 gap-y-1 break-words">
        {text(report.lifecycle) && <><dt>{t('modelReport.lifecycle')}</dt><dd>{reportWord(stages, report.lifecycle)}</dd></>}
        {tokens(offered.context_max) && <><dt>{t('modelReport.inputMax')}</dt><dd className="tabular-nums">{formatNumber(offered.context_max, locale)}</dd></>}
        {tokens(report.output_max) && <><dt>{t('modelReport.outputMax')}</dt><dd className="tabular-nums">{formatNumber(report.output_max, locale)}</dd></>}
        {text(report.line) && <><dt>{t('modelReport.line')}</dt><dd>{report.line}</dd></>}
        {dates.map(({ key, label, value }) => <div className="contents" key={key}><dt>{label}</dt><dd><time dateTime={value}>{formatExact(value, { locale, timeZone })}</time></dd></div>)}
        {support.map(({ path, supported }) => <div className="contents" key={JSON.stringify(path)}><dt>{path.map((key) => reportWord(labels, key)).join(' · ')}</dt><dd>{t(supported ? 'modelReport.supported' : 'modelReport.unsupported')}</dd></div>)}
      </dl>
    </div>
  </details>;
}
