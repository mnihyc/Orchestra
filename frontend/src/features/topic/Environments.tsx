/** The environments this topic's threads use: owner, place, threads bound, mounts, the default for new threads, and adding one.
 *
 * The environments the threads of a topic run in, and those made for it, as the topic names them: each
 * with its name, its owner, where it comes from, how it stands and whether it is the one new threads
 * join. The topic owns none of them; each belongs to the person who made it. What is only known to the
 * environment itself (its mounts, how long it stays awake, which threads are in it) is on its own page,
 * which each name leads to.
 */
import { Link } from '@tanstack/react-router';
import { Box, Laptop, Server } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import type { EnvironmentRef, Topic } from '@/generated/api-types';
import { PortsExposure } from '@/features/ports/Exposure';
import { ResourceHint, ResourceName } from '@/features/resource-facts/ResourceFacts';
import { cx } from '@/ui/cx';
import { EmptyState } from '@/ui/EmptyState';
import { EnvironmentDot } from '@/ui/EnvironmentDot';
import { InlineAlert } from '@/ui/InlineAlert';
import { UserChip } from '@/ui/UserChip';

/** How many environments a row of a list draws a dot for, before it counts the rest. */
const DOTS_SHOWN = 3;

/** The environments of a topic as one dot each, up to three and then how many more. */
export function EnvironmentDots({ environments, className }: { environments: readonly EnvironmentRef[]; className?: string }) {
  const { t } = useTranslation();
  if (environments.length === 0) return null;
  return (
    <span className={cx('inline-flex items-center', className)}>
      {environments.slice(0, DOTS_SHOWN).map((environment) => (
        <ResourceName key={environment.id} subject={{ kind: 'environment', environment }} hint={false}><EnvironmentDot status={environment.status} name={environment.name} /></ResourceName>
      ))}
      {environments.length > DOTS_SHOWN && (
        <span className="ml-1 text-sm text-text-muted">{t('places.environments.more', { count: environments.length - DOTS_SHOWN })}</span>
      )}
    </span>
  );
}

function EnvironmentCard({ environment, isDefault }: { environment: EnvironmentRef; isDefault: boolean }) {
  const { t } = useTranslation();
  const Icon = environment.placement === 'connected' ? Laptop : Server;
  return (
    <li className="flex flex-col gap-3 rounded-lg border border-border bg-surface p-4">
      <div className="flex items-start gap-3">
        <span className="flex size-8 shrink-0 items-center justify-center rounded-md bg-bg-muted text-text-muted">
          <Icon aria-hidden />
        </span>
        <div className="flex min-w-0 flex-1 flex-col">
          <ResourceName subject={{ kind: 'environment', environment }} asChild><Link to="/resources/environments/$id" params={{ id: environment.id }} className="font-medium wrap-anywhere text-text hover:underline">
            {environment.name}
          </Link></ResourceName><ResourceHint subject={{ kind: 'environment', environment }} />
          <PortsExposure environment={environment} />
        </div>
        <EnvironmentDot status={environment.status} showLabel />
      </div>
      <div className="flex flex-wrap items-center justify-between gap-2 text-sm text-text-muted">
        <span className="inline-flex items-center gap-2">
          {t('places.environments.ownedBy')} <UserChip user={environment.owner} size={24} thing={{ kind: 'environment', id: environment.id }} />
        </span>
        <span>{environment.placement === 'connected' ? t('places.environments.onMachine') : t('places.environments.onHost')}</span>
      </div>
      {isDefault && <p className="text-sm font-medium text-accent-text">{t('places.environments.default')}</p>}
    </li>
  );
}

export function TopicEnvironments({ topic }: { topic: Pick<Topic, 'environments' | 'default_environment_id'> }) {
  const { t } = useTranslation();
  const environments = [...topic.environments].sort((one, other) => Number(other.id === topic.default_environment_id) - Number(one.id === topic.default_environment_id));
  if (environments.length === 0) {
    return <EmptyState icon={<Box aria-hidden />} title={t('places.environments.empty')} description={t('places.environments.emptyAbout')} />;
  }
  return (
    <div className="flex flex-col gap-4">
      <p className="max-w-prose text-sm text-text-muted">{t('places.environments.lead')}</p>
      <ul className="grid grid-cols-1 gap-3 md:grid-cols-2">
        {environments.map((environment) => (
          <EnvironmentCard key={environment.id} environment={environment} isDefault={environment.id === topic.default_environment_id} />
        ))}
      </ul>
      <InlineAlert tone="info">{t('places.environments.ownPage')}</InlineAlert>
    </div>
  );
}
