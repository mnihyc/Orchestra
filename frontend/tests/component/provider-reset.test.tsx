/** A reset requires an explicit focused confirmation and keeps its request identity after a lost reply. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { client } from '@/api/client';
import { consumeReset } from '@/api/provider-plan';
import { ProviderReset } from '@/features/resources/providers/Reset';
import { planProvider } from '../unit/provider-plan-fixtures';
import { show } from './support';
import { i18n } from '@/i18n';

vi.mock('@/api/provider-plan', () => ({ consumeReset:vi.fn() }));
vi.mock('@tanstack/react-router', () => ({ Link:({ children }: {children:React.ReactNode}) => <span>{children}</span> }));
vi.mock('@/features/help/open', () => ({useHelpOpener:() => vi.fn()}));
afterEach(() => { client.session.setState({ status:'signed_out',userId:null }); vi.clearAllMocks(); });

it('focuses Cancel and only an explicit reset button can consume, reusing the same request when unknown', async () => {
  client.session.setState({ status:'signed_in',userId:'ada' });
  const provider = planProvider();
  provider.facts.available_resets!.credits![0]!.expires_at = '2099-01-01T00:00:00.000Z';
  vi.mocked(consumeReset).mockRejectedValue(new Error('reply lost'));
  show(<QueryClientProvider client={new QueryClient({ defaultOptions:{ mutations:{ retry:false } } })}><ProviderReset provider={provider} /></QueryClientProvider>);
  const user = userEvent.setup();
  await user.click(screen.getByRole('button', { name:i18n.t('providerPlan.reset.open') }));
  await waitFor(() => expect(screen.getByRole('button',{ name:'Cancel' })).toHaveFocus());
  await user.keyboard('{Enter}');
  expect(consumeReset).not.toHaveBeenCalled();
  await user.click(screen.getByRole('button', { name:i18n.t('providerPlan.reset.open') }));
  await user.click(screen.getByRole('button',{ name:'Use this reset' }));
  await screen.findByText(i18n.t('providerPlan.reset.unknown'));
  const first = vi.mocked(consumeReset).mock.calls[0]![1];
  await user.click(within(screen.getByRole('dialog')).getByRole('button',{ name:i18n.t('providerPlan.reset.check') }));
  await waitFor(() => expect(consumeReset).toHaveBeenCalledTimes(2));
  expect(vi.mocked(consumeReset).mock.calls[1]![1]).toEqual(first);
});

it('offers a listed reset for a key when its source spends resets, even without usage windows', async () => {
  client.session.setState({status:'signed_in',userId:'ada'});
  const provider=planProvider({kind:'api'});
  provider.facts.windows=[];
  provider.facts.available_resets!.credits![0]!.expires_at=null;
  vi.mocked(consumeReset).mockResolvedValue({outcome:'The account allowance was restored.',provider});
  show(<ProviderReset provider={provider} />);
  await userEvent.click(screen.getByRole('button',{name:i18n.t('providerPlan.reset.open')}));
  expect(screen.getByRole('button',{name:'Use this reset'})).toBeEnabled();
  await userEvent.click(screen.getByRole('button',{name:'Use this reset'}));
  expect(await screen.findByText('The source answered this reset request. Its outcome is shown below.')).toBeVisible();
  expect(screen.getByText(/The account allowance was restored\./)).toBeVisible();
  expect(screen.queryByText(i18n.t('providerPlan.reset.unknown'))).not.toBeInTheDocument();
});

it('offers no reset for a sign-in when its source does not spend resets', () => {
  client.session.setState({status:'signed_in',userId:'ada'});
  const provider=planProvider({kind:'subscription'});
  provider.sources.facts.descriptor!.facts!.spends_resets=false;
  show(<ProviderReset provider={provider} />);
  expect(screen.queryByRole('button',{name:i18n.t('providerPlan.reset.open')})).not.toBeInTheDocument();
});
