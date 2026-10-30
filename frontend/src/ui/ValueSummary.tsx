/** Read-only structured values use words and chips, never a serialized JSON box. */
import { useTranslation } from 'react-i18next';
import { Badge } from '@/ui/Badge';
import { PREVIEW_LINES, Preview, TextPreview } from '@/ui/Preview';

const record = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value);
const fieldWords = (name: string) => name.replaceAll('_', ' ').replaceAll('.', ' ');
export interface ValueSummaryProps {
  value: unknown;
  label?: string;
  word?: (value: string) => string;
  fieldName?: (name: string) => string;
  number?: (value: number) => string;
  /** A summary which itself opens an editor must contain no further buttons or block lists. */
  inControl?: boolean;
}

export function ValueSummary({ value, label, word = (text) => text, fieldName = fieldWords, number = String, inControl = false }: ValueSummaryProps) {
  const { t } = useTranslation();
  const child = (entry: unknown) => <ValueSummary value={entry} word={word} fieldName={fieldName} number={number} />;
  if (value === null || value === undefined || value === '') return <span className="text-text-muted">{t('day100.settings.empty')}</span>;
  if (typeof value === 'boolean') return <span>{t(value ? 'common.on' : 'common.off')}</span>;
  if (typeof value === 'number') return <span>{number(value)}</span>;
  if (typeof value === 'string') return inControl ? <span className="min-w-0 text-sm wrap-anywhere">{word(value)}</span> : <TextPreview value={word(value)} className="min-w-0 text-sm wrap-anywhere" />;
  const short = (entry: unknown) => typeof entry === 'string' ? word(entry) : typeof entry === 'number' ? number(entry)
    : typeof entry === 'boolean' ? t(entry ? 'common.on' : 'common.off') : Array.isArray(entry) ? t('settings.compact.items', { count: entry.length })
      : record(entry) ? t('settings.compact.fields', { count: Object.keys(entry).length }) : t('day100.settings.empty');
  if (Array.isArray(value)) {
    const values: readonly unknown[] = value;
    if (values.length === 0) return <span className="text-text-muted">{t('day100.settings.empty')}</span>;
    if (inControl) return <span className="inline-flex min-w-0 max-w-full flex-wrap gap-1">{values.slice(0, PREVIEW_LINES).map((entry, index) => <Badge key={index} className="max-w-full"><span className="min-w-0 wrap-anywhere whitespace-normal">{short(entry)}</span></Badge>)}
      {values.length > PREVIEW_LINES && <span>{t('settings.compact.moreValues', { count: values.length - PREVIEW_LINES })}</span>}</span>;
    return <Preview more={values.length > PREVIEW_LINES}>{(expanded) => <ul aria-label={label} className="flex min-w-0 max-w-full flex-wrap gap-1">
      {(expanded ? values : values.slice(0, PREVIEW_LINES)).map((entry, index) => <li key={index} className="min-w-0 max-w-full">
        {entry !== null && typeof entry === 'object' ? child(entry) : <Badge className="max-w-full"><span className="min-w-0 wrap-anywhere whitespace-normal">{typeof entry === 'string' ? word(entry) : typeof entry === 'number' ? number(entry) : typeof entry === 'boolean' ? t(entry ? 'common.on' : 'common.off') : t('day100.settings.empty')}</span></Badge>}
      </li>)}
    </ul>}</Preview>;
  }
  if (record(value)) {
    const entries = Object.entries(value);
    if (entries.length === 0) return <span className="text-text-muted">{t('screenReview.emptyObject')}</span>;
    if (inControl) return <span className="inline-flex min-w-0 flex-wrap gap-x-3 gap-y-1 text-sm">{entries.slice(0, PREVIEW_LINES).map(([name, entry]) => <span key={name} className="min-w-0 wrap-anywhere"><span className="font-normal">{fieldName(name)}:</span>{' '}{short(entry)}</span>)}
      {entries.length > PREVIEW_LINES && <span>{t('settings.compact.moreValues', { count: entries.length - PREVIEW_LINES })}</span>}</span>;
    return <Preview more={entries.length > PREVIEW_LINES}>{(expanded) => <dl aria-label={label} className="grid min-w-0 max-w-full grid-cols-[auto_minmax(0,1fr)] gap-x-3 gap-y-1 text-sm">
      {(expanded ? entries : entries.slice(0, PREVIEW_LINES)).map(([name, entry]) => <div key={name} className="contents">
        <dt className="min-w-0 wrap-anywhere text-text-muted">{fieldName(name)}</dt><dd className="min-w-0 wrap-anywhere">{child(entry)}</dd>
      </div>)}
    </dl>}</Preview>;
  }
  return <span className="text-text-muted">{t('day100.settings.empty')}</span>;
}
