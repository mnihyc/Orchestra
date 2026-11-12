/** A starter's inspected program keeps the same reported failure and provider destination. */
import { Link } from '@tanstack/react-router';
import { useTranslation } from 'react-i18next';
import { resourcePreview } from '@/features/resource-facts/ResourceFacts';
import type { BindingAgent, ProviderRef } from '@/generated/api-types';
import type { CardPreview } from '@/ui/DetailCard';
import { linkClasses } from '@/ui/link';

export function ProgramReport({ agent, provider }: { agent: BindingAgent; provider: ProviderRef }) {
  const { t } = useTranslation();
  const report = agent.program_report;
  if (report?.status !== 'error') return null;
  return <section aria-label={t('start.agent.programDetails', { agent: agent.agent.name })} className="flex min-w-0 flex-col gap-2 text-sm">
    <p className="font-medium text-danger-text">{t('providers.runtime.harnessStatus.error')}</p>
    {report.detail !== null && report.detail !== '' && <pre className="max-w-full overflow-x-auto whitespace-pre-wrap wrap-anywhere text-text-muted">{report.detail}</pre>}
    <Link to="/resources/providers/$id" params={{ id: provider.id }} search={{ tab: 'settings' }} className={linkClasses}>{t('start.agent.openProgramReports')}</Link>
  </section>;
}

export function agentProgramPreview(agent: BindingAgent, provider: ProviderRef | null): CardPreview {
  const preview = resourcePreview({ kind: 'agent', agent: agent.agent });
  if (provider === null || agent.program_report?.status !== 'error') return preview;
  return { ...preview, content: () => <><ProgramReport agent={agent} provider={provider} />{preview.content()}</> };
}
