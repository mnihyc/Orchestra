/** A setup reason is shown once with the act that addresses it; observers can read the reason too. */
import { PlugZap } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import type { Provider } from '@/generated/api-types';
import { useFormatContext } from '@/lib/appearance';
import { useNow } from '@/lib/time';
import { Button } from '@/ui/Button';
import { InlineAlert } from '@/ui/InlineAlert';
import { PROVIDER_NEXT, providerState } from './state';
import type { ProviderTab } from './Provider';

export function ProviderSetupNotice({ provider, manage, onTab, currentTab }: {
  provider: Provider; manage: boolean; onTab: (tab: 'key' | 'models' | 'runtime' | 'settings') => void;
  /** The recovery panel already provides the instructions and its actual errors. */
  currentTab?:ProviderTab;
}) {
  const { t } = useTranslation();
  const format = useFormatContext();
  const state = providerState(provider, t, format, useNow());
  if (state.next === null || !manage && provider.status !== 'needs_setup') return null;
  const step = PROVIDER_NEXT[state.next];
  if(manage && (currentTab===step.tab || currentTab==='settings' && step.tab==='runtime'))return null;
  return <InlineAlert tone="warning" title={state.text}
    action={manage ? <Button size="sm" icon={<PlugZap aria-hidden />} onClick={() => onTab(step.tab)}>{t(step.label)}</Button> : undefined}>
    {state.detail}
  </InlineAlert>;
}
