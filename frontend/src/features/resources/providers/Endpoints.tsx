/** Endpoints of an API provider with the options their format declares, presets, and the secrets of an endpoint.
 *
 * One row for each endpoint: a format, which the system names, and an address, with a line under it that
 * says which agents can use it as entered. Under "Advanced" are the options that format declares (how
 * the key is sent, extra headers, headers that are secrets, query parameters), drawn from the
 * declaration; nothing here knows a vendor or a format. An address that starts with `http://` is refused
 * unless the org allows it, and then says that the key will travel unencrypted. A secret header's value
 * is written once and never shown again: the drafts keep its name and what was typed this time.
 */
import type { TFunction } from 'i18next';
import { Plus, Trash2 } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { useViewChoice } from '@/lib/viewChoices';
import { Hint } from '@/features/help/Hint';
import { DeclaredHint } from '@/features/resources/Hint';
import type { Field, ProviderSetupProtocol, Upstream } from '@/generated/api-types';
import { Button } from '@/ui/Button';
import { FieldForm, type FormValue, type FormValues, initialValues } from '@/ui/FieldForm';
import { IconButton } from '@/ui/IconButton';
import { InlineAlert } from '@/ui/InlineAlert';
import { Input } from '@/ui/Input';
import { Select } from '@/ui/Select';

/** An endpoint as it is being edited. */
export interface EndpointDraft {
  /** Tells a row from the others while its protocol or address changes. */
  id: string;
  protocol: string;
  baseUrl: string;
  /** The values of the format's options, by the name each is declared under. */
  options: FormValues;
}

/** How an address begins: what a field for one shows while it is empty. */
const ADDRESS_START = 'https://';

let nextId = 0;
const newId = (): string => `endpoint-${(nextId += 1)}`;

/** A row for a protocol, with the options it declares at their starting values. */
export function newEndpoint(protocol: ProviderSetupProtocol | undefined, baseUrl = ''): EndpointDraft {
  return { id: newId(), protocol: protocol?.name ?? '', baseUrl, options: protocol === undefined ? {} : initialValues(protocol.endpoint_options) };
}

/** A row for an endpoint that exists already; a secret header keeps its name and has no value to show. */
export function draftOf(upstream: Upstream, protocol: ProviderSetupProtocol | undefined): EndpointDraft {
  const options: FormValues = protocol === undefined ? {} : initialValues(protocol.endpoint_options);
  const held = typeof upstream.options === 'object' && upstream.options !== null ? (upstream.options as Record<string, unknown>) : {};
  for (const field of protocol?.endpoint_options ?? []) {
    const value = held[field.name];
    if (field.type === 'secret_map') {
      options[field.name] = Array.isArray(value) ? Object.fromEntries(value.filter((name): name is string => typeof name === 'string').map((name) => [name, ''])) : {};
    } else if (field.type === 'map') {
      options[field.name] = typeof value === 'object' && value !== null && !Array.isArray(value) ? Object.fromEntries(Object.entries(value).map(([name, text]) => [name, String(text)])) : {};
    } else if (typeof value === 'string') {
      options[field.name] = value;
    }
  }
  return { id: newId(), protocol: upstream.protocol, baseUrl: upstream.base_url, options };
}

/** The endpoint a draft stands for, and the values of its secret headers by name, which go beside it and never into it. */
export function upstreamOf(draft: EndpointDraft, protocol: ProviderSetupProtocol | undefined): { upstream: Upstream; secrets: Record<string, string> } {
  const options: Record<string, unknown> = {};
  const secrets: Record<string, string> = {};
  for (const field of protocol?.endpoint_options ?? []) {
    const value: FormValue | undefined = draft.options[field.name];
    if (value === undefined) continue;
    if (field.type === 'secret_map' && typeof value === 'object') {
      const names = Object.keys(value).filter((name) => name !== '');
      if (names.length > 0) options[field.name] = names;
      for (const [name, text] of Object.entries(value)) if (name !== '' && text !== '') secrets[name] = text;
    } else if (typeof value === 'object') {
      if (Object.keys(value).length > 0) options[field.name] = Object.fromEntries(Object.entries(value).filter(([name]) => name !== ''));
    } else if (typeof value === 'string' && value !== '') {
      options[field.name] = value;
    } else if (typeof value === 'boolean' && value) {
      options[field.name] = true;
    }
  }
  return { upstream: { protocol: draft.protocol, base_url: draft.baseUrl.trim(), options }, secrets };
}

/** The exact runtime loopback addresses allowed by the server without sending clear text on a network. */
export function runtimeLoopback(address: string): boolean {
  let parsed: URL;
  try { parsed = new URL(address); } catch { return false; }
  if (parsed.username !== '' || parsed.password !== '') return false;
  const host = /^http:\/\/(\[[^\]]+\]|[^/:?#]+)(?::[0-9]+)?(?:\/|$)/i.exec(address)?.[1];
  if (host === undefined) return false;
  if (host.toLowerCase() === 'localhost') return true;
  if (host.startsWith('[')) return parsed.hostname === '[::1]';
  const parts = host.split('.');
  return parts.length === 4 && parts[0] === '127' && parts.every((part) => /^(0|[1-9][0-9]*)$/.test(part) && Number(part) <= 255);
}

/** What is wrong with a row, in words: an address that is not one, or that would send the key in clear where that is not allowed. */
export function endpointFault(draft: EndpointDraft, httpAllowed: boolean, t: TFunction): string | null {
  const address = draft.baseUrl.trim();
  if (draft.protocol === '') return t('resources.endpoint.protocolEmpty');
  if (address === '') return t('resources.endpoint.addressEmpty');
  let parsed: URL;
  try {
    parsed = new URL(address);
  } catch {
    return t('resources.endpoint.addressInvalid');
  }
  if (parsed.protocol === 'http:' && !httpAllowed && !runtimeLoopback(address)) return t('resources.endpoint.httpRefused');
  if (parsed.protocol !== 'http:' && parsed.protocol !== 'https:') return t('resources.endpoint.addressInvalid');
  return null;
}

export interface EndpointsEditorProps {
  value: readonly EndpointDraft[];
  onChange: (value: EndpointDraft[]) => void;
  protocols: readonly ProviderSetupProtocol[];
  httpAllowed: boolean;
  /** What is wrong with each row, by its id. */
  errors?: Readonly<Record<string, string>>;
  disabled?: boolean;
  /** Creation shows the interface compactly; credential values are written afterwards. */
  compact?: boolean;
}

/** Server paths identify draft fields; only their error messages are shown to a person. */
function endpointErrors(id: string, errors: Readonly<Record<string, string>>) {
  const optionPrefix = `${id}.options.`;
  return {
    protocol: errors[`${id}.protocol`],
    address: errors[id] ?? errors[`${id}.base_url`],
    options: errors[`${id}.options`],
    fields: Object.fromEntries<string>(Object.entries(errors).flatMap(([key, message]): [string, string][] =>
      key.startsWith(optionPrefix) ? [[key.slice(optionPrefix.length), message]] : [])),
  };
}

export function EndpointsEditor({ value, onChange, protocols, httpAllowed, errors = {}, disabled = false, compact = false }: EndpointsEditorProps) {
  const { t } = useTranslation();
  const [advanced, chooseAdvanced] = useViewChoice<boolean>('resource-providers', 'endpointAdvanced', false);
  const used = new Set(value.map((row) => row.protocol));
  const spare = protocols.filter((protocol) => !used.has(protocol.name));
  const change = (id: string, patch: Partial<EndpointDraft>) => onChange(value.map((row) => (row.id === id ? { ...row, ...patch } : row)));

  return (
    <div className="flex flex-col gap-4">
      {value.map((row, index) => {
        const protocol = protocols.find((candidate) => candidate.name === row.protocol);
        const fields: readonly Field[] = (protocol?.endpoint_options ?? []).filter((field) => !compact || field.name !== 'auth' && field.type !== 'secret' && field.type !== 'secret_text' && field.type !== 'secret_map');
        const agents = protocol?.agents ?? [];
        const clear = row.baseUrl.trim().toLowerCase().startsWith('http://') && !runtimeLoopback(row.baseUrl.trim());
        const { fields: optionErrors, ...rowErrors } = endpointErrors(row.id, errors);
        const otherOptions=Object.entries(optionErrors).filter(([name])=>!fields.some((field)=>field.name===name)).map(([,message])=>message);
        return (
          <fieldset key={row.id} className={compact ? 'flex min-w-0 flex-col gap-2' : 'flex flex-col gap-3 rounded-lg border border-border bg-surface p-3'} disabled={disabled}>
            {value.length > 1 && <legend className="px-1 text-sm font-medium text-text-muted">{t('resources.endpoint.row', { n: index + 1 })}</legend>}
            <div className="flex flex-wrap items-start gap-3">
              <Select
                label={t('resources.endpoint.protocol')}
                error={rowErrors.protocol}
                help={<Hint id="endpoint-format" />}
                value={row.protocol}
                placeholder={t('resources.endpoint.protocolChoose')}
                onValueChange={(next) => {
                  const chosen = protocols.find((candidate) => candidate.name === next);
                  change(row.id, { protocol: next, options: chosen === undefined ? {} : initialValues(chosen.endpoint_options) });
                }}
                options={protocols
                  .filter((candidate) => candidate.name === row.protocol || !used.has(candidate.name))
                  .map((candidate) => ({ value: candidate.name, label: candidate.title }))}
                fieldClassName="w-full sm:w-[14rem]"
              />
              <Input
                label={t('resources.endpoint.address')}
                mono
                value={row.baseUrl}
                error={rowErrors.address}
                placeholder={ADDRESS_START}
                onChange={(event) => change(row.id, { baseUrl: event.target.value })}
                fieldClassName="min-w-0 flex-1 basis-[15rem]"
                autoComplete="off"
                inputMode="url"
              />
              {value.length > 1 && (
                <IconButton
                  label={t('resources.endpoint.remove', { n: index + 1 })}
                  icon={<Trash2 aria-hidden />}
                  className="mt-6"
                  onClick={() => onChange(value.filter((other) => other.id !== row.id))}
                />
              )}
            </div>
            {clear && httpAllowed && <InlineAlert tone="warning">{t('resources.endpoint.clear')}</InlineAlert>}
            {protocol !== undefined && <p className="text-sm text-text-muted">
              {agents.length === 0
                ? t('resources.endpoint.noAgent')
                : agents.length === 1
                  ? t('resources.endpoint.oneAgent', { agents: agents.map((agent) => agent.name).join(', ') })
                  : t('resources.endpoint.manyAgents', { agents: agents.map((agent) => agent.name).join(', ') })}
            </p>}
            {fields.length > 0 && (
              <details className="group" open={advanced || Object.keys(optionErrors).length>0} onToggle={(event) => { if (event.currentTarget.open !== advanced) chooseAdvanced(event.currentTarget.open); }}>
                <summary className="cursor-default rounded-md text-sm font-medium text-accent-text select-none hover:underline">{t('resources.endpoint.advanced')}</summary>
                <FieldForm className="pt-3" fields={fields} values={row.options} errors={optionErrors} helpFor={(field) => <DeclaredHint term={field.label} help={field.help} />} onChange={(name, next) => change(row.id, { options: { ...row.options, [name]: next } })} />
              </details>
            )}
            {(rowErrors.options!==undefined || otherOptions.length>0) && <InlineAlert tone="warning">{[rowErrors.options,...otherOptions].filter((message)=>message!==undefined).join(' ')}</InlineAlert>}
          </fieldset>
        );
      })}
      {spare.length > 0 && (
        <div>
          <Button size="sm" icon={<Plus aria-hidden />} disabled={disabled} onClick={() => onChange([...value, newEndpoint(spare[0])])}>
            {t('resources.endpoint.add')}
          </Button>
        </div>
      )}
    </div>
  );
}
