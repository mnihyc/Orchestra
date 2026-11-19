/** A call exactly as its harness made it: its own name for the tool, its arguments and what came back, whole, searched within and copied.
 *
 * The third depth of a step: the call exactly as its harness made it. Its own name for the tool, the
 * arguments it passed (as a tree where they are JSON, with long strings folded and a search that
 * narrows the tree) and what came back, whole, each copyable on its own.
 */
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import type { Item } from '@/generated/api-types';
import { CodeBlock } from '@/ui/CodeBlock';
import { CopyButton } from '@/ui/CopyButton';
import { Input } from '@/ui/Input';
import { InlineAlert } from '@/ui/InlineAlert';
import { JsonTree } from '@/ui/JsonTree';

type RawItem = Extract<Item, { kind: 'tool_call' }>;

function parse(text: string): { ok: true; value: unknown } | { ok: false } {
  try {
    return { ok: true, value: JSON.parse(text) as unknown };
  } catch {
    return { ok: false };
  }
}

export function RawCall({ item }: { item: RawItem }) {
  const { t } = useTranslation();
  const [search, setSearch] = useState('');
  const raw = item.payload.raw;
  const input = parse(raw.input.inline);
  const output = raw.output;
  const result = output === null ? null : parse(output.inline);
  return (
    <div className="flex flex-col gap-3" data-raw={item.id}>
      <p className="flex items-center gap-2 text-sm">
        <span className="text-text-muted">{t('timeline.raw.tool')}</span>
        <code className="rounded-sm bg-bg-muted px-1">{raw.name}</code>
      </p>
      <Input label={t('timeline.raw.search')} hideLabel placeholder={t('timeline.raw.search')} value={search} onChange={(event) => setSearch(event.target.value)} fieldClassName="max-w-(--container-form)" />
      <section className="flex flex-col gap-1">
        <h2 className="flex items-center justify-between text-xs font-medium text-text-muted">
          {t('timeline.raw.input')}
          <CopyButton value={raw.input.inline} label={t('timeline.raw.copyInput')} />
        </h2>
        {input.ok ? <JsonTree value={input.value} search={search} label={t('timeline.raw.input')} /> : <CodeBlock code={raw.input.inline} />}
        {raw.input.truncated && <p className="text-xs text-text-subtle">{t('timeline.raw.cut')}</p>}
      </section>
      <section className="flex flex-col gap-1">
        <h2 className="flex items-center justify-between text-xs font-medium text-text-muted">
          {t('timeline.raw.output')}
          {output !== null && <CopyButton value={output.inline} label={t('timeline.raw.copyOutput')} />}
        </h2>
        {raw.is_error && <InlineAlert tone="danger">{t('timeline.raw.error')}</InlineAlert>}
        {output === null ? (
          <p className="text-sm text-text-muted">{item.status === 'in_progress' ? t('timeline.raw.running') : t('timeline.raw.noOutput')}</p>
        ) : result?.ok === true && typeof result.value === 'object' && result.value !== null ? (
          <JsonTree value={result.value} search={search} label={t('timeline.raw.output')} />
        ) : (
          <CodeBlock code={output.inline} maxHeight="24rem" />
        )}
        {output?.truncated === true && <p className="text-xs text-text-subtle">{t('timeline.raw.cut')}</p>}
      </section>
    </div>
  );
}
