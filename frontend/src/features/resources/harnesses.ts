/** The names of harnesses as the system's adapters title them.
 */
import { useQuery } from '@tanstack/react-query';
import { useMemo } from 'react';
import { useTranslation } from 'react-i18next';

import { harnessKindsQuery } from '@/api/resources';
import { programNamesIndex } from '@/features/resources/programNames';

const EMPTY_NAMES: ReadonlyMap<string, string | null> = new Map();

/** A reported title wins; even while waiting, a protocol kind never becomes the visible name. */
export function useHarnessTitle(enabled = true): (kind: string) => string {
  return useHarnessNames(enabled).title;
}

/** Several labels share one registry read and one index, including its pending/error state. */
export function useHarnessNames(enabled = true) {
  const { t, i18n } = useTranslation();
  const kinds = useQuery({ ...harnessKindsQuery, enabled });
  const locale = i18n.resolvedLanguage ?? i18n.language;
  const names = useMemo(() => kinds.data === undefined ? EMPTY_NAMES : programNamesIndex(kinds.data, locale), [kinds.data, locale]);
  return { kinds, title: (kind: string) => names.get(kind) ?? t('resources.harnessNameUnknown') };
}
