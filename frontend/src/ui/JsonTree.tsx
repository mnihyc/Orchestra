/** Shared JsonTree component (content); primitives are wrapped here.
 *
 * Something structured shown as a tree: its keys and values, objects and lists that open and close,
 * long strings folded to their first lines with a way to read the rest, and every member copyable on
 * its own. A search narrows it to the branches that hold what was looked for.
 */
import { ChevronDown, ChevronRight } from 'lucide-react';
import { memo, useId, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { CopyButton } from '@/ui/CopyButton';
import { cx } from '@/ui/cx';
import { PREVIEW_LINES, Preview } from '@/ui/Preview';

/** A string longer than this is folded: the rest is behind "Show all". */
export const STRING_FOLDED_AT = 240;
/** Objects and lists nested deeper than this begin closed. */
const OPEN_DEPTH = 2;

type Json = unknown;

const isObject = (value: Json): value is Record<string, Json> => typeof value === 'object' && value !== null && !Array.isArray(value);

/** Whether a value, or anything inside it, has this text in a key or a value. */
export function holds(value: Json, key: string | null, needle: string): boolean {
  if (needle === '') return true;
  const lower = needle.toLowerCase();
  if (key?.toLowerCase().includes(lower) === true) return true;
  if (Array.isArray(value)) return value.some((entry, index) => holds(entry, String(index), lower));
  if (isObject(value)) return Object.entries(value).some(([name, entry]) => holds(entry, name, lower));
  return String(value).toLowerCase().includes(lower);
}

function text(value: Json): string {
  return value === undefined || typeof value === 'string' || typeof value === 'function' || typeof value === 'symbol' || typeof value === 'bigint'
    ? String(value) : JSON.stringify(value, null, 2);
}

function Member({ name, path, value, depth, needle }: { name: string | null; path: string; value: Json; depth: number; needle: string }) {
  const { t } = useTranslation();
  const [opened, setOpened] = useState<boolean | null>(null);
  const [whole, setWhole] = useState(false);
  const childrenId = useId();
  if (needle !== '' && !holds(value, name, needle)) return null;
  const branch = Array.isArray(value) || isObject(value);
  const open = needle !== '' ? true : (opened ?? depth < OPEN_DEPTH);
  const entries: [string, Json][] = Array.isArray(value) ? value.map((entry, index) => [String(index), entry]) : isObject(value) ? Object.entries(value) : [];

  return (
    <li className="group/member">
      <div className="flex min-w-0 items-start gap-1">
        {branch ? (
          <button
            type="button"
            aria-expanded={open}
            aria-controls={childrenId}
            aria-label={t(open ? 'ui.json.collapse' : 'ui.json.expand', { name: path })}
            onClick={() => setOpened(!open)}
            className="focus-inset inline-flex size-6 shrink-0 items-center justify-center rounded-sm text-text-muted hover:bg-bg-muted"
          >
            {open ? <ChevronDown aria-hidden /> : <ChevronRight aria-hidden />}
          </button>
        ) : (
          <span className="size-6 shrink-0" />
        )}
        <div className="min-w-0 flex-1">
          {name !== null && <span className="mr-1 text-text-muted">{name}:</span>}
          {branch ? (
            <span className="text-text-subtle">
              {Array.isArray(value) ? t('ui.json.items', { count: entries.length }) : t('ui.json.keys', { count: entries.length })}
            </span>
          ) : typeof value === 'string' ? (
            <span data-overflow-text={value.length > STRING_FOLDED_AT && !whole ? value : undefined} className="wrap-anywhere whitespace-pre-wrap text-text">
              {value.length > STRING_FOLDED_AT && !whole ? (
                <>{`“${value.slice(0, STRING_FOLDED_AT)}`}<button type="button" data-overflow-text={value} aria-label={t('ui.json.showAll', { count: value.length })}
                  className="focus-inset inline-flex min-h-6 max-w-full items-center rounded-sm text-left text-accent-text underline" onClick={() => setWhole(true)}>
                  {`…” ${t('ui.json.showAll', { count: value.length })}`}
                </button></>
              ) : (
                <>{`“${value}”`}</>
              )}
            </span>
          ) : (
            <span className={cx(typeof value === 'number' || typeof value === 'boolean' ? 'text-info' : 'text-text-subtle')}>{String(value)}</span>
          )}
        </div>
        <span className="opacity-0 transition-opacity group-focus-within/member:opacity-100 group-hover/member:opacity-100">
          <CopyButton value={text(value)} label={name === null ? t('ui.json.copy') : t('ui.json.copyNamed', { name: path })} />
        </span>
      </div>
      {branch && (
        <ul id={childrenId} hidden={!open} className="ml-2 border-l border-border pl-2">
          {open && entries.map(([key, entry]) => (
            <Member key={key} name={key} path={`${path}.${key}`} value={entry} depth={depth + 1} needle={needle} />
          ))}
        </ul>
      )}
    </li>
  );
}

export interface JsonTreeProps {
  value: Json;
  /** Narrows the tree to what holds this text. */
  search?: string;
  /** What the tree is, for a screen reader. */
  label: string;
  className?: string;
  /** Large inspector values start with the shared six-line preview; expanding never adds a scroller. */
  preview?: boolean;
}

function JsonTreeView({ value, search = '', label, className, preview = false }: JsonTreeProps) {
  const tree = (
    <ul aria-label={label} className={cx('font-mono text-sm', className)}>
      <Member name={null} path={label} value={value} depth={0} needle={search} />
    </ul>
  );
  if (!preview || search !== '') return tree;
  const lines = text(value).split('\n');
  if (lines.length <= PREVIEW_LINES) return tree;
  return <Preview more>{(expanded) => expanded ? tree : <pre role="region" tabIndex={0} aria-label={label}
    className={cx('focus-inset whitespace-pre-wrap wrap-anywhere font-mono text-sm', className)}>{lines.slice(0, PREVIEW_LINES).join('\n')}</pre>}</Preview>;
}

export const JsonTree = memo(JsonTreeView);
