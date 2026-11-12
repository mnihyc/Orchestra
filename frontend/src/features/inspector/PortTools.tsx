/** The current native port-tool offer, rendered without resolving permissions or exposing private sources. */
import { useTranslation } from 'react-i18next';
import { ErrorState } from '@/ui/ErrorState';
import { StateLine } from '@/ui/StateLine';

const TOOLS = ['list_ports', 'open_port', 'close_port', 'list_port_connections'] as const;
type PortTool = typeof TOOLS[number];
const isTool = (value: unknown): value is PortTool => typeof value === 'string' && TOOLS.some((tool) => tool === value);
const record = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value);
interface Row { tool: PortTool; reason: string | null }
function rows(value: unknown): Row[] | null {
  if (!record(value) || !Array.isArray(value.requested) || !Array.isArray(value.effective) || !Array.isArray(value.inactive)
    || !value.requested.every(isTool) || !value.effective.every(isTool)) return null;
  const requested = new Set(value.requested);
  const effective = new Set(value.effective);
  if (requested.size !== value.requested.length || effective.size !== value.effective.length
    || value.effective.some((tool) => !requested.has(tool))) return null;
  const inactive = new Map<PortTool, string>();
  for (const item of value.inactive) {
    if (!record(item) || !isTool(item.tool) || typeof item.reason !== 'string'
      || !requested.has(item.tool) || effective.has(item.tool) || inactive.has(item.tool)) return null;
    inactive.set(item.tool, item.reason);
  }
  if (requested.size !== effective.size + inactive.size) return null;
  return value.requested.map((tool) => ({ tool, reason: effective.has(tool) ? null : inactive.get(tool) ?? '' }));
}
const REASONS: Readonly<Record<string, string>> = {
  no_environment: 'ports.tools.reasons.noEnvironment', environment_unavailable: 'ports.tools.reasons.environment',
  no_machine: 'ports.tools.reasons.noMachine', unsupported: 'ports.tools.reasons.unsupported',
  owner_view_required: 'ports.tools.reasons.owner', policy_off: 'ports.tools.reasons.policy',
  local_off: 'ports.tools.reasons.local', no_audience: 'ports.tools.reasons.audience',
  program_unsupported: 'ports.tools.reasons.program', environment_tools_off: 'ports.tools.reasons.environmentTools',
  continuation_restricted: 'ports.tools.reasons.continuation',
};

export function PortTools({ value, error, onRetry }: { value: unknown; error?: unknown; onRetry?: () => void }) {
  const { t } = useTranslation();
  const found = rows(value);
  return <section aria-label={t('ports.tools.title')} className="border-b border-border px-3 py-2" data-testid="port-tools-inspector">
    <h3 className="text-sm font-medium">{t('ports.tools.title')}</h3>
    {error !== null && error !== undefined ? <ErrorState compact error={error} onRetry={onRetry} />
      : found === null ? <StateLine compact role="status" message={t('ports.tools.unreported')} />
        : found.length === 0 ? <StateLine compact message={t('ports.tools.none')} />
          : <dl className="flex min-w-0 flex-col gap-1 text-sm">{found.map((row) => <div key={row.tool} className="flex min-w-0 flex-wrap items-baseline justify-between gap-x-3 gap-y-1" data-port-tool={row.tool}>
            <dt className="min-w-0 wrap-anywhere">{t(`settings.editor.words.${row.tool}`)}</dt>
            <dd className="min-w-0 wrap-anywhere text-text-muted">{row.reason === null ? t('ports.tools.available')
              : <>{t('ports.tools.inactive')} · {t(REASONS[row.reason] ?? 'ports.tools.reasonUnknown')}</>}</dd>
          </div>)}</dl>}
  </section>;
}
