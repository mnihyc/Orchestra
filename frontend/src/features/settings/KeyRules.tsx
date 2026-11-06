/** One existing key form: explicit targets, resource filters and exact operation/action selections. */
import { useQuery } from '@tanstack/react-query';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { tokenOperationsQuery } from '@/api/users';
import type { NameKind } from '@/api/search';
import { NamePicker } from '@/features/search/NamePicker';
import { CurrentDetailPage } from '@/features/resources/CurrentDetailPage';
import type { ApiTokenInfo, KeyAction, KeyOperation, KeyRule, KeyTarget, KeyTargetKind, Operation } from '@/generated/api-types';
import { Button } from '@/ui/Button';
import { Checkbox } from '@/ui/Checkbox';
import { Dialog } from '@/ui/Dialog';
import { ErrorState } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';
import { Input } from '@/ui/Input';
import { Select } from '@/ui/Select';
import { SkeletonRows } from '@/ui/Skeleton';
import { hasKeySubtree, KEY_ACTIONS, offeredActions, presetOperations, ruleOperations, selectOperations } from './keyRules';

const PRESETS = [
  { preset: 'read', actions: ['read'] },
  { preset: 'write', actions: ['write'] },
  { preset: 'readWrite', actions: ['read', 'write'] },
  { preset: 'manage', actions: KEY_ACTIONS },
] as const satisfies readonly { preset: string; actions: readonly KeyAction[] }[];
const NAME_KINDS: Partial<Record<KeyTargetKind, NameKind>> = {
  org: 'org', project: 'project', topic: 'topic', thread: 'thread', document: 'document', document_container: 'document_container', chat: 'chat',
  provider: 'provider', agent: 'agent', environment: 'environment', host: 'host', machine: 'machine', template: 'template',
  mcp_server: 'mcp_server', mount: 'mount', skill: 'skill', group: 'group', account: 'user',
};
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

export function KeyRuleSummary({ rule }: { rule: KeyRule }) {
  const { t } = useTranslation();
  return <div className="flex min-w-0 flex-col gap-1 text-sm">
    <p className="wrap-anywhere"><span className="font-medium">{t(`operationKeys.kind.${rule.target.kind}`)}</span>{rule.target.id !== undefined && rule.target.id !== null && <> · <code>{rule.target.id}</code></>} · {t(`operationKeys.reach.${rule.target.reach}`)}</p>
    <p className="text-text-muted">{rule.resources === undefined || rule.resources === null ? t('operationKeys.allResources') : rule.resources.length === 0 ? t('operationKeys.noResources') : rule.resources.map((kind) => t(`operationKeys.kind.${kind}`)).join(', ')}</p>
    <CurrentDetailPage count={rule.allow.length}>{(from, to) => <ul className="flex flex-col gap-1">{rule.allow.slice(from, to).map((item) => <li key={item.operation} className="wrap-anywhere"><code>{item.operation}</code> · {item.actions.map((action) => t(`operationKeys.action.${action}`)).join(', ')}</li>)}</ul>}</CurrentDetailPage>
  </div>;
}

/** Server-retained ceilings are read-only, and never presented as an extra requested grant. */
export function KeyScopeReview({ token }: { token: ApiTokenInfo }) {
  const { t } = useTranslation();
  return <div className="flex min-w-0 flex-col gap-3">
    <p className="text-sm text-text-muted">{t('operationKeys.currentRights')}</p>
    {token.rules === undefined || token.rules === null ? <><InlineAlert tone="warning">{t('operationKeys.legacy')}</InlineAlert><ul className="text-sm">{token.scopes.map((scope) => <li key={scope}><code>{scope}</code></li>)}</ul></>
      : token.rules.length === 0 ? <InlineAlert>{t('operationKeys.denyAll')}</InlineAlert>
      : token.rules.map((rule, index) => <KeyRuleSummary key={index} rule={rule} />)}
    {(token.constraints?.length ?? 0) > 0 && <details className="text-sm"><summary>{t('operationKeys.ceilings')}</summary><p className="my-2 text-text-muted">{t('operationKeys.ceilingsHint')}</p>
      {token.constraints?.map((group, index) => <div key={index} className="my-2 flex flex-col gap-2 rounded-md border border-border p-2">{group.length === 0 ? t('operationKeys.denyAll') : group.map((rule, child) => <KeyRuleSummary key={child} rule={rule} />)}</div>)}
    </details>}
    {(token.excluded_places?.length ?? 0) > 0 && <details className="text-sm"><summary>{t('operationKeys.placeLimits')}</summary><ul>{token.excluded_places?.map((limit, index) => <li key={index} className="wrap-anywhere"><code>{limit.place}</code> · {limit.reason === 'refused' ? t('operationKeys.refused') : t('operationKeys.maxDays', { count: limit.max_days })}</li>)}</ul></details>}
  </div>;
}

export function KeyOperationChoice({ catalogue, resources, value, onChange }: {
  catalogue: readonly Operation[]; resources: readonly KeyTargetKind[]; value: readonly KeyOperation[]; onChange: (next: KeyOperation[]) => void;
}) {
  const { t } = useTranslation();
  const [query, setQuery] = useState('');
  const rows = ruleOperations(catalogue, resources);
  const needle = query.trim().toLocaleLowerCase();
  const filtered = rows.filter((operation) => `${operation.title} ${operation.operation_id} ${operation.variants.map((variant) => variant.operation).join(' ')}`.toLocaleLowerCase().includes(needle));
  const selected = new Map(value.map((item) => [item.operation, new Set(item.actions)]));
  const offered = new Map(rows.flatMap((operation) => operation.variants.map((variant) => [variant.operation, offeredActions(variant, resources)] as const)));
  const unavailable = value.map((item) => ({ ...item, actions: item.actions.filter((action) => !offered.get(item.operation)?.includes(action)) })).filter((item) => item.actions.length > 0);
  return <div className="flex min-w-0 flex-col gap-3">
    <div className="flex flex-wrap gap-2">{PRESETS.map(({ preset, actions }) => <Button key={preset} size="sm" onClick={() => onChange(presetOperations(catalogue, resources, actions))}>{t(`operationKeys.preset.${preset}`)}</Button>)}</div>
    <Input label={t('operationKeys.search')} value={query} onChange={(event) => setQuery(event.target.value)} />
    <p className="text-sm text-text-muted">{t('operationKeys.exactReview')}</p>
    {unavailable.length > 0 && <InlineAlert tone="warning">{t('operationKeys.unavailable')}<ul>{unavailable.map((item) => <li key={item.operation} className="wrap-anywhere"><code>{item.operation}</code> · {item.actions.map((action) => t(`operationKeys.action.${action}`)).join(', ')} <Button size="sm" variant="ghost" aria-label={t('operationKeys.removeUnavailableFor', { operation: item.operation })} onClick={() => onChange(value.flatMap((other) => {
      if (other.operation !== item.operation) return [other];
      const actions = other.actions.filter((action) => !item.actions.includes(action));
      return actions.length === 0 ? [] : [{ ...other, actions }];
    }))}>{t('operationKeys.removeUnavailable')}</Button></li>)}</ul></InlineAlert>}
    <CurrentDetailPage count={filtered.length}>{(from, to) => <div className="flex flex-col gap-3">{filtered.slice(from, to).map((operation) => {
      const all = operation.variants.every((variant) => offeredActions(variant, resources).every((action) => selected.get(variant.operation)?.has(action) === true));
      return <fieldset key={operation.operation_id} className="flex min-w-0 flex-col gap-2 rounded-md border border-border p-3">
        <legend className="max-w-full px-1"><Checkbox label={operation.title} hint={operation.operation_id} checked={all} onCheckedChange={(checked) => onChange(selectOperations(value, operation.variants, resources, KEY_ACTIONS, checked))} /></legend>
        {operation.variants.map((variant) => <div key={variant.operation} className="flex min-w-0 flex-col gap-1">
          <p className="font-mono text-xs wrap-anywhere">{variant.operation}</p>
          <div className="flex flex-wrap gap-3">{offeredActions(variant, resources).map((action) => <Checkbox key={action} label={t(`operationKeys.action.${action}`)} checked={selected.get(variant.operation)?.has(action) === true}
            onCheckedChange={(checked) => onChange(selectOperations(value, [variant], resources, [action], checked))} />)}</div>
          <ul className="text-xs text-text-muted">{variant.facets.map((facet, index) => <li key={index} className="wrap-anywhere">{t(`operationKeys.action.${facet.action}`)} · {facet.resources.map((kind) => t(`operationKeys.kind.${kind}`)).join(', ')} · {t(`operationKeys.usage.${facet.usage}`)}</li>)}</ul>
        </div>)}
      </fieldset>;
    })}</div>}</CurrentDetailPage>
  </div>;
}

function KeyRuleEditor({ initial, accountId, onSave, onClose }: { initial: KeyRule | null; accountId: string; onSave: (rule: KeyRule) => void; onClose: () => void }) {
  const { t } = useTranslation();
  const catalogue = useQuery(tokenOperationsQuery);
  const [kind, setKind] = useState<KeyTargetKind>(initial?.target.kind ?? 'org');
  const [targetId, setTargetId] = useState(initial?.target.id ?? '');
  const [targetName, setTargetName] = useState<string | null>(initial?.target.id ?? null);
  const [reach, setReach] = useState<KeyTarget['reach']>(initial?.target.reach ?? 'self');
  const [resources, setResources] = useState<KeyTargetKind[]>(initial?.resources ?? []);
  const [allow, setAllow] = useState<KeyOperation[]>(initial?.allow ?? []);
  const kinds = [...new Set((catalogue.data ?? []).filter((operation) => operation.delegation === 'business').flatMap((operation) => operation.variants.flatMap((variant) => variant.facets.flatMap((facet) => facet.resources))))].sort();
  const targets = [...new Set<KeyTargetKind>(['account', 'system', ...kinds])];
  const nameKind = NAME_KINDS[kind];
  const valid = (kind === 'system' || UUID.test(targetId)) && resources.length > 0 && allow.length > 0;
  const save = () => {
    if (!valid || catalogue.data === undefined) return;
    const target: KeyTarget = { kind, reach: hasKeySubtree(kind) ? reach : 'self', ...(kind === 'system' ? {} : { id: targetId }) };
    onSave({ target, resources: [...resources], allow: allow.map((item) => ({ operation: item.operation, actions: [...item.actions] })) });
  };
  return <Dialog open onOpenChange={(open) => !open && onClose()} title={t(initial === null ? 'operationKeys.addRule' : 'operationKeys.editRule')} size="xl"
    footer={<><Button onClick={onClose}>{t('common.cancel')}</Button><Button variant="primary" disabled={!valid || catalogue.data === undefined} onClick={save}>{t('operationKeys.keepRule')}</Button></>}>
    {catalogue.isPending ? <SkeletonRows rows={4} /> : catalogue.isError ? <ErrorState error={catalogue.error} onRetry={() => void catalogue.refetch()} /> : <div className="flex min-w-0 flex-col gap-4">
      <Select label={t('operationKeys.targetKind')} value={kind} options={targets.map((value) => ({ value, label: t(`operationKeys.kind.${value}`) }))} onValueChange={(next) => { setKind(next); setTargetId(''); setTargetName(null); setReach('self'); }} />
      {kind === 'account' && <Button size="sm" onClick={() => { setTargetId(accountId); setTargetName(t('operationKeys.thisAccount')); }}>{t('operationKeys.thisAccount')}</Button>}
      {kind !== 'system' && (nameKind === undefined ? <Input label={t('operationKeys.targetId')} value={targetId} onChange={(event) => { setTargetId(event.target.value.trim()); setTargetName(null); }} hint={t('operationKeys.targetIdHint')} />
        : <><NamePicker key={kind} label={t('operationKeys.target')} kinds={[nameKind]} chosen={targetName ?? (targetId === '' ? null : targetId)} placeholder={t('operationKeys.findTarget')} nothing={t('operationKeys.noTargets')} onChoose={(hit) => { setTargetId(hit?.id ?? ''); setTargetName(hit?.name ?? null); }} />
          <details className="text-sm"><summary>{t('operationKeys.byId')}</summary><Input label={t('operationKeys.targetId')} value={targetId} onChange={(event) => { setTargetId(event.target.value.trim()); setTargetName(null); }} hint={t('operationKeys.targetIdHint')} /></details></>)}
      {hasKeySubtree(kind) && <Select label={t('operationKeys.reachLabel')} value={reach} onValueChange={setReach} options={(['self', 'subtree'] as const).map((value) => ({ value, label: t(`operationKeys.reach.${value}`) }))} />}
      {kind === 'account' && reach === 'subtree' && <p className="text-sm text-text-muted">{t('operationKeys.accountSubtree')}</p>}
      <fieldset className="flex flex-col gap-2"><legend className="mb-2 text-sm font-medium">{t('operationKeys.resourceKinds')}</legend><div className="flex flex-wrap gap-x-4 gap-y-2">{[...new Set([...kinds, ...resources])].map((resource) => <Checkbox key={resource} label={t(`operationKeys.kind.${resource}`)} checked={resources.includes(resource)} onCheckedChange={(on) => setResources(on ? [...resources, resource] : resources.filter((other) => other !== resource))} />)}</div></fieldset>
      {resources.length === 0 ? <p className="text-sm text-text-muted">{t('operationKeys.chooseResources')}</p> : <KeyOperationChoice catalogue={catalogue.data} resources={resources} value={allow} onChange={setAllow} />}
      <p className="text-sm text-text-muted">{t('operationKeys.currentRights')}</p>
    </div>}
  </Dialog>;
}

export function KeyRulesField({ value, onChange, accountId, disabled = false }: { value: readonly KeyRule[]; onChange: (rules: KeyRule[]) => void; accountId: string; disabled?: boolean }) {
  const { t } = useTranslation();
  const [editing, setEditing] = useState<number | 'new' | null>(null);
  return <section aria-label={t('operationKeys.rules')} className="flex min-w-0 flex-col gap-3">
    <h3 className="text-sm font-medium">{t('operationKeys.rules')}</h3>
    {value.length === 0 ? <InlineAlert>{t('operationKeys.denyAll')}</InlineAlert> : value.map((rule, index) => <div key={index} className="flex min-w-0 flex-col gap-2 rounded-md border border-border p-3"><KeyRuleSummary rule={rule} /><div className="flex gap-2"><Button size="sm" disabled={disabled} onClick={() => setEditing(index)}>{t('operationKeys.editRuleButton')}</Button><Button size="sm" disabled={disabled} onClick={() => onChange(value.filter((_, at) => at !== index))}>{t('operationKeys.removeRule')}</Button></div></div>)}
    <div><Button disabled={disabled} onClick={() => setEditing('new')}>{t('operationKeys.addRule')}</Button></div>
    {editing !== null && <KeyRuleEditor initial={editing === 'new' ? null : value[editing] ?? null} accountId={accountId} onClose={() => setEditing(null)} onSave={(rule) => { onChange(editing === 'new' ? [...value, rule] : value.map((old, index) => index === editing ? rule : old)); setEditing(null); }} />}
  </section>;
}
