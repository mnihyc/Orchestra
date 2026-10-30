/** A preset's name and public definition, or the editable labels of an imported server. */
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { ApiError } from '@/api/client';
import { createMcp, mcpKey, updateMcp } from '@/api/resource-tools';
import type { McpServer } from '@/generated/api-types';
import { EditDialog } from '@/features/resources/EditDialog';
import { adminSpec } from '@/ui/AdminAct';
import { HomePicker } from '@/features/resources/HomePicker';
import { readObject } from '@/features/resources/jsonObject';
import { useActKey } from '@/lib/forms';
import { Input } from '@/ui/Input';
import { InlineAlert } from '@/ui/InlineAlert';
import { Select } from '@/ui/Select';
import { StateLine } from '@/ui/StateLine';
import { Switch } from '@/ui/Switch';
import { Textarea } from '@/ui/Textarea';
import { chooseAuthentication, chooseTransport, connection, definitionIssue, secretWrites, strings, type Authentication, type Definition, type Transport } from './definition';

export function EditMcp({ server: offered, onClose, onCreated }: { server?: McpServer; onClose: () => void; onCreated?: (server: McpServer) => void }) {
  const { t } = useTranslation();
  const cache = useQueryClient();
  const actKey = useActKey();
  // The save names the exact revision reviewed at opening, even if the live page changes behind it.
  const [server] = useState(offered);
  const [name, setName] = useState(server?.name ?? '');
  const [description, setDescription] = useState(server?.description ?? '');
  const [home, setHome] = useState('');
  const [enabled, setEnabled] = useState(server?.enabled ?? true);
  const initial = JSON.stringify(server?.spec ?? { transport: 'http', url: '' }, null, 2);
  const [text, setText] = useState(initial);
  const [advanced, setAdvanced] = useState(false);
  const [touched, setTouched] = useState<ReadonlySet<string>>(new Set());
  const [values, setValues] = useState<Record<string, string>>({});
  const spec = readObject(text);
  const original = readObject(initial) ?? {};
  const entry = connection(spec ?? original);
  const [authentication, setAuthentication] = useState<Authentication>(connection(original).authentication);
  const issue = definitionIssue(spec);
  const imported = server === undefined ? false : server.imported !== null;
  const secrets = imported ? {} : secretWrites(spec ?? {}, values);
  const specChanged = JSON.stringify(spec) !== JSON.stringify(original);
  const retained = new Set(strings(original.secret_env));
  const visibleSecrets = authentication === 'bearer' ? [entry.tokenSecret] : authentication === 'variables' ? entry.secretNames : [];
  const needsValue = visibleSecrets.some((key) => !retained.has(key) && (values[key] ?? '').trim() === '');
  const namesMissing = authentication === 'variables' && entry.secretNames.length === 0;
  const nameValid = /^[a-z][a-z0-9_-]{1,31}$/u.test(name.trim());
  const saved = { name: server?.name, description: server?.description, enabled: server?.enabled };
  const changed = name !== saved.name || description !== saved.description || enabled !== saved.enabled || (!imported && (specChanged || Object.keys(secrets).length > 0));
  const valid = nameValid && (server !== undefined || home !== '') && (imported || spec !== null && (!specChanged && server !== undefined || issue === null) && !needsValue && !namesMissing);
  const save = useMutation({ mutationFn: () => {
    if (!valid || spec === null) throw new Error(t('resourceDetails.mcp.form.invalidDefinition'));
    const credentials = Object.keys(secrets).length === 0 ? {} : { secrets };
    const body = { name: name.trim(), description, home_id: home, spec, ...credentials };
    return server === undefined ? createMcp(body, actKey(body)) : updateMcp(server.id, { expected_revision: server.revision, name: name.trim(), description, enabled, ...credentials, ...(imported || !specChanged ? {} : { spec }) });
  }, onSuccess: (row) => { cache.setQueryData(mcpKey(row.id), row); onCreated?.(row); onClose(); } });
  const touch = (field: string) => setTouched((before) => new Set(before).add(field));
  const edit = (next: Definition) => { setText(JSON.stringify(next, null, 2)); save.reset(); };
  const changeArguments = (args: string[]) => { const next: Definition = { ...spec, command: [entry.command, ...args] }; delete next.args; edit(next); };
  // A script may itself be one multiline argument; splitting its lines would change argv.
  const separateArguments = [...connection(original).args, ...entry.args].some((argument) => /[\r\n]/u.test(argument));
  const faults = save.error instanceof ApiError ? save.error.faults() : [];
  const fieldError = (path: string) => faults.find((fault) => fault.path === path || fault.path.startsWith(path + '[') || fault.path.startsWith(path + '.'))?.message;
  const commandConnection = entry.transport === 'stdio';
  const usingToken = authentication === 'bearer';
  const usingVariables = authentication === 'variables';
  const advancedMessage = !advanced && issue?.field === 'definition' && text !== initial ? t(issue.key) : null;
  const mapped = imported ? ['name'] : ['name', commandConnection ? 'spec.command' : 'spec.url', 'spec.authentication', 'secrets', ...(usingVariables ? ['spec.secret_env'] : [])];
  const unplaced = faults.length === 0 || faults.some((fault) => !mapped.some((path) => fault.path === path || fault.path.startsWith(path + '[') || fault.path.startsWith(path + '.')));
  const fieldDisabled = save.isPending || imported || spec === null;
  const secretField = (key: string, label: string) => <Input key={key} type="password" label={label} autoComplete="new-password" spellCheck={false} value={values[key] ?? ''}
    disabled={fieldDisabled} placeholder={retained.has(key) ? t('resourceDetails.mcp.form.keepSecret') : undefined}
    onChange={(event) => { const value = event.target.value; setValues((before) => ({ ...before, [key]: value })); save.reset(); }} onBlur={() => touch('secret:' + key)}
    error={touched.has('secret:' + key) && !retained.has(key) && (values[key] ?? '').trim() === '' ? t('resourceDetails.mcp.form.secretRequired') : undefined} />;
  return <EditDialog admin={adminSpec(server, server?.environment === null ? 'mcp.manage' : 'environment.manage')} title={server === undefined ? t('resourceDetails.mcp.add') : t('resources.edit.title', { name: server.name })}
    submitLabel={server === undefined ? t('common.add') : undefined} changed={changed} valid={valid} pending={save.isPending} error={unplaced ? save.error : null} onSave={() => save.mutate()} onClose={onClose}>
    <Input label={t('resources.edit.name')} value={name} disabled={save.isPending} maxLength={32} onChange={(event) => { setName(event.target.value); save.reset(); }} onBlur={() => touch('name')}
      error={fieldError('name') ?? (touched.has('name') && !nameValid ? t('resourceDetails.mcp.form.nameInvalid') : undefined)} data-autofocus />
    {server === undefined && <HomePicker value={home} onChange={setHome} disabled={save.isPending} />}
    <Select<Transport> label={t('resourceDetails.mcp.form.connection')} value={entry.transport} disabled={fieldDisabled}
      options={[{ value: 'http', label: t('resourceDetails.mcp.form.http') }, { value: 'stdio', label: t('resourceDetails.mcp.form.stdio') }]}
      onValueChange={(transport) => { const next = chooseTransport(spec ?? {}, transport); edit(next); setAuthentication(connection(next).authentication); }} />
    {imported ? <InlineAlert>{t('resourceDetails.mcp.importedDefinition')}</InlineAlert> : <>
      {commandConnection ? <>
        <Input label={t('resourceDetails.mcp.form.command')} value={entry.command} disabled={fieldDisabled} autoComplete="off" spellCheck={false}
          onChange={(event) => { const next: Definition = { ...spec, command: [event.target.value, ...entry.args] }; delete next.args; edit(next); }} onBlur={() => touch('command')}
          error={fieldError('spec.command') ?? ((touched.has('command') || text !== initial) && issue?.field === 'command' ? t(issue.key) : undefined)} />
        {separateArguments ? <fieldset className="flex min-w-0 flex-col gap-3" aria-label={t('resourceDetails.mcp.form.arguments')}>
          {entry.args.map((argument, index) => <Textarea key={index} label={t('resourceDetails.mcp.form.argument', { number: index + 1 })} value={argument} rows={3} disabled={fieldDisabled} spellCheck={false}
            onChange={(event) => changeArguments(entry.args.map((old, at) => at === index ? event.target.value : old))} />)}
        </fieldset> : <Textarea label={t('resourceDetails.mcp.form.args')} value={entry.args.join('\n')} rows={2} disabled={fieldDisabled} spellCheck={false}
          onChange={(event) => changeArguments(event.target.value === '' ? [] : event.target.value.split(/\r?\n/u))} />}
      </> : <Input label={t('resourceDetails.mcp.form.url')} type="url" value={entry.url} disabled={fieldDisabled} autoComplete="off" spellCheck={false}
        onChange={(event) => edit({ ...spec, url: event.target.value })} onBlur={() => touch('url')}
        error={fieldError('spec.url') ?? ((touched.has('url') || text !== initial) && issue?.field === 'url' ? t(issue.key) : undefined)} />}
      <Select<Authentication> label={t('resourceDetails.mcp.form.authentication')} value={authentication} disabled={fieldDisabled}
        options={[{ value: 'none', label: t('resourceDetails.mcp.form.none') }, entry.transport === 'http' ? { value: 'bearer', label: t('resourceDetails.mcp.form.bearer') } : { value: 'variables', label: t('resourceDetails.mcp.form.variables') }]}
        error={fieldError('spec.authentication') ?? fieldError('secrets') ?? (issue?.field === 'authentication' || !usingVariables && issue?.field === 'secretNames' ? t(issue.key) : undefined)}
        onValueChange={(mode) => { setAuthentication(mode); edit(chooseAuthentication(spec ?? {}, mode)); }} />
      {usingToken && secretField(entry.tokenSecret, t('resourceDetails.mcp.form.token'))}
      {usingVariables && <>
        <Textarea label={t('resourceDetails.mcp.form.variableNames')} value={entry.secretNames.join('\n')} rows={2} disabled={fieldDisabled} spellCheck={false}
          onChange={(event) => edit({ ...spec, secret_env: event.target.value.split(/\r?\n/u).map((value) => value.trim()).filter((value) => value !== '') })} onBlur={() => touch('secretNames')}
          error={fieldError('spec.secret_env') ?? (issue?.field === 'secretNames' ? t(issue.key) : touched.has('secretNames') && namesMissing ? t('resourceDetails.mcp.form.variablesRequired') : undefined)} />
        {[...new Set(entry.secretNames)].map((key) => secretField(key, key))}
      </>}
    </>}
    {advancedMessage !== null && <StateLine role="alert" tone="danger" compact message={advancedMessage} />}
    <details open={advanced} onToggle={(event) => setAdvanced(event.currentTarget.open)}>
      <summary className="cursor-pointer rounded-md text-sm text-text-muted focus-visible:outline-2 focus-visible:outline-accent">{t('resourceDetails.mcp.form.advanced')}</summary>
      {advanced && <div className="mt-3 flex min-w-0 flex-col gap-3">
        <Textarea label={t('resources.listDescription')} value={description} rows={2} disabled={save.isPending} onChange={(event) => { setDescription(event.target.value); save.reset(); }} />
        <Textarea label={t('resourceDetails.mcp.form.json')} value={text} rows={12} disabled={save.isPending} readOnly={imported} className="font-mono"
          onChange={(event) => { const next = event.target.value; setText(next); save.reset(); const parsed = readObject(next); if (parsed !== null) setAuthentication(connection(parsed).authentication); }}
          error={issue?.field === 'definition' ? t(issue.key) : undefined} />
      </div>}
    </details>
    {server !== undefined && <Switch label={t('resourceDetails.enabled')} checked={enabled} disabled={save.isPending} onCheckedChange={setEnabled} />}
  </EditDialog>;
}
