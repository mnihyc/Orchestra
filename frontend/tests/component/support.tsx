/** What a component test draws into: the few things every screen of the application stands in. */
import { render, type RenderResult } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import type { ReactNode } from 'react';

import { Toaster } from '@/ui/Toast';
import { TooltipProvider } from '@/ui/Tooltip';

/** Draws something as the application would: with the timing of tooltips and the place for notices round it. */
export function show(ui: ReactNode): RenderResult {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  const Surround = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={client}><TooltipProvider>
      <Toaster>{children}</Toaster>
    </TooltipProvider></QueryClientProvider>
  );
  return render(ui, { wrapper: Surround });
}
