/** Import steps allow safe return before a job and stay readable on a phone. */
import { fireEvent, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { ImportSteps } from '@/features/people/ImportSteps';
import { ViewCompactContext } from '@/features/workbench/Narrow';
import { setLocale } from '@/i18n';
import { show } from './support';

afterEach(async () => { await setLocale('en'); });

it('lets the person revisit earlier choices before starting an import', () => {
  const back = vi.fn();
  show(<ViewCompactContext.Provider value={false}><ImportSteps at="review" onBack={back} /></ViewCompactContext.Provider>);
  const steps = screen.getByRole('list', { name: 'Import steps' });
  expect(steps.querySelector('[aria-current="step"]')).toHaveTextContent('3 Review');
  fireEvent.click(screen.getByRole('button', { name: '1 Source' }));
  expect(back).toHaveBeenCalledExactlyOnceWith('source');
  expect(screen.queryByRole('button', { name: /4 Import/ })).not.toBeInTheDocument();
});

it('does not return to editable choices after the job has started', () => {
  show(<ViewCompactContext.Provider value={false}><ImportSteps at="run" onBack={vi.fn()} /></ViewCompactContext.Provider>);
  expect(screen.getByRole('list', { name: 'Import steps' }).querySelector('[aria-current="step"]')).toHaveTextContent('4 Import');
  expect(screen.queryByRole('button', { name: '1 Source' })).not.toBeInTheDocument();
});

it('says the current step in Chinese on a narrow screen', async () => {
  await setLocale('zh-CN');
  show(<ViewCompactContext.Provider value><ImportSteps at="review" onBack={vi.fn()} /></ViewCompactContext.Provider>);
  expect(screen.getByText('3 / 5 · 预览')).toHaveAttribute('aria-current', 'step');
  expect(screen.queryByRole('button', { name: /来源/ })).not.toBeInTheDocument();
});
