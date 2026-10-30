/** The page a thread will have, where one is created, forked or switched: one line when it follows, the page of a fork's source carried over, and what changes between a managed environment and a person's machine.
 *
 * A thread's page exists from the moment the thread does, so the dialog that makes one shows it before
 * anything is made: in one line when it simply follows the defaults of where it is made ("Access: as Fix
 * login decides · Developers set it up, Interactors talk to the agent, everyone watches"), and offers to
 * make it private before it is. On a person's machine the same defaults say that only its owner
 * works with the thread. A fork chooses between the page of the thread it was taken from and what the
 * place decides, and the dialog shows what each comes to.
 */
import { useQuery } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';
import { choiceOption } from '@/i18n/choices';

import { type AccessTarget, defaultsQuery, permissionsQuery } from '@/api/access';
import type { AccessSummary } from '@/generated/api-types';
import { ErrorState } from '@/ui/ErrorState';
import { RadioGroup } from '@/ui/RadioGroup';
import { ChoiceField } from '@/ui/ChoiceField';
import { Skeleton } from '@/ui/Skeleton';
import { GeneralAccessControl } from './GeneralAccess';
import { holderName } from './holders';
import { type GeneralAccessPreset, indexCatalogue, linesOf } from './model';

/** How a new thread starts, short of a change: what the form of the dialog says. */
export type NewPagePreset = GeneralAccessPreset;

export interface NewPageProps {
  /** The topic the thread is made in: the place whose defaults its page follows. */
  place: AccessTarget;
  /** Where the thread will run, which decides which of the two rows of each default it follows. */
  placement: 'managed' | 'connected';
  preset: NewPagePreset;
  onPresetChange: (preset: NewPagePreset) => void;
  letIntoEnvironment: boolean;
  onLetIntoEnvironmentChange: (checked: boolean) => void;
  /** A project creates a new topic first; its audience is that new topic, rather than the project. */
  audienceName?: string;
  /** Binding options already expose these defaults to a creator who cannot manage the topic's defaults. */
  summary?: AccessSummary;
  /** For a fork: the thread it is taken from, and which page it starts with. */
  fork?: { source: string; value: 'original' | 'place'; onChange: (value: 'original' | 'place') => void } | undefined;
}

export function NewPage({ place, placement, preset, onPresetChange, letIntoEnvironment, onLetIntoEnvironmentChange, audienceName, summary, fork }: NewPageProps) {
  const { t } = useTranslation();
  const defaults = useQuery({ ...defaultsQuery(place.id), enabled: summary === undefined });
  const catalogue = useQuery(permissionsQuery);
  if (catalogue.isError) return <ErrorState compact error={catalogue.error} onRetry={() => void catalogue.refetch()} />;
  if (summary === undefined && defaults.isError) return <ErrorState compact error={defaults.error} onRetry={() => void defaults.refetch()} />;
  if (summary === undefined && defaults.data === undefined || catalogue.data === undefined) return <Skeleton className="h-5 w-3/4" />;

  const index = indexCatalogue(catalogue.data);
  // The page of a thread made here: the rows of a thread, for work where it will run.
  const rows = defaults.data?.rows.filter((row) => index.defs.get(row.permission)?.on.includes('thread') === true && (row.profile === 'any' || row.profile === placement)) ?? [];
  const lines = defaults.data === undefined ? null : linesOf({ ...defaults.data, rows }, index, 'thread');
  const said = (['manage', 'interact', 'view'] as const).flatMap((line) =>
    (summary?.[line] ?? lines?.[line].map((holder) => holder.holder) ?? [])
      .map((holder) =>
        t('access.roles.does', { who: holderName(t, holder, audienceName ?? place.name), what: t(`access.roles.verb.thread.${line}`) }),
      ),
  );

  const forkOptions = [
    { value: 'original' as const, ...choiceOption(t, 'newPage.original', { source: fork?.source ?? '' }) },
    { value: 'place' as const, ...choiceOption(t, 'newPage.place', { place: place.name }) },
  ];
  return (
    <div className="flex flex-col gap-3">
      <GeneralAccessControl catalogue={index} kind="thread" place={audienceName ?? place.name} value={preset} onChange={onPresetChange}
        summary={said.length === 0 ? t('access.newPage.ownerOnly') : said.join('; ')}
        environment={{ checked: letIntoEnvironment, onChange: onLetIntoEnvironmentChange }} />
      {fork !== undefined && (
        <ChoiceField label={t('access.newPage.forkLabel')} options={forkOptions}>
        <RadioGroup<'original' | 'place'>
          label={t('access.newPage.forkLabel')}
          variant="list"
          value={fork.value}
          onValueChange={fork.onChange}
          options={forkOptions}
        />
        </ChoiceField>
      )}
      {placement === 'connected' && <p className="text-sm text-text-muted">{t('access.newPage.machine')}</p>}
    </div>
  );
}
