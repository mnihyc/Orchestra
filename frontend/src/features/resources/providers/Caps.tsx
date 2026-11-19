/** Caps on use of the provider by person, group, model or project, in tokens, in credits or in turns at once; and who may let a turn past one.
 *
 * What is here is the part of that which a model's row editor needs: the caps on the provider that name one
 * model, a family it is in or a group it is in, each as a line with how far it is used, and the means to add
 * one with the model filled in. A cap is the only place a limit on a model lives; the Models tab shows the
 * ones that cover a model and leads to the form that makes one (`capEntry.ts`). The caps of the whole
 * provider are counted here and left to the screens of usage.
 */
import { useQuery } from '@tanstack/react-query';
import type { TFunction } from 'i18next';
import { useTranslation } from 'react-i18next';
import { EmptyState } from '@/ui/EmptyState';
import { offeredChoice } from '@/i18n/choices';

import { providerCapsQuery } from '@/api/models';
import { openCap } from '@/features/resources/providers/capEntry';
import { groupPattern, isGroupPattern, listNames } from '@/features/models/patterns';
import type { CapStatus, ProviderModel, ProviderRef, UsageCap } from '@/generated/api-types';
import { useAppearance } from '@/lib/appearance';
import { formatNumber } from '@/lib/format';
import { Badge } from '@/ui/Badge';
import { Button } from '@/ui/Button';
import { SkeletonRows } from '@/ui/Skeleton';
import { useErrorText } from '@/ui/errorText';

/** Whom a cap covers, in words. */
export function capWho(cap: Pick<UsageCap, 'subject' | 'split'>, t: TFunction): string {
  const each = cap.split === 'each';
  switch (cap.subject.kind) {
    case 'everyone':
      return each ? t('models.limits.who.eachPerson') : t('models.limits.who.everyone');
    case 'user':
      return cap.subject.user.display_name;
    case 'group':
      return each ? t('models.limits.who.eachOf', { name: cap.subject.group.name }) : t('models.limits.who.together', { name: cap.subject.group.name });
    case 'role':
      return t('models.limits.who.eachRole', { role: t(`roles.${cap.subject.role}`) });
    case 'members':
      return t('access.holder.membersOf', { place: cap.subject.node.name });
  }
}

/** What a cap allows, in words: "500 credits a month", "At most 4 turns at once", "No limit". */
export function capWhat(cap: Pick<UsageCap, 'amount' | 'meter' | 'period'>, t: TFunction, locale: string): string {
  if (cap.amount === null) return t('models.limits.noLimit');
  const amount = formatNumber(Number(cap.amount), locale);
  if (cap.meter === 'running') return t('models.limits.running', { amount });
  return t(`models.limits.${cap.meter}`, { amount, period: t(`models.limits.period.${cap.period ?? 'total'}`) });
}

/** Whether a cap names this model, a family it is in or a group it is in; a cap that names no model covers every one and is left to the whole provider. */
export function namesModel(cap: Pick<UsageCap, 'models'>, model: Pick<ProviderModel, 'model_name' | 'groups'>): boolean {
  return cap.models.length > 0 && listNames(cap.models, model.model_name, model.groups);
}

/** What a cap covers beside whom, in words: the models it names, and the speeds. */
function capCovers(cap: UsageCap, t: TFunction): string {
  const names = cap.models.map((pattern) => (isGroupPattern(pattern) ? t('models.limits.group', { name: pattern.slice(1) }) : pattern));
  const speeds = cap.tiers.length === 0 ? [] : [t('models.limits.atSpeed', { speeds: cap.tiers.map((tier)=>offeredChoice(t,'serviceTier',tier)?.name??tier).join(t('startFlow.separator')) })];
  return [...names, ...speeds].join(t('startFlow.separator'));
}

function CapLine({ status }: { status: CapStatus }) {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  const { cap } = status;
  const used = formatNumber(Number(status.used), locale);
  const of = cap.amount === null ? null : formatNumber(Number(cap.amount), locale);
  return (
    <li className="flex flex-wrap items-baseline gap-x-3 gap-y-1 px-3 py-2">
      <span className="flex min-w-0 flex-1 flex-wrap gap-x-3 gap-y-1">
        <span className="font-medium">{capWho(cap, t)}</span>
        {capCovers(cap,t)!=='' && <span className="text-text-muted">{capCovers(cap,t)}</span>}
        <span>{capWhat(cap, t, locale)}</span>
      </span>
      <span className="flex items-center gap-2 text-sm text-text-muted tabular-nums">
        {of !== null && <span>{t(cap.meter === 'running' ? 'models.limits.usedRunning' : 'models.limits.used', { used, of })}</span>}
        {status.state !== 'ok' && <Badge tone={status.state === 'reached' ? 'danger' : 'warning'}>{t(`models.limits.state.${status.state}`)}</Badge>}
        {!cap.enabled && <Badge>{t('models.limits.off')}</Badge>}
      </span>
    </li>
  );
}

/** Opens the application's limit dialog with this provider and model or group selected. */
export function AddLimit({ provider, models, label, size = 'sm' }: { provider: ProviderRef; models: string[]; label: string; size?: 'sm' | 'md' }) {
  return (
    <Button size={size} action="create" onClick={() => openCap({ provider, models, tiers: [] })}>
      {label}
    </Button>
  );
}

/** The limits that cover a model: the caps on its provider that name it, by its name, by a family or by a group it is in. */
export function ModelLimits({ provider, model }: { provider: ProviderRef; model: ProviderModel }) {
  const { t } = useTranslation();
  const describe = useErrorText();
  const query = useQuery(providerCapsQuery(provider.id));
  const covering = (query.data ?? []).filter((status) => namesModel(status.cap, model));
  const wholeProvider = (query.data ?? []).filter((status) => status.cap.models.length === 0).length;
  return (
    <div className="flex flex-col gap-2">
      <p className="text-sm text-text-muted">{t('models.limits.lead')}</p>
      {query.isPending ? (
        <SkeletonRows rows={2} />
      ) : query.isError ? (
        <p className="text-sm text-text-muted">{t('models.limits.unavailable', { reason: describe(query.error).message })}</p>
      ) : covering.length === 0 ? (
        <EmptyState compact title={t('models.limits.none')} />
      ) : (
        <ul aria-label={t('models.limits.title')} className="flex flex-col divide-y divide-border rounded-lg border border-border bg-surface text-sm">
          {covering.map((status) => (
            <CapLine key={status.cap.id} status={status} />
          ))}
        </ul>
      )}
      {wholeProvider > 0 && <p className="text-sm text-text-muted">{t('models.limits.wholeProvider', { count: wholeProvider })}</p>}
      <AddLimit provider={provider} models={[model.model_name]} label={t('models.limits.add')} />
    </div>
  );
}

/** A group's pattern as the caps name it. */
export const patternOfGroup = groupPattern;
