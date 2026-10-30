/** Performance's frame exposes the requested views and chart measures without inventing observations. */
import { screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { PerformanceView } from '@/features/usage/PerformanceView';
import { show } from './support';

it('names each figure, keeps missing observations explicit and offers the three chart measures', async () => {
  const onMeasure = vi.fn();
  show(<PerformanceView measure="speed" onMeasure={onMeasure} />);
  expect(screen.getByRole('list', { name: 'Performance figures' })).toHaveTextContent('Median speed');
  expect(screen.getByRole('list', { name: 'Performance figures' })).toHaveTextContent('Median first output');
  expect(screen.getByRole('list', { name: 'Performance figures' })).toHaveTextContent('Error rate');
  expect(screen.getAllByRole('button', { name: 'Not measured' }).length).toBeGreaterThanOrEqual(3);
  expect(screen.getByRole('region', { name: 'Models' })).toBeInTheDocument();
  await userEvent.setup().click(screen.getByRole('combobox', { name: 'Measure' }));
  await userEvent.setup().click(screen.getByRole('option', { name: 'First output' }));
  expect(onMeasure).toHaveBeenCalledWith('first_token');
});
