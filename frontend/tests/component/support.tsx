/** What a component test draws into: the few things every screen of the application stands in. */
import { render, within, type RenderResult } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import type { ReactNode } from 'react';

import { Toaster } from '@/ui/Toast';
import { TooltipProvider } from '@/ui/Tooltip';
import { TestRouter } from './TestRouter';

/** Draws something as the application would: with the timing of tooltips and the place for notices round it. */
export function show(ui: ReactNode, client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } })): RenderResult {
  const Surround = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={client}><TooltipProvider>
      <TestRouter><Toaster>{children}</Toaster></TestRouter>
    </TooltipProvider></QueryClientProvider>
  );
  return render(ui, { wrapper: Surround });
}

/** Native group headings are rows too; resource assertions address the rows that actually carry cells. */
export function tableDataRows(table: HTMLElement): HTMLElement[] {
  return within(table).getAllByRole('row').filter((row) => row.querySelector('td') !== null);
}
