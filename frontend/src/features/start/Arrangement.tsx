/** The arrangement a thread gets: its type, where the harness and its tools run, the executor and what the agent works with there as the environment offers it, how the credential travels, and why another type is not offered.
 *
 * How the agent and the provider will be placed, in the words of what each way puts into the
 * environment, in the order the server ranks them. The one ticked is the one that is sent, the first as much as
 * any other, and until somebody ticks another it is the one the server would take if none were named, marked as
 * such. A way that lets a credential leave the provider is drawn as a warning, and a way that cannot be had is
 * shown off with the server's own reason and is never put in place of the one that was asked for. What the thread
 * then runs on is told in its head.
 */
import { TriangleAlert } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { EmptyState } from '@/ui/EmptyState';
import { choiceOption } from '@/i18n/choices';

import type { ArrangementType, Provider } from '@/generated/api-types';
import { InlineAlert } from '@/ui/InlineAlert';
import { RadioGroup, type RadioOption } from '@/ui/RadioGroup';
import { ChoiceField } from '@/ui/ChoiceField';
import { cx } from '@/ui/cx';

import type { Resolved } from './choices';
import { agentWhy, arrangementWhy, describeIsolation } from './words';

/** The ways that let the credential, or the whole sign-in, out of the provider. */
const SENSITIVE: readonly ArrangementType[] = ['credential_shared', 'handed_over', 'together'];

/** The value of the choice that leaves the arrangement to what the thread's place and the provider allow. */
const AS_CHOSEN = '';

/**
 * The arrangement a thread moved to another provider asks for, among those its owner allows and some harness there
 * can take, each in the words of what it puts into the environment; or none, and the server takes the first it
 * ranks open there. A way that lets a credential leave the provider is drawn as a warning.
 */
export function ProviderArrangementChoice({ provider, value, onChange }: { provider: Provider; value: ArrangementType | null; onChange: (type: ArrangementType | null) => void }) {
  const { t } = useTranslation();
  const offered = provider.arrangements.filter((consent) => consent.allowed && consent.available);
  if (offered.length === 0) return <EmptyState compact title={t('start.arrangement.noneAllowed', { provider: provider.name })} />;
  const options: RadioOption<ArrangementType | typeof AS_CHOSEN>[] = [
    { value: AS_CHOSEN, ...choiceOption(t, 'arrangement.default'), description: `${t('start.arrangement.asChosen')} · ${t('start.arrangement.asChosenAbout')}` },
    ...offered.map((consent) => ({
      value: consent.type,
      ...choiceOption(t, `arrangement.${consent.type}`),
      icon: SENSITIVE.includes(consent.type) ? <TriangleAlert className="text-warning" aria-hidden /> : undefined,
    })),
  ];
  return (
    <ChoiceField label={t('start.arrangement.label')} options={options}>
    <RadioGroup
      variant="list"
      label={t('start.arrangement.label')}
      value={value ?? AS_CHOSEN}
      onValueChange={(chosen) => onChange(chosen === AS_CHOSEN ? null : chosen)}
      options={options}
    />
    </ChoiceField>
  );
}

export function ArrangementSection({ resolved, refreshing, onChange }: { resolved: Resolved; refreshing: boolean; onChange: (type: ArrangementType) => void }) {
  const { t } = useTranslation();
  const { provider, agent } = resolved;
  if (provider === null) return <p className="text-sm text-text-muted">{t('start.arrangement.pickProvider')}</p>;
  if (agent === null) {
    const unavailable = resolved.agents.find((row) => row.isDefault && !row.startable) ?? resolved.agents.find((row) => !row.startable);
    return resolved.waiting ? (
      <InlineAlert tone="info">{t('start.arrangement.waiting')}</InlineAlert>
    ) : unavailable !== undefined ? (
      <InlineAlert tone="warning">{agentWhy(unavailable, t)}</InlineAlert>
    ) : (
      <p className="text-sm text-text-muted">{t('start.arrangement.pickAgent')}</p>
    );
  }
  if (resolved.arrangements.length === 0) {
    return <p className="text-sm text-text-muted">{t('start.arrangement.unknown', { agent: agent.entry.agent.name })}</p>;
  }
  const first = resolved.arrangements[0];
  const options: RadioOption<ArrangementType>[] = resolved.arrangements.map((row) => ({
    value: row.type,
    ...choiceOption(t, `arrangement.${row.type}`),
    icon: SENSITIVE.includes(row.type) ? <TriangleAlert className="text-warning" aria-hidden /> : undefined,
    note: row.open && row.type === resolved.defaultArrangement ? t('start.arrangement.default') : undefined,
    description: describeIsolation(row.entry.isolation, t),
    ...(row.open ? {} : { disabled: arrangementWhy(row.entry, t) }),
  }));
  return (
    <div className={cx('flex flex-col gap-2', refreshing && 'opacity-60')} aria-busy={refreshing || undefined}>
      {resolved.arrangement === null && first !== undefined && (
        <InlineAlert tone="warning" title={t('start.arrangement.nothingOpen', { agent: agent.entry.agent.name, provider: provider.entry.provider.name })}>
          {arrangementWhy(first.entry, t)}
        </InlineAlert>
      )}
      <ChoiceField label={t('start.arrangement.label')} options={options}><RadioGroup
        variant="list"
        label={t('start.arrangement.label')}
        value={resolved.arrangement}
        onValueChange={onChange}
        options={options}
      /></ChoiceField>
    </div>
  );
}
