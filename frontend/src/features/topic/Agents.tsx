/** Agents available to a new thread here, and its current defaults, from the same authorized answer as Start. */
import { useQuery } from '@tanstack/react-query';
import { Bot } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { startOptionsQuery, type StartOptions } from '@/api/start';
import { modelName } from '@/features/start/choices';
import type { AgentRef, BindingAgent, Topic } from '@/generated/api-types';
import { offeredChoice } from '@/i18n/choices';
import { holds } from '@/lib/permissions';
import { Button } from '@/ui/Button';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorState } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';

/** Placement and policy are already judged by the server; never list a merely visible, unusable profile as available. */
function usable(agent: BindingAgent): boolean {
  return agent.usable && agent.reason === null && !agent.agent.disabled && agent.arrangements.some((way) => way.available && way.permitted);
}

/** A profile offered by several providers is still one agent; keep its name and default marker once. */
function available(options: StartOptions) {
  const providers = options.providers.filter((entry) => entry.usable && entry.reason === null && entry.provider.status === 'ready'
    && entry.models.some((model) => model.allowed) && entry.agents.some(usable));
  const agents = new Map<string, AgentRef>();
  for (const provider of providers) for (const entry of provider.agents) if (usable(entry)) agents.set(entry.agent.id, entry.agent);
  return { providers, agents: [...agents.values()] };
}

export function TopicAgents({ topic }: { topic: Topic }) {
  const { t } = useTranslation();
  const mayStart = holds(topic.access, 'topic.interact') && !topic.archived;
  const query = useQuery({ ...startOptionsQuery(topic.id, null), enabled: mayStart });
  if (!mayStart) return <EmptyState compact title={topic.archived ? t('topicAgents.archived') : t('topicAgents.notAllowed', { owner: topic.owner.display_name })} />;
  if (query.isPending) return <SkeletonRows rows={3} />;
  if (query.isError) return <ErrorState compact error={query.error} retrying={query.isRefetching} onRetry={() => void query.refetch()} />;

  const { providers, agents } = available(query.data);
  const defaults = query.data.defaults;
  const provider = providers.find((entry) => entry.provider.id === defaults.provider_id);
  const agent = provider?.agents.find((entry) => entry.agent.id === defaults.agent_id && usable(entry));
  const model = provider?.models.find((entry) => entry.allowed && entry.model_name === defaults.model_name);
  const efforts = model?.parameters.efforts ?? [];
  const effort = defaults.effort ?? model?.parameters.default_effort ?? null;
  const effortName = effort === null || !efforts.includes(effort) ? null : offeredChoice(t, 'effort', effort)?.name ?? t('changeCompact.otherEffort');
  const incomplete = provider === undefined || agent === undefined || model === undefined;
  const settings = <Button size="sm" variant="ghost" asChild><a href={`/topic/${topic.id}?tab=settings&section=policies`}>{t('topicAgents.defaults')}</a></Button>;
  if (agents.length === 0) return <EmptyState compact icon={<Bot aria-hidden />} title={t('topicAgents.empty')} action={settings} />;

  return <div data-topic-agents className="flex min-w-0 flex-col gap-4">
    <ul aria-label={t('topicAgents.available')} className="divide-y divide-border rounded-lg border border-border">
      {agents.map((entry) => <li key={entry.id} className="flex min-w-0 items-start gap-3 px-3 py-2">
        <span className="flex h-6 shrink-0 items-center"><Bot aria-hidden className="size-4 text-text-muted" /></span>
        <span className="flex min-w-0 flex-1 flex-wrap items-baseline gap-x-3 gap-y-1">
          <a href={`/resources/agents/${entry.id}`} className="focus-inset min-w-0 max-w-full rounded-sm font-medium wrap-anywhere hover:underline">{entry.name}</a>
          {agent?.agent.id === entry.id && <span className="text-sm text-text-muted">{t('topicAgents.defaultAgent')}</span>}
        </span>
      </li>)}
    </ul>
    <div className="flex flex-wrap items-start gap-x-4 gap-y-2 text-sm">
      <span className="text-text-muted">{t('topicAgents.providers')}</span>
      <ul className="flex min-w-0 flex-1 flex-wrap items-start gap-x-4 gap-y-2">
        {providers.map((entry) => <li key={entry.provider.id} className="flex min-w-0 flex-wrap items-baseline gap-x-2">
          <a href={`/resources/providers/${entry.provider.id}`} className="focus-inset rounded-sm wrap-anywhere hover:underline">{entry.provider.name}</a>
          {provider?.provider.id === entry.provider.id && <span className="text-text-muted">{t('topicAgents.defaultProvider')}</span>}
        </li>)}
      </ul>
    </div>
    <div className="flex flex-wrap items-start justify-between gap-3">
      {model !== undefined && <dl className="flex min-w-0 flex-wrap gap-x-4 gap-y-2 text-sm">
        <div className="flex min-w-0 items-baseline gap-2"><dt className="text-text-muted">{t('topicAgents.model')}</dt><dd className="wrap-anywhere">{modelName(model)}</dd></div>
        {effortName !== null && <div className="flex items-baseline gap-2"><dt className="text-text-muted">{t('topicAgents.effort')}</dt><dd>{effortName}</dd></div>}
      </dl>}
      {settings}
    </div>
    {incomplete && <p className="text-sm text-text-muted">{t('topicAgents.incomplete')}</p>}
  </div>;
}
