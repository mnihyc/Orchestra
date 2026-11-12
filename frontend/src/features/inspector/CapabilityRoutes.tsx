/** The inspector renders the server's single capability resolution; it computes no support matrix. */
import { Trans, useTranslation } from 'react-i18next';
import { ArticleGuideLink } from '@/features/help/ArticleGuideLink';
import { ResourceChip } from '@/features/resource-facts/ResourceFacts';
import { useAppearance } from '@/lib/appearance';
import { formatNumber } from '@/lib/format';
import { StatusDot } from '@/ui/StatusDot';

interface Quote { amount: string; currency: string; unit: string }
interface CapabilityView {
  capability: string; title: string; reason: string | null; program_default: boolean;
  detail?: string | null;
  route_view: { kind: string } | null; provider: { id: string; title: string } | null;
  prices: Record<string, Quote>; fallbacks?: unknown; facts?: unknown;
}
const record = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value);
/** Older servers omit the block. Unknown additive members remain the server's own data. */
function rows(value: unknown): CapabilityView[] {
  if (!Array.isArray(value)) return [];
  return value.filter((row): row is CapabilityView => record(row) && typeof row.capability === 'string' && typeof row.title === 'string'
    && (row.reason === null || typeof row.reason === 'string') && typeof row.program_default === 'boolean'
    && (row.detail === undefined || row.detail === null || typeof row.detail === 'string')
    && (row.route_view === null || (record(row.route_view) && typeof row.route_view.kind === 'string'))
    && (row.provider === null || (record(row.provider) && typeof row.provider.id === 'string' && typeof row.provider.title === 'string'))
    && record(row.prices));
}
function quotes(value: Record<string, Quote>): Quote[] {
  return Object.values(value).filter((price) => record(price) && typeof price.amount === 'string'
    && price.amount.trim() !== '' && Number.isFinite(Number(price.amount)) && Number(price.amount) >= 0
    && typeof price.currency === 'string' && typeof price.unit === 'string');
}
interface Fallback { rule_id: string; scope: string; when: string; provider: { id: string; title: string } | null }
const word = (words: Record<string, string>, key: string | null | undefined): string | undefined => key === null || key === undefined || !Object.hasOwn(words, key) ? undefined : words[key];
function fallbackRows(value: unknown): Fallback[] {
  return Array.isArray(value) ? value.filter((row): row is Fallback => record(row) && typeof row.rule_id === 'string'
    && typeof row.scope === 'string' && typeof row.when === 'string'
    && (row.provider === null || (record(row.provider) && typeof row.provider.id === 'string' && typeof row.provider.title === 'string'))) : [];
}
function facts(value: unknown, language: string): { id: string; text: string }[] {
  if (!Array.isArray(value)) return [];
  return value.flatMap((fact) => {
    if (!record(fact) || typeof fact.id !== 'string' || !record(fact.text)) return [];
    const options = [fact.text[language], fact.text[language.split('-')[0] ?? ''], fact.text.en];
    const text = options.find((word): word is string => typeof word === 'string' && word.trim() !== '');
    return text === undefined ? [] : [{ id: fact.id, text }];
  });
}
export function CapabilityRoutes({ value }: { value: unknown }) {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  const capabilities = rows(value);
  if (capabilities.length === 0) return null;
  const names: Record<string, string> = {
    "generation": t('capabilityRoutes.names.generation'),
    "image_input": t('capabilityRoutes.names.image_input'),
    "web_search": t('capabilityRoutes.names.web_search'),
    "page_fetch": t('capabilityRoutes.names.page_fetch'),
    "image_generation": t('capabilityRoutes.names.image_generation'),
    "embeddings": t('capabilityRoutes.names.embeddings'),
    "provider_facts": t('capabilityRoutes.names.provider_facts'),
    "summary": t('capabilityRoutes.names.summary'),
  };
  const routes: Record<string, string> = {
    "native-model": t('capabilityRoutes.routes.native-model'),
    "native-program": t('capabilityRoutes.routes.native-program'),
    "service": t('capabilityRoutes.routes.service'),
    "delegation": t('capabilityRoutes.routes.delegation'),
  };
  const reasons: Record<string, string> = {
    "native_image_sign_in_required": t('capabilityRoutes.reasons.native_image_sign_in_required'),
    "route_not_declared": t('capabilityRoutes.reasons.route_not_declared'),
    "route_does_not_carry_capability": t('capabilityRoutes.reasons.route_does_not_carry_capability'),
    "explicit_choice_required": t('capabilityRoutes.reasons.explicit_choice_required'),
    "program_cannot_carry_tools": t('capabilityRoutes.reasons.program_cannot_carry_tools'),
    "executor_not_installed": t('capabilityRoutes.reasons.executor_not_installed'),
    "backend_not_selected": t('capabilityRoutes.reasons.backend_not_selected'),
    "provider_not_declared": t('capabilityRoutes.reasons.provider_not_declared'),
    "offering_not_declared": t('capabilityRoutes.reasons.offering_not_declared'),
    "offering_not_enabled": t('capabilityRoutes.reasons.offering_not_enabled'),
    "current_authority_unavailable": t('capabilityRoutes.reasons.current_authority_unavailable'),
    "current_authority_refused": t('capabilityRoutes.reasons.current_authority_refused'),
    "interface_not_declared": t('capabilityRoutes.reasons.interface_not_declared'),
    "credential_role_unavailable": t('capabilityRoutes.reasons.credential_role_unavailable'),
    "endpoint_not_declared": t('capabilityRoutes.reasons.endpoint_not_declared'),
    "offering_not_selected": t('capabilityRoutes.reasons.offering_not_selected'),
    "route_not_applicable": t('capabilityRoutes.reasons.route_not_applicable'),
    "program_does_not_carry_interface": t('capabilityRoutes.reasons.program_does_not_carry_interface'),
    "program_does_not_carry_mode": t('capabilityRoutes.reasons.program_does_not_carry_mode'),
    "program_support_not_applicable": t('capabilityRoutes.reasons.program_support_not_applicable'),
    "offering_does_not_enable_mode": t('capabilityRoutes.reasons.offering_does_not_enable_mode'),
    "offering_support_not_applicable": t('capabilityRoutes.reasons.offering_support_not_applicable'),
    "native_tool_not_declared": t('capabilityRoutes.reasons.native_tool_not_declared'),
    "native_tool_not_applicable": t('capabilityRoutes.reasons.native_tool_not_applicable'),
    "program_default_unreported": t('capabilityRoutes.reasons.program_default_unreported'),
    "mode_not_declared": t('capabilityRoutes.reasons.mode_not_declared'),
    "disabled": t('capabilityRoutes.reasons.disabled'),
    "route_unavailable": t('capabilityRoutes.reasons.route_unavailable'),
    "interface_not_installed": t('capabilityRoutes.reasons.interface_not_installed'),
  };
  const units: Record<string, string> = {
    "search": t('capabilityRoutes.units.search'),
    "page": t('capabilityRoutes.units.page'),
    "image": t('capabilityRoutes.units.image'),
    "token": t('capabilityRoutes.units.token'),
    "call": t('capabilityRoutes.units.call'),
  };
  const scopes: Record<string, string> = {
    "default": t('capabilityRoutes.scopes.default'),
    "system": t('capabilityRoutes.scopes.system'),
    "org": t('capabilityRoutes.scopes.org'),
    "project": t('capabilityRoutes.scopes.project'),
    "topic": t('capabilityRoutes.scopes.topic'),
    "thread": t('capabilityRoutes.scopes.thread'),
  };
  return <section aria-label={t('capabilityRoutes.title')} data-testid="capability-routes" className="border-b border-border">
    <div className="flex flex-wrap items-center justify-between gap-2 px-3 py-2">
      <h3 className="text-sm font-medium text-text">{t('capabilityRoutes.title')}</h3>
      <ArticleGuideLink article="provider-web-search" title={t('capabilityRoutes.help')} />
    </div>
    <ul className="flex flex-col">
      {capabilities.map((row) => {
        const kind = row.route_view?.kind;
        const label = kind === undefined ? word(reasons, row.detail) ?? word(reasons, row.reason) ?? t('capabilityRoutes.unavailable') : word(routes, kind) ?? t('capabilityRoutes.unknownRoute');
        const prices = quotes(row.prices);
        const notes = facts(row.facts, locale);
        return <li key={row.capability} data-capability={row.capability} className="flex min-w-0 flex-col gap-1 border-t border-border px-3 py-2 text-sm">
          <div className="flex min-w-0 flex-wrap items-baseline justify-between gap-x-3 gap-y-1">
            <span className="font-medium text-text">{word(names, row.capability) ?? row.title}</span>
            <StatusDot tone={kind === undefined ? 'off' : 'idle'} label={label} showLabel />
          </div>
          {kind !== undefined && row.program_default && <span className="text-text-muted">{t('capabilityRoutes.programDefault')}</span>}
          {row.provider !== null && <ResourceChip subject={{ kind: 'provider', provider: { id: row.provider.id, name: row.provider.title } }} hint={false} />}
          {prices.map((price, index) => <span key={index} className="text-text-muted tabular-nums">{t('capabilityRoutes.quote', {
            // Significant digits preserve a small positive quote without inventing a zero cost.
            amount: formatNumber(Number(price.amount), locale, { maximumSignificantDigits: 6 }),
            currency: price.currency === 'credits' ? t('capabilityRoutes.creditUnit') : price.currency,
            unit: word(units, price.unit) ?? price.unit,
          })}</span>)}
          {(kind === 'native-model' || kind === 'service' || kind === 'delegation') && prices.length === 0 && <span className="text-text-muted">{t('capabilityRoutes.noPrice')}</span>}
          {notes.length > 0 && <ul className="flex min-w-0 list-disc flex-col gap-1 break-words ps-4 text-text-muted" data-testid="capability-facts">
            {notes.map((fact) => <li key={fact.id} data-fact={fact.id}>{fact.text}</li>)}
          </ul>}
          {fallbackRows(row.fallbacks).map((fallback, index) => <span key={index} className="text-text-muted"><Trans i18nKey="capabilityRoutes.fallback" values={{ rule: fallback.rule_id, source: word(scopes, fallback.scope) ?? t('capabilityRoutes.scopeUnknown'), when: t(`calls.kinds.${fallback.when}`, { defaultValue: t('calls.kinds.any') }) }} components={{ provider: fallback.provider === null ? <span>{t('calls.capabilityDestination')}</span> : <ResourceChip subject={{ kind:'provider',provider:{id:fallback.provider.id,name:fallback.provider.title} }} hint={false} /> }} /></span>)}
        </li>;
      })}
    </ul>
  </section>;
}
