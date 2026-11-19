/** Per-driver capability definitions.
 *
 * What an agent can ask of the machine its environment runs on: each capability with a label, how risky
 * it is, what happens by default when somebody asks for it (it is given, it waits for an approver, or it
 * is refused), who the approver is (the host's managers, the admins of the environment's home org, or
 * system admins), and the patch each driver applies for it, as JSON. A system admin changes a
 * definition or adds one; one that ships with Orch cannot be removed, and one an environment holds
 * cannot be removed while it does.
 */
import { useMutation, useQuery } from '@tanstack/react-query';
import { Ellipsis, Pencil, Plus, ShieldQuestionMark, Trash } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { choiceOption } from '@/i18n/choices';

import { capabilitiesQuery, deleteCapability, saveCapability } from '@/api/admin';
import { capabilityDescription, capabilityName } from '@/features/resources/capabilityWords';
import { ApiError } from '@/api/client';
import { parseObject, patchedDrivers } from '@/features/admin/harnesses';
import type { ApproverScope, CapabilityDef, CapabilityDefaultDecision, Risk } from '@/generated/api-types';
import { useForm } from '@/lib/forms';
import { StatusTag, type TagTone } from '@/features/admin/StatusTag';
import { Button } from '@/ui/Button';
import { ConfirmDialog } from '@/ui/ConfirmDialog';
import { Dialog } from '@/ui/Dialog';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { IconButton } from '@/ui/IconButton';
import { Input } from '@/ui/Input';
import { Menu } from '@/ui/Menu';
import { Select } from '@/ui/Select';
import { SkeletonRows } from '@/ui/Skeleton';
import { type Column, Table } from '@/ui/Table';
import { Textarea } from '@/ui/Textarea';
import { toast } from '@/ui/Toast';

const RISKS: readonly Risk[] = ['low', 'medium', 'high', 'critical'];
const DECISIONS: readonly CapabilityDefaultDecision[] = ['allow', 'ask', 'deny'];
const SCOPES: readonly ApproverScope[] = ['environment', 'host', 'org', 'system'];
const KEY = /^[a-z][a-z0-9_]{1,31}$/;

/** How a risk is told: the word is always written; the colour only helps. */
const RISK_TONES: Record<Risk, TagTone> = { low: 'neutral', medium: 'neutral', high: 'warning', critical: 'danger' };

function CapabilityForm({ def, onClose }: { def: CapabilityDef | null; onClose: () => void }) {
  const { t } = useTranslation();
  const form = useForm(
    {
      key: def?.key ?? '',
      label: def?.label ?? '',
      description: def?.description ?? '',
      patch: JSON.stringify(def?.driver_patch ?? { docker: {} }, null, 2),
    },
    {
      key: (value) => (KEY.test(value) ? null : t('admin.capabilities.keyRule')),
      label: (value) => (value.trim() === '' ? t('admin.capabilities.labelRule') : null),
      patch: (value) => (parseObject(value).ok ? null : t('admin.capabilities.patchRule')),
    },
  );
  const [risk, setRisk] = useState<Risk>(def?.risk ?? 'medium');
  const [decision, setDecision] = useState<CapabilityDefaultDecision>(def?.default_decision ?? 'ask');
  const [scope, setScope] = useState<ApproverScope>(def?.default_approver_scope ?? 'host');
  const [placed, setPlaced] = useState(false);

  const save = useMutation({
    mutationFn: (values: typeof form.values) => {
      const parsed = parseObject(values.patch);
      return saveCapability({
        key: values.key,
        label: values.label.trim(),
        description: values.description.trim(),
        risk,
        default_decision: decision,
        default_approver_scope: scope,
        driver_patch: parsed.ok ? parsed.value : {},
        builtin: def?.builtin ?? false,
        revision: def?.revision ?? 0,
      });
    },
    onSuccess: (saved) => {
      toast({ tone: 'success', title: t(def === null ? 'admin.capabilities.added' : 'admin.capabilities.saved', { name: saved.label }) });
      onClose();
    },
    onError: (error) => setPlaced(form.refuse(error)),
  });
  const stale = save.error instanceof ApiError && save.error.is('conflict.stale_revision');

  return (
    <Dialog
      open
      size="lg"
      onOpenChange={(open) => !open && !save.isPending && onClose()}
      title={def === null ? t('admin.capabilities.addTitle') : t('admin.capabilities.editTitle', { name: capabilityName(def.key, def.label, t) })}
      description={t('admin.capabilities.formLead')}
      dismissible={!save.isPending}
      footer={
        <>
          <Button disabled={save.isPending} onClick={onClose}>
            {t('common.cancel')}
          </Button>
          <Button variant="primary" type="submit" form="admin-capability-form" pending={save.isPending}>
            {def === null ? t('admin.capabilities.add') : t('admin.capabilities.save')}
          </Button>
        </>
      }
    >
      <form id="admin-capability-form" className="flex flex-col gap-4 py-2" noValidate onSubmit={form.submit((values) => save.mutate(values))}>
        <div className="grid gap-4 sm:grid-cols-2">
          <Input label={t('admin.capabilities.key')} hint={t('admin.capabilities.keyHint')} autoComplete="off" autoCapitalize="none" mono readOnly={def !== null} {...form.field('key')} />
          <Input label={t('admin.capabilities.label')} autoComplete="off" {...form.field('label')} />
        </div>
        <Textarea label={t('admin.capabilities.description')} optional rows={2} {...form.field('description')} />
        <div className="grid gap-4 sm:grid-cols-3">
          <Select label={t('admin.capabilities.risk')} value={risk} onValueChange={setRisk} options={RISKS.map((one) => ({ value: one, ...choiceOption(t, `risk.${one}`) }))} />
          <Select
            label={t('admin.capabilities.decision')}
            hint={t(`admin.capabilities.decisions.${decision}.about`)}
            value={decision}
            onValueChange={setDecision}
            options={DECISIONS.map((one) => ({ value: one, ...choiceOption(t, `capabilityDecision.${one}`), description: t(`admin.capabilities.decisions.${one}.about`) }))}
          />
          <Select
            label={t('admin.capabilities.approver')}
            hint={t(`admin.capabilities.scopes.${scope}`)}
            value={scope}
            onValueChange={setScope}
            options={SCOPES.map((one) => ({ value: one, ...choiceOption(t, `approverScope.${one}`), description: t(`admin.capabilities.scopes.${one}`) }))}
          />
        </div>
        <Textarea label={t('admin.capabilities.patch')} hint={t('admin.capabilities.patchHint')} rows={8} spellCheck={false} className="font-mono text-sm" {...form.field('patch')} />
        {save.error !== null && !placed && <ErrorAlert error={save.error} />}
        {stale && <p className="text-sm text-text-muted">{t('admin.capabilities.stale')}</p>}
      </form>
    </Dialog>
  );
}

export function Capabilities() {
  const { t } = useTranslation();
  const driverNames=(def:CapabilityDef)=>patchedDrivers(def.driver_patch).map((driver)=>t(`templates.driverChoices.${driver}.name`, { defaultValue: driver })).join(t('startFlow.separator'));
  const defs = useQuery(capabilitiesQuery);
  const [editing, setEditing] = useState<CapabilityDef | 'new' | null>(null);
  const [removing, setRemoving] = useState<CapabilityDef | null>(null);

  const remove = useMutation({
    mutationFn: (def: CapabilityDef) => deleteCapability(def.key),
    onSuccess: (_gone, def) => {
      toast({ tone: 'success', title: t('admin.capabilities.removed', { name: capabilityName(def.key, def.label, t) }) });
      setRemoving(null);
    },
  });

  const columns: Column<CapabilityDef>[] = [
    {
      id: 'capability',
      header: t('admin.capabilities.capability'),
      cell: (def) => (
        <span className="flex min-w-0 flex-col gap-1">
          <span className="flex flex-wrap items-center gap-2">
            <span className="font-medium text-text">{capabilityName(def.key, def.label, t)}</span>
            {def.builtin && <StatusTag>{t('admin.capabilities.ships')}</StatusTag>}
          </span>
          {def.description !== '' && <span className="text-sm wrap-anywhere text-text-muted">{capabilityDescription(def.key, def.description, t)}</span>}
        </span>
      ),
    },
    { id: 'risk', header: t('admin.capabilities.risk'), className: 'whitespace-nowrap', cell: (def) => <StatusTag tone={RISK_TONES[def.risk]}>{t(`admin.capabilities.risks.${def.risk}`)}</StatusTag> },
    { id: 'decision', header: t('admin.capabilities.decision'), from: 'sm', cell: (def) => t(`admin.capabilities.decisions.${def.default_decision}.label`) },
    { id: 'approver', header: t('admin.capabilities.approver'), from: 'md', cell: (def) => <span className="text-sm">{t(`admin.capabilities.scopes.${def.default_approver_scope}`)}</span> },
    { id: 'drivers', header: t('admin.capabilities.drivers'), from: 'lg', className: 'whitespace-nowrap', cell: (def) => <span className="text-sm">{driverNames(def)}</span> },
    {
      id: 'menu',
      header: t('admin.users.actions'),
      hideHeader: true,
      align: 'end',
      cell: (def) => (
        <Menu
          label={t('admin.capabilities.menuOf', { name: capabilityName(def.key, def.label, t) })}
          align="end"
          entries={[
            { id: 'edit', label: t('admin.capabilities.edit'), icon: <Pencil aria-hidden />, onSelect: () => setEditing(def) },
            { type: 'separator', id: 'apart' },
            {
              id: 'remove',
              label: t('admin.capabilities.remove'),
              icon: <Trash aria-hidden />,
              danger: true,
              disabled: def.builtin ? t('admin.capabilities.shipsReason') : false,
              onSelect: () => setRemoving(def),
            },
          ]}
          trigger={<IconButton size="sm" label={t('admin.capabilities.menuOf', { name: capabilityName(def.key, def.label, t) })} icon={<Ellipsis aria-hidden />} />}
        />
      ),
    },
  ];

  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-wrap items-center justify-end gap-3">
        <Button variant="primary" icon={<Plus aria-hidden />} onClick={() => setEditing('new')}>
          {t('admin.capabilities.addTitle')}
        </Button>
      </div>
      {defs.isPending ? (
        <SkeletonRows rows={4} />
      ) : defs.isError ? (
        <ErrorState error={defs.error} retrying={defs.isRefetching} onRetry={() => void defs.refetch()} />
      ) : defs.data.length === 0 ? (
        <EmptyState compact icon={<ShieldQuestionMark aria-hidden />} title={t('admin.capabilities.none')} />
      ) : (
        <Table label={t('admin.sections.capabilities.title')} columns={columns} rows={defs.data} rowKey={(def) => def.key} />
      )}

      {editing !== null && <CapabilityForm key={editing === 'new' ? 'new' : editing.key} def={editing === 'new' ? null : editing} onClose={() => setEditing(null)} />}
      <ConfirmDialog
        open={removing !== null}
        onOpenChange={(open) => !open && setRemoving(null)}
        title={t('admin.capabilities.removeTitle', { name: removing === null ? '' : capabilityName(removing.key, removing.label, t) })}
        confirmLabel={t('admin.capabilities.removeConfirm', { name: removing === null ? '' : capabilityName(removing.key, removing.label, t) })}
        danger
        pending={remove.isPending}
        error={remove.error ?? undefined}
        onConfirm={() => removing !== null && remove.mutate(removing)}
      >
        {t('admin.capabilities.removeBody')}
      </ConfirmDialog>
    </div>
  );
}
