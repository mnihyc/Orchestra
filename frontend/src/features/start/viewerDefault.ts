/** One server-resolved default shared by inline and full creation. */
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useStore } from 'zustand';
import { client } from '@/api/client';
import type { StartOptions } from '@/api/start';

export const VIEWERS_IN_SETTING = 'threads.let_viewers_in';

export function useViewerDefault(at: string, options: StartOptions | null, initial?: boolean) {
  const { t } = useTranslation();
  const actor = useStore(client.session, (state) => state.userId);
  const version = useStore(client.session, (state) => state.readerVersion ?? 0);
  const [chosen, setChosen] = useState<{ at: string; actor: string | null; version: number; value: boolean } | null>(
    () => initial === undefined ? null : { at, actor, version, value: initial },
  );
  const fact = options?.viewer_default;
  const value = (chosen?.at === at && chosen.actor === actor && chosen.version === version ? chosen.value : undefined) ?? fact?.value;
  const source = fact?.source.node?.name ?? (fact?.source.scope === 'default' ? t('resources.settings.sourceDefault')
    : fact?.source.scope === 'system' ? t('startImplicit.sourceSystem') : t('startImplicit.sourceUnreported'));
  return { value, ready: value !== undefined, onChange: (value: boolean) => setChosen({ at, actor, version, value }), reset:()=>setChosen(null),
    tooltip: t('startImplicit.viewerDefault', { key: VIEWERS_IN_SETTING, source }) };
}
