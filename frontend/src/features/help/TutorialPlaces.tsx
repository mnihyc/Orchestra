/** A tutorial opens a reader-chosen owned place; enumeration starts only for a step that uses it. */
import { useQuery } from '@tanstack/react-query';
import { useRouter } from '@tanstack/react-router';
import { useTranslation } from 'react-i18next';
import { choiceOption } from '@/i18n/choices';
import { useStore } from 'zustand';

import { client } from '@/api/client';
import { environmentsQuery } from '@/api/environments';
import { openAccess } from '@/features/access/AccessDialog';
import { resourceLabels, type NamedResource } from '@/features/resources/labels';
import { useTutorialRun } from '@/features/help/tutorialState';
import { open } from '@/features/workbench/layout';
import { useShape } from '@/features/workbench/Narrow';
import { useWorkbench } from '@/features/workbench/store';
import type { Environment } from '@/generated/api-types';
import type { TutorialDestination } from '@/help/tutorials/model';
import { holds } from '@/lib/permissions';
import { Button } from '@/ui/Button';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorState } from '@/ui/ErrorState';
import { Select } from '@/ui/Select';

export interface TutorialPlace { kind: 'thread' | 'environment'; id: string; name: string; label: string }
export type TutorialEnvironment = Pick<Environment, 'id' | 'name' | 'address' | 'owner' | 'access' | 'threads'> & Partial<Pick<Environment, 'home' | 'org_id'>>;

export function tutorialPlaces(rows: readonly TutorialEnvironment[], user: string, threads: boolean): TutorialPlace[] {
  const found = new Map<string, TutorialPlace>();
  const origins = new Map<string, NamedResource>();
  for (const environment of rows) {
    if (threads) {
      for (const entry of environment.threads) {
        if (entry.owner.id !== user || entry.thread.redacted || entry.thread.name === null) continue;
        found.set(entry.thread.id, { kind: 'thread', id: entry.thread.id, name: entry.thread.name, label: entry.thread.name });
        origins.set(entry.thread.id, { id: entry.thread.id, name: entry.thread.name, home: entry.topic });
      }
    } else if (environment.owner.id === user || holds(environment.access, 'access.own')) {
      found.set(environment.id, { kind: 'environment', id: environment.id, name: environment.name, label: environment.name });
      origins.set(environment.id, environment);
    }
  }
  const labels = resourceLabels([...origins.values()], new Map());
  return [...found.values()].map((place) => ({ ...place, label: labels.get(place.id) ?? place.name })).sort((one, other) => one.label.localeCompare(other.label));
}

export function tutorialPlacePath(destination: TutorialDestination, id: string): string {
  if (destination.startsWith('thread-')) return `/thread/${encodeURIComponent(id)}`;
  const tab = destination === 'environment-ports' ? 'ports' : destination === 'environment-settings' ? 'settings' : 'overview';
  return `/resources/environments/${encodeURIComponent(id)}?tab=${tab}`;
}

export function TutorialPlaces({ destination }: { destination: TutorialDestination }) {
  const { t } = useTranslation();
  const router = useRouter();
  const user = useStore(client.session, (state) => state.userId);
  const picked = useTutorialRun((state) => state.picked);
  const change = useWorkbench((state) => state.change);
  const setOverlay = useWorkbench((state) => state.setOverlay);
  const shape = useShape();
  const rows = useQuery({ ...environmentsQuery(null), enabled: user !== null, retry: false });
  const threads = destination.startsWith('thread-');
  const choices = user === null ? [] : tutorialPlaces(rows.data ?? [], user, threads);
  const choice = choices.find((one) => one.id === picked?.id);
  const go = (place: TutorialPlace) => {
    useTutorialRun.setState({ picked: { kind: place.kind, id: place.id, name: place.name } });
    router.history.push(tutorialPlacePath(destination, place.id));
    if (destination === 'thread-parameters') {
      change((layout) => open(layout, 'parameters', 'right'));
      if (shape !== 'wide') setOverlay('right');
    } else if (destination === 'thread-access') {
      openAccess({ kind: 'thread', id: place.id, name: place.name }, { share: false });
    }
  };
  return <div className="flex flex-col gap-2" data-testid="tutorial-places">
    {rows.isPending ? <p className="text-sm text-text-muted">{t('helpScreenReview.loading')}</p>
      : rows.isError ? <ErrorState compact error={rows.error} onRetry={() => void rows.refetch()} />
      : choices.length === 0 ? <EmptyState compact title={t('helpScreenReview.empty')} />
      : <><Select label={t(threads ? 'helpScreenReview.chooseThread' : 'helpScreenReview.chooseEnvironment')}
        value={choice?.id ?? ''} options={choices.map((one) => ({ value: one.id, ...choiceOption(t, 'dynamic.place', { name: one.label }) }))}
        placeholder={t('helpScreenReview.choose')} onValueChange={(id) => { const place = choices.find((one) => one.id === id); if (place !== undefined) go(place); }} />
        {choice !== undefined && <Button size="sm" onClick={() => go(choice)}>{t('helpScreenReview.openChosen')}</Button>}</>}
  </div>;
}
