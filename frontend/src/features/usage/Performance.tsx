/** Request performance figures and a sortable comparison in the current usage scope. */
import { useListChoice } from '@/lib/listState';
import { ArrowDownUp } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import type { UsageComparison, UsagePerformance } from '@/generated/api-types';
import { errorRate, errorsText, relativeSpeed, speedText, timeText } from '@/features/usage/performance';
import { changeOf } from '@/features/usage/figures';
import { ChangeLine } from '@/features/usage/Figures';
import { effortDisplayName, modelDisplayName } from '@/features/models/names';
import { offeredChoice } from '@/i18n/choices';
import { useFormatContext } from '@/lib/appearance';
import { boxClasses,Section } from '@/shell/Page';
import { formatNumber } from '@/lib/format';
import { Button } from '@/ui/Button';
import { Select } from '@/ui/Select';
import { Table, type Column, type Room } from '@/ui/Table';
import { Tooltip } from '@/ui/Tooltip';

type Performance = UsagePerformance | undefined;
type Sort = 'model' | 'tier' | 'effort' | 'program' | 'speed' | 'speedP90' | 'firstToken' | 'firstTokenP90' | 'requests' | 'errors';
const SPEED_COLUMN = 'speed';
const COMPARISON_ROOMS: Partial<Record<Sort, Room>> = { tier: 'xl', effort: 'xl', program: 'xl', speedP90: 'xl', firstToken: 'md', firstTokenP90: 'xl', requests: 'sm', errors: 'lg' };
const REQUESTS_COLUMN = 'requests';
const FIRST_COLUMN = 'firstToken';
const ERRORS_COLUMN = 'errors';
const SPEED_TAIL_COLUMN = 'speedP90';
const FIRST_TAIL_COLUMN = 'firstTokenP90';

export function NotMeasured({ performance, speed = false }: { performance?: UsagePerformance; speed?: boolean }) {
  const { t } = useTranslation();
  const reason = performance === undefined || performance.requests === 0 ? t('usagePerf.noSamples')
    : speed && performance.hidden_requests > 0 ? t('usagePerf.hiddenThinking')
    : speed && performance.short_requests > 0 ? t('usagePerf.tooShort') : t('usagePerf.noBoundaries');
  return <span className="inline-flex items-center gap-1"><span>{t('usagePerf.dash')}</span><Tooltip content={reason}><Button size="sm" variant="ghost" aria-label={t('usagePerf.notMeasured')}>{t('usagePerf.why')}</Button></Tooltip></span>;
}

/** Three figures, each beside the identical measure of the immediately preceding span. */
export function PerformanceFigures({ current, previous, before = '' }: { current: Performance; previous?: Performance; before?: string }) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  const speed = (p: Performance) => speedText(p?.speed_milli === undefined || p.speed_milli === null ? null : p.speed_milli / 1000, locale, t);
  const first = (p: Performance) => timeText(p?.first_token_ms, locale, t);
  const rate = (p: Performance) => errorsText(p,locale,t);
  const cards = [
    { label: t('usagePerf.speed'), format: speed, read: (p: Performance) => p?.speed_milli ?? null, speed: true },
    { label: t('usagePerf.firstToken'), format: first, read: (p: Performance) => p?.first_token_ms ?? null, thinking: true },
    { label: t('usagePerf.errors'), format: rate, read: errorRate },
  ];
  return <ul aria-label={t('usagePerf.figures')} className="grid grid-cols-1 gap-3 sm:grid-cols-3">
    {cards.map(({ label, format, read, speed: isSpeed, thinking }) =>
      <li key={label} className={`${boxClasses} flex min-w-0 flex-col gap-1 px-4 py-3`}>
        <span className="text-sm font-medium text-text-muted">{label}</span>
        <span className="text-2xl font-semibold text-text tabular-nums">{read(current) === null ? <NotMeasured performance={current} speed={isSpeed} /> : format(current)}</span>
        <ChangeLine change={read(current) === null ? { kind: 'none' } : changeOf(read(current) ?? 0, read(previous))} before={before} />
        {thinking && current !== undefined && current.hidden_requests > current.requests / 2 && <span className="text-xs text-text-muted">{t('usagePerf.includesThinking')}</span>}
      </li>)}
  </ul>;
}

function value(row: UsageComparison, sort: Sort): string | number | null {
  const p = row.performance;
  switch (sort) {
    case 'model': return row.model;
    case 'tier': return row.tier;
    case 'effort': return row.effort;
    case 'program': return row.program;
    case 'speed': return p.speed_milli;
    case 'speedP90': return p.speed_p90_milli;
    case 'firstToken': return p.first_token_ms;
    case 'firstTokenP90': return p.first_token_p90_ms;
    case 'requests': return p.requests;
    case 'errors': return errorRate(p);
  }
}

export function PerformanceComparison({ rows, modelNames, onFilter }: { rows: readonly UsageComparison[]; modelNames: ReadonlyMap<string, string>; onFilter: (patch: { model: string; tier: string; effort: string; program: string }) => void }) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  const [sort, setSort] = useListChoice<{ key: Sort; descending: boolean }>('performance', 'sort', { key: 'requests', descending: true });
  const modelName=(row:UsageComparison)=>modelDisplayName(modelNames.get(row.model),t);
  const ordered = [...rows].sort((a, b) => {
    const x = sort.key==='model'?modelName(a):value(a, sort.key), y = sort.key==='model'?modelName(b):value(b, sort.key);
    if (x === null) return y === null ? 0 : 1;
    if (y === null) return -1;
    const comparison = typeof x === 'number' && typeof y === 'number' ? x - y : String(x).localeCompare(String(y), locale);
    return (sort.descending ? -comparison : comparison) || modelName(a).localeCompare(modelName(b),locale) || a.model.localeCompare(b.model, locale);
  });
  const labels: Record<Sort, string> = { model: t('usage.chip.model'), tier: t('usage.chip.tier'), effort: t('usage.chip.effort'), program: t('usagePerf.program'), speed: t('usagePerf.speed'), speedP90: t('usagePerf.speedP90'), firstToken: t('usagePerf.firstToken'), firstTokenP90: t('usagePerf.firstTokenP90'), requests: t('usagePerf.requests'), errors: t('usagePerf.errors') };
  const columns: Column<UsageComparison>[] = (Object.keys(labels) as Sort[]).map((key) => ({
    id: key,
    from: COMPARISON_ROOMS[key],
    header: <Button size="sm" variant="ghost" aria-label={t('usagePerf.sortBy', { column: labels[key] })} onClick={() => setSort((before) => ({ key, descending: before.key === key ? !before.descending : key !== 'model' }))}>{labels[key]}{sort.key === key ? sort.descending ? ' ↓' : ' ↑' : ''}</Button>,
    cell: (row, shown) => {
      const p = row.performance;
      const text = key === 'speed' || key === 'speedP90' ? speedText((value(row, key) as number | null) === null ? null : (value(row, key) as number) / 1000, locale, t)
        : key === 'firstToken' || key === 'firstTokenP90' ? timeText(value(row, key) as number | null, locale, t)
        : key === 'errors' ? errorsText(p,locale,t)
        : key === 'requests' ? new Intl.NumberFormat(locale).format(p.requests)
        : key === 'tier' ? row.tier===null?t('usage.key.standard'):offeredChoice(t,'serviceTier',row.tier)?.name??row.tier : key === 'effort' ? row.effort===null?t('usage.key.noEffort'):effortDisplayName(row.effort,t) : row.program ?? t('usagePerf.noProgram');
      if (key !== 'model') return <span className="tabular-nums">{text}</span>;
      const compact = [
        !shown.has(REQUESTS_COLUMN) ? t('usagePerf.compactRequests', { count: p.requests, value: new Intl.NumberFormat(locale).format(p.requests) }) : null,
        !shown.has(FIRST_COLUMN) ? t('usagePerf.compactFirst', { value: timeText(p.first_token_ms, locale, t) }) : null,
        !shown.has(ERRORS_COLUMN) ? t('usagePerf.compactErrors', { value: errorsText(p, locale, t) }) : null,
        !shown.has(SPEED_TAIL_COLUMN) ? t('usagePerf.compactSpeedP90', { value: speedText(p.speed_p90_milli === null ? null : p.speed_p90_milli / 1000, locale, t) }) : null,
        !shown.has(FIRST_TAIL_COLUMN) ? t('usagePerf.compactFirstP90', { value: timeText(p.first_token_p90_ms, locale, t) }) : null,
      ].filter((part):part is string=>part!==null);
      const choices=[
          ...(row.tier===null?[]:[{label:t('usage.chip.tier'),value:offeredChoice(t,'serviceTier',row.tier)?.name??row.tier}]),
          ...(row.effort===null?[]:[{label:t('usage.chip.effort'),value:effortDisplayName(row.effort,t)}]),
          ...(row.program===null?[]:[{label:t('usage.chip.program'),value:row.program}]),
      ];
      const filter=()=>onFilter({ model: row.model, tier: row.tier ?? 'none', effort: row.effort ?? 'none', program: row.program ?? 'none' });
      return <button type="button" onClick={filter} className="min-w-0 text-left text-accent-text hover:underline">
        <span className="block font-medium wrap-anywhere">{modelName(row)}</span>
        <span className="flex flex-col gap-1 text-xs text-text-muted">{choices.map((part)=><span key={part.label} className="flex flex-wrap gap-x-2"><span>{part.label}</span><span>{part.value}</span></span>)}</span>
        {relativeSpeed(row, rows) !== null && <span className="block text-xs text-text-muted">{t((relativeSpeed(row, rows) ?? 1) > 1 ? 'usagePerf.faster' : 'usagePerf.relativeSpeed', { ratio: new Intl.NumberFormat(locale, { maximumFractionDigits: 1 }).format(relativeSpeed(row, rows) ?? 1) })}</span>}
        {!shown.has(SPEED_COLUMN) && <span className="flex flex-wrap gap-x-2 text-sm text-text-muted"><span>{t('usagePerf.requestSpeed')}</span><span>{speedText(p.speed_milli === null ? null : p.speed_milli / 1000, locale, t)}</span></span>}
        {compact.map((part)=><span key={part} className="block text-sm text-text-muted">{part}</span>)}
      </button>;
    },
  }));
  return <Section id="usage-models" title={t('usagePerf.sections.models')} action={<>
    <Select size="sm" label={t('usagePerf.sort')} value={sort.key} onValueChange={(key)=>setSort({key,descending:key!=='model'})} options={(Object.keys(labels) as Sort[]).map((key)=>({value:key,name:t('usagePerf.choices.comparisonSort.name',{name:labels[key]}),description:t('usagePerf.choices.comparisonSort.description',{name:labels[key]})}))} />
    <Button size="sm" variant="ghost" icon={<ArrowDownUp aria-hidden />} onClick={()=>setSort((before)=>({...before,descending:!before.descending}))}>{t('usagePerf.reverseSort')}</Button>
  </>}>
    <p aria-live="polite" className="sr-only">{t('usagePerf.sorted',{column:labels[sort.key],direction:t(sort.descending ? 'usagePerf.descending' : 'usagePerf.ascending')})}</p>
    <Table label={t('usagePerf.comparison')} rows={ordered} columns={columns} rowKey={(row)=>JSON.stringify([row.model,row.tier,row.effort,row.program])} />
  </Section>;
}

/** Error codes stay actionable: selecting one opens its records in the same scope. */
export function PerformanceErrors({ performance, onOutcome }: { performance: Performance; onOutcome: (code: string) => void }) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  const errors=Object.entries(performance?.errors ?? {});
  return <Section id="usage-errors" title={t('usagePerf.errors')} className={`${boxClasses} p-4`}>
    {errors.length===0 ? <p className="text-sm text-text-muted">{t(performance===undefined || performance.requests===0 ? 'usagePerf.notMeasured' : 'usagePerf.noErrors')}</p> : <div className="flex flex-wrap gap-2">{errors.map(([code,count])=><Button key={code} size="sm" variant="ghost" onClick={()=>onOutcome(code)}>{t(`usagePerf.outcomes.${code}` as 'usagePerf.outcomes.harness_error',{defaultValue:t('usagePerf.choices.outcomes.errors.name')})} ({formatNumber(count,locale)})</Button>)}</div>}
  </Section>;
}
