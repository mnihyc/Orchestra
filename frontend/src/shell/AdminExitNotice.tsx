/** The actual destination says why the previous page could no longer stay open. */
import { useRouterState } from '@tanstack/react-router';
import { useEffect } from 'react';
import { useTranslation } from 'react-i18next';
import { useStore } from 'zustand';
import { client } from '@/api/client';
import { useAdminExitNotice } from '@/shell/adminExit';
import { Banner } from '@/ui/Banner';

export function AdminExitNotice() {
  const { t } = useTranslation();
  const href = useRouterState({ select: (state) => state.location.href });
  const person = useStore(client.session, (state) => state.userId);
  const notice = useAdminExitNotice((state) => state.notice);
  const current = notice !== null && notice.person === person && notice.href === href;
  useEffect(() => {
    if (notice !== null && !current && useAdminExitNotice.getState().notice === notice) useAdminExitNotice.setState({ notice: null });
  }, [notice, current]);
  return current ? <Banner tone="info" onDismiss={() => useAdminExitNotice.setState({ notice: null })}>{t('adminReach.mode.leftPage')}</Banner> : null;
}
