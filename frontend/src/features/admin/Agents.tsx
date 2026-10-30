/** The system's profiles.
 *
 * The agent profiles that belong to the system itself, whose profile has no home: each with the adapter
 * it runs and whether it is switched off, opening on its own page. A new one is made here from an
 * adapter the system has, with the values of its parameters written as JSON.
 */
import { useMutation, useQuery } from '@tanstack/react-query';
import { Link } from '@tanstack/react-router';
import { ResourceChip } from '@/features/resource-facts/ResourceFacts';
import { Bot, Plus } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { choiceOption } from '@/i18n/choices';

import { createSystemAgent, harnessKindsQuery, systemAgentsQuery } from '@/api/admin';
import { parseObject } from '@/features/admin/harnesses';
import { programInfoName, programNameIn } from '@/features/resources/programNames';
import type { Agent } from '@/generated/api-types';
import { useForm } from '@/lib/forms';
import { StatusTag } from '@/features/admin/StatusTag';
import { Button } from '@/ui/Button';
import { Dialog } from '@/ui/Dialog';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { Input } from '@/ui/Input';
import { RelativeTime } from '@/ui/RelativeTime';
import { ChoiceLocator } from '@/ui/ChoiceLocator';
import { SkeletonRows } from '@/ui/Skeleton';
import { type Column, Table } from '@/ui/Table';
import { Textarea } from '@/ui/Textarea';
import { toast } from '@/ui/Toast';

function NewProfile({ onClose }: { onClose: () => void }) {
  const { t, i18n } = useTranslation();
  const kinds = useQuery(harnessKindsQuery);
  const form = useForm(
    { name: '', profile: '{}' },
    {
      name: (value) => (value.trim() === '' ? t('admin.agents.nameRule') : null),
      profile: (value) => (parseObject(value).ok ? null : t('admin.agents.profileRule')),
    },
  );
  const [chosen, setChosen] = useState<string | null>(null);
  const kind = chosen ?? kinds.data?.[0]?.kind ?? '';
  const [placed, setPlaced] = useState(false);

  const make = useMutation({
    mutationFn: (values: typeof form.values) => {
      const parsed = parseObject(values.profile);
      return createSystemAgent({ name: values.name.trim(), harness_kind: kind, profile: parsed.ok ? parsed.value : {} });
    },
    onSuccess: (agent) => {
      toast({ tone: 'success', title: t('admin.agents.made', { name: agent.name }) });
      onClose();
    },
    onError: (error) => setPlaced(form.refuse(error)),
  });

  return (
    <Dialog
      open
      onOpenChange={(open) => !open && !make.isPending && onClose()}
      title={t('admin.agents.newTitle')}
      description={t('admin.agents.newLead')}
      dismissible={!make.isPending}
      footer={
        <>
          <Button disabled={make.isPending} onClick={onClose}>
            {t('common.cancel')}
          </Button>
          <Button variant="primary" type="submit" form="admin-agent-form" pending={make.isPending} disabled={kind === ''}>
            {t('admin.agents.make')}
          </Button>
        </>
      }
    >
      <form id="admin-agent-form" className="flex flex-col gap-4 py-2" noValidate onSubmit={form.submit((values) => make.mutate(values))}>
        <Input label={t('admin.agents.name')} autoComplete="off" {...form.field('name')} />
        <ChoiceLocator
          label={t('admin.agents.adapter')}
          value={kind}
          onValueChange={setChosen}
          placeholder={kinds.isSuccess && !kinds.isFetching && kinds.data.length === 0 ? t('admin.agents.noAdapters') : undefined}
          options={(kinds.data ?? []).map((one) => ({ value: one.kind, ...choiceOption(t, 'dynamic.harness', { name: programInfoName(one, i18n.resolvedLanguage ?? i18n.language) ?? t('resources.harnessNameUnknown') }) }))}
        />
        <Textarea label={t('admin.agents.profile')} hint={t('admin.agents.profileHint')} rows={6} spellCheck={false} className="font-mono text-sm" {...form.field('profile')} />
        {make.error !== null && !placed && <ErrorAlert error={make.error} />}
      </form>
    </Dialog>
  );
}

export function Agents() {
  const { t, i18n } = useTranslation();
  const agents = useQuery(systemAgentsQuery);
  const kinds = useQuery(harnessKindsQuery);
  const [making, setMaking] = useState(false);
  const adapter = (agent: Agent) => {
    const locale = i18n.resolvedLanguage ?? i18n.language;
    const found = kinds.data?.find((one) => one.kind === agent.harness_kind);
    return programNameIn(agent, locale) ?? programInfoName(found, locale) ?? t('resources.harnessNameUnknown');
  };

  const columns: Column<Agent>[] = [
    {
      id: 'name',
      header: t('admin.agents.name'),
      cell: (agent) => (
        <span className="flex flex-wrap items-center gap-2">
          <ResourceChip subject={{ kind: 'agent', agent }} asChild hint={false}><Link to="/resources/agents/$id" params={{ id: agent.id }} className="font-medium text-accent-text hover:underline">
            {agent.name}
          </Link></ResourceChip>
          {agent.disabled && <StatusTag>{t('admin.agents.disabled')}</StatusTag>}
        </span>
      ),
    },
    { id: 'adapter', header: t('admin.agents.adapter'), from: 'sm', cell: (agent) => adapter(agent) },
    { id: 'made', header: t('admin.agents.created'), from: 'md', cell: (agent) => <RelativeTime at={agent.created_at} className="text-text-muted" /> },
  ];

  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-wrap items-center gap-3">
        <p className="min-w-0 flex-1 text-sm text-text-muted">{t('admin.agents.lead')}</p>
        <Button variant="primary" icon={<Plus aria-hidden />} onClick={() => setMaking(true)}>
          {t('admin.agents.newTitle')}
        </Button>
      </div>
      {agents.isPending ? (
        <SkeletonRows rows={3} />
      ) : agents.isError ? (
        <ErrorState error={agents.error} retrying={agents.isRefetching} onRetry={() => void agents.refetch()} />
      ) : agents.data.length === 0 ? (
        <EmptyState compact icon={<Bot aria-hidden />} title={t('admin.agents.none')} description={t('admin.agents.noneHint')} />
      ) : (
        <Table label={t('admin.sections.agents.title')} columns={columns} rows={agents.data} rowKey={(agent) => agent.id} />
      )}
      {making && <NewProfile onClose={() => setMaking(false)} />}
    </div>
  );
}
