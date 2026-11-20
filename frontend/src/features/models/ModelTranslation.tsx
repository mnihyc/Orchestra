/** The selected conversion and its supplier-declared fidelity, opened on the value itself. */
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import type { TFunction } from 'i18next';
import type { ProtocolTranslation } from '@/generated/api-types';
import { Popover } from '@/ui/Popover';

const record = (value: unknown): value is Record<string, unknown> => value !== null && typeof value === 'object' && !Array.isArray(value);

/** A malformed or absent capture supplies no substitute facts. */
export function translationOf(value: unknown): ProtocolTranslation | null {
  if (!record(value) || typeof value.from !== 'string' || typeof value.to !== 'string'
    || typeof value.title !== 'string' || !record(value.losses)) return null;
  const strings = (value: unknown): value is string[] => Array.isArray(value) && value.every((line) => typeof line === 'string');
  if (!strings(value.losses.preserved) || !strings(value.losses.approximated) || !strings(value.losses.dropped)) return null;
  return { from: value.from, to: value.to, title: value.title,
    losses: { preserved: value.losses.preserved, approximated: value.losses.approximated, dropped: value.losses.dropped } };
}

/** Names the interface, independently of the vendor behind that endpoint. */
export function interfaceName(protocol: string, t: TFunction): string {
  const names: Record<string, string> = {
    common: t('providerFeatures.common'),
    'openai-responses': t('modelTranslation.names.responses'),
    'anthropic-messages': t('modelTranslation.names.messages'),
    'openai-chat': t('modelTranslation.names.chat'),
    'openai-embeddings': t('modelTranslation.names.embeddings'),
  };
  return names[protocol] ?? protocol;
}

function lossWords(t: TFunction): Record<string,string> {
  return {
    'Visible reasoning summaries survive an upstream change; incompatible opaque state is omitted.': t('modelTranslation.foreign'),
    'Text, image inputs, tool identifiers and upstream token usage.': t('modelTranslation.preserved'),
    'Signed reasoning is carried opaquely and restored on the next turn.': t('modelTranslation.reasoning'),
    'Rate limits, overload, output limits and interrupted streams retain their meaning.': t('modelTranslation.failures'),
    'Later system instructions become instructions in the following user turn.': t('modelTranslation.instructions'),
    'Custom tool grammars become a single string argument; the upstream does not enforce the grammar.': t('modelTranslation.custom'),
    'Prefix caching uses system and newest-turn cache breakpoints.': t('modelTranslation.cacheMarkers'),
    'Unpaired tool notifications become user input with their source name.': t('modelTranslation.notifications'),
    'Messages from other agents become user input carrying author and recipient.': t('modelTranslation.agentMessages'),
    'Reasoning effort uses the offering\'s declared thinking controls.': t('modelTranslation.effort'),
    'Compaction produces a text summary carried as opaque conversation state.': t('modelTranslation.compaction'),
    'Images in a tool result follow the tool result as user image inputs.': t('modelTranslation.toolImages'),
    'Explicit cache breakpoints become a stable session cache key when one is provided.': t('modelTranslation.cacheKey'),
    'Thinking budgets use the offering\'s declared reasoning effort.': t('modelTranslation.budget'),
    'Vendor server tools are not translated; use Orch\'s capability routes.': t('modelTranslation.serverTools'),
    'Stop sequences and sampling controls without an outgoing equivalent.': t('modelTranslation.sampling'),
    'Unrecognized beta headers are not forwarded across interfaces.': t('modelTranslation.beta'),
  };
}

export function ModelTranslation({ value }: { value: unknown }) {
  const { t } = useTranslation();
  const [search, setSearch] = useState('');
  const translation = translationOf(value);
  if (translation === null) return null;
  const label = t('modelTranslation.route', { from: interfaceName(translation.from,t), to: interfaceName(translation.to,t) });
  const words = lossWords(t);
  const groups = [
    { key:'preserved',title:t('modelTranslation.kept'),values:translation.losses.preserved },
    { key:'approximated',title:t('modelTranslation.approximate'),values:translation.losses.approximated },
    { key:'dropped',title:t('modelTranslation.omitted'),values:translation.losses.dropped },
  ].map((group) => ({ ...group, values:group.values.map((line) => words[line.replaceAll('’',"'")] ?? line) }));
  const count = groups.reduce((count,group) => count+group.values.length,0);
  const term = search.trim().toLocaleLowerCase();
  return <Popover label={label} onOpenChange={(open) => { if (!open) setSearch(''); }}
    trigger={<button type="button" className="max-w-full truncate text-start text-xs text-text-muted underline decoration-dotted underline-offset-2 hover:text-text focus-visible:outline-offset-2">{label}</button>}>
    <div className="flex max-w-sm flex-col gap-2 text-sm">
      {count > 10 && <input type="search" className="rounded border border-border px-2 py-1" aria-label={t('modelTranslation.search')} value={search} onChange={(event) => setSearch(event.target.value)} />}
      <dl className="flex flex-col gap-2">
        {groups.map((group) => {
          const values = group.values.filter((line) => line.toLocaleLowerCase().includes(term));
          return values.length === 0 ? null : <div key={group.key}>
            <dt className="font-medium">{group.title}</dt>
            <dd><ul className="list-disc space-y-1 ps-4 text-text-muted">{values.map((line,index) => <li key={index}>{line}</li>)}</ul></dd>
          </div>;
        })}
      </dl>
    </div>
  </Popover>;
}
