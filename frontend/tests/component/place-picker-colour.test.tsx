/** A place's custom colour survives the summary used by a picker and its chosen value. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { screen } from '@testing-library/react';
import { expect, it } from 'vitest';
import { PlacePicker } from '@/features/settings/PlacePicker';
import { show } from './support';

it('draws the chosen place’s own colour instead of the palette', () => {
  const cache = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } });
  cache.setQueryData(['orgs'], []);
  const drawn = show(<QueryClientProvider client={cache}><PlacePicker label="Place" value={{ id: 'web', kind: 'project', color: null, custom_color: '#3a7bd5', folder: false, path: 'Acme / Web' }} onChange={() => undefined} /></QueryClientProvider>);
  expect(screen.getByText('Acme / Web')).toBeInTheDocument();
  expect(drawn.container.querySelector('[data-node-icon]')).toHaveAttribute('data-node-color', 'own');
  expect(drawn.container.querySelector('[data-node-icon]')).toHaveAttribute('style', '--node-own: #3a7bd5;');
});
