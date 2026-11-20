/** A failed background account check keeps its own exact retry without replacing any page. */
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { useEffect, useRef } from 'react';
import { useTranslation } from 'react-i18next';
import { useStore } from 'zustand';
import { client } from '@/api/client';
import { isReadCancelled } from '@/api/reads';
import { shellBootstrapQuery, shellCheckQuery } from '@/shell/bootstrap';
import { Button } from '@/ui/Button';
import { StateLine } from '@/ui/StateLine';

export function ShellCheckNotice() {
  const { t } = useTranslation();
  const cache = useQueryClient();
  const user = useStore(client.session, (state) => state.userId ?? '');
  const reader = useStore(client.session, (state) => state.readerVersion);
  const joined = useQuery({ ...shellCheckQuery(cache, user, reader), enabled: false });
  const initial = useQuery({ ...shellBootstrapQuery(cache, user), enabled: false });
  const query = initial.isError ? initial : joined;
  const { isError, error, refetch } = query;
  const opened = useRef<string | null>(null);
  useEffect(() => {
    const key = JSON.stringify([user, reader]);
    if (opened.current === key) return;
    opened.current = key;
    if (isError && !isReadCancelled(error)) void refetch({ cancelRefetch: false });
  }, [user, reader, isError, error, refetch]);
  useEffect(() => {
    if (!isError || isReadCancelled(error)) return;
    const retry = () => { if (document.visibilityState === 'visible') void refetch({ cancelRefetch: false }); };
    document.addEventListener('visibilitychange', retry);
    window.addEventListener('focus', retry);
    return () => { document.removeEventListener('visibilitychange', retry); window.removeEventListener('focus', retry); };
  }, [isError, error, refetch]);
  if (user === '') return null;
  if (query.isFetching && query.errorUpdatedAt > 0) return <StateLine role="status" compact message={t('ui.read.retrying')} />;
  if (!query.isError || isReadCancelled(query.error)) return null;
  return <StateLine role="alert" tone="warning" compact message={t('shell.banner.checkFailed')}
    action={<Button size="sm" onClick={() => { void refetch({ cancelRefetch: false }); }}>{t('common.tryAgain')}</Button>} />;
}
