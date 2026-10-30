/** How agents may run on this provider: the arrangement types its owner allows, what each puts into an environment, copies issued, and the return of a handed-over sign-in.
 *
 * The four types as a list with ticks, each with one sentence of what it puts into an environment, and
 * for a copy of the sign-in what that copy is and how long it lasts. Under the list, what a thread gets by
 * default with these ticks. Ticking a type that lets something leave asks first, and says what leaves;
 * at least one type stays ticked. A change is shown as asked and as waiting until the server has answered,
 * and a warning the server gives (what may now leave) is kept on the page.
 */
import { useMutation, useQueryClient } from '@tanstack/react-query';
import type { TFunction } from 'i18next';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { providerKey, updateProvider } from '@/api/resources';
import { Hint } from '@/features/help/Hint';
import type { ArrangementConsent, ArrangementType, Provider } from '@/generated/api-types';
import { Section } from '@/shell/Page';
import { Checkbox } from '@/ui/Checkbox';
import { ConfirmDialog } from '@/ui/ConfirmDialog';
import { adminSpec } from '@/ui/AdminAct';
import { ErrorAlert } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';
import { toast } from '@/ui/Toast';

/** The types in which something of the credential leaves the provider, which are asked about before they are ticked. */
const LEAVING: readonly ArrangementType[] = ['credential_shared', 'handed_over'];

const TYPES = ['separated', 'key_separated', 'credential_shared', 'handed_over', 'together'] as const satisfies readonly ArrangementType[];

type Offered = (typeof TYPES)[number];

const isOffered = (type: ArrangementType): type is Offered => (TYPES as readonly ArrangementType[]).includes(type);

/** A setup's familiar name; an unknown wire type does not become a screen label. */
export function arrangementName(t: TFunction, type: string): string {
  return (TYPES as readonly string[]).includes(type) && isOffered(type as ArrangementType) ? t(`providers.arrangements.types.${type as Offered}.name`) : t('ui.nameUnknown');
}

const MINUTE = 60;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;

/** How long a copy of the credential lasts, in the largest unit that is not a fraction: "about an hour", "3 days". */
function lasts(t: TFunction, seconds: number): string {
  if (seconds >= DAY) return t('providers.arrangements.lifetime.days', { count: Math.round(seconds / DAY) });
  if (seconds >= HOUR) return t('providers.arrangements.lifetime.hours', { count: Math.round(seconds / HOUR) });
  return t('providers.arrangements.lifetime.minutes', { count: Math.max(1, Math.round(seconds / MINUTE)) });
}

function Row({ consent, ticked, disabled, onToggle }: { consent: ArrangementConsent; ticked: boolean; disabled: boolean; onToggle: (on: boolean) => void }) {
  const { t } = useTranslation();
  const type = consent.type;
  if (!isOffered(type)) return null;
  const form = consent.form;
  return (
    <li className="px-4 py-3">
      <Checkbox
        label={arrangementName(t, type)}
        help={<Hint id={`arrangement-${type}`} />}
        hint={
          <span className="flex flex-col gap-1">
            <span>{t(`providers.arrangements.types.${type}.what`)}</span>
            {form !== null && (
              <span>
                {form.lifetime_secs === null
                  ? t('providers.arrangements.copy', { title: form.title })
                  : t('providers.arrangements.copyLasting', { title: form.title, time: lasts(t, form.lifetime_secs) })}
              </span>
            )}
            {!consent.available && <span>{t('providers.arrangements.unavailable')}</span>}
          </span>
        }
        checked={ticked}
        disabled={disabled}
        onCheckedChange={onToggle}
      />
    </li>
  );
}

export function ProviderArrangements({ provider }: { provider: Provider }) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const [asking, setAsking] = useState<ArrangementType | null>(null);
  const [warning, setWarning] = useState<string | null>(null);
  const change = useMutation({
    mutationFn: (next: ArrangementType[]) => updateProvider(provider.id, { expected_revision: provider.revision, arrangements: next }),
    onSuccess: (reply) => {
      setWarning(reply.warning ?? null);
      setAsking(null);
      toast({ tone: 'success', title: t('providers.arrangements.saved') });
    },
    // Somebody else changed the provider first: it is read again before the next try.
    onError: () => void queryClient.invalidateQueries({ queryKey: providerKey(provider.id) }),
  });

  const allowed = provider.arrangements.filter((consent) => consent.allowed).map((consent) => consent.type);
  // What was asked is shown as asked while the server has not answered.
  const shown = change.isPending ? change.variables : allowed;
  // A tick that has its answer coming is not given another beside it, and the control that was ticked keeps the keyboard while it waits.
  const toggle = (type: ArrangementType, on: boolean) => {
    if (on && provider.arrangements.find((consent)=>consent.type===type)?.available !== true) return;
    if (on && LEAVING.includes(type) && !allowed.includes(type)) {
      setAsking(type);
      return;
    }
    change.mutate(on ? [...allowed, type] : allowed.filter((other) => other !== type));
  };

  const asked = asking === null || !isOffered(asking) ? null : asking;
  return (
    <div className="flex flex-col gap-6">
      <Section id="provider-arrangements" title={t('providers.arrangements.title')} lead={t('providers.arrangements.lead')}>
        <ul aria-label={t('providers.arrangements.title')} aria-busy={change.isPending || undefined} className="flex flex-col divide-y divide-border rounded-lg border border-border bg-surface">
          {provider.arrangements.map((consent) => (
            <Row
              key={consent.type}
              consent={consent}
              ticked={shown.includes(consent.type)}
              // The last tick stays: a provider that no arrangement allows could be used by nobody.
              disabled={change.isPending || !consent.available && !shown.includes(consent.type) || shown.length === 1 && shown.includes(consent.type)}
              onToggle={(on) => !change.isPending && toggle(consent.type, on)}
            />
          ))}
        </ul>
        {provider.arrangements.some((consent)=>!consent.available) && <p className="text-sm text-text-muted">{t('providers.arrangements.machineRestricted')}</p>}
        <p className="text-sm text-text-muted">{shown.includes('credential_shared') ? t('providers.arrangements.defaultShared') : t('providers.arrangements.defaultHere')}</p>
        {warning !== null && <InlineAlert tone="warning">{warning}</InlineAlert>}
        {change.error !== null && asking === null && <ErrorAlert error={change.error} />}
      </Section>
      <ConfirmDialog
        admin={adminSpec(provider, 'provider.manage')}
        open={asked !== null}
        onOpenChange={(next) => !next && setAsking(null)}
        title={asked === null ? '' : t('providers.arrangements.askTitle', { type: t(`providers.arrangements.types.${asked}.name`) })}
        confirmLabel={asked === null ? '' : t('providers.arrangements.askConfirm', { type: t(`providers.arrangements.types.${asked}.name`) })}
        danger
        pending={change.isPending}
        error={change.error ?? undefined}
        onConfirm={() => asked !== null && change.mutate([...allowed, asked])}
      >
        {asked === null ? '' : t(`providers.arrangements.types.${asked}.leaves`)}
      </ConfirmDialog>
    </div>
  );
}
