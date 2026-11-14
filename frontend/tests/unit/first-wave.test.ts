/** The page names the files its entry would only find once it had run, so that they are fetched with it. */
import { expect, it } from 'vitest';

import { type BuiltChunk, firstWaveFiles } from '../../build/first-wave';
import { vocabularyPlan } from '../../build/vocabulary';

const chunk = (fileName: string, rest: Partial<BuiltChunk> = {}): BuiltChunk => ({ type: 'chunk', fileName, ...rest });

it('preloads the frame without transferring another reader\'s whole English catalogue', () => {
  const bundle = {
    'assets/initial-aaa.js': chunk('assets/initial-aaa.js', { name: 'initial' }),
    'assets/shell-bbb.js': chunk('assets/shell-bbb.js', { name: 'shell' }),
    'assets/en-ccc.js': chunk('assets/en-ccc.js', { name: 'en', facadeModuleId: '/work/frontend/src/i18n/en.json' }),
    'assets/zh-CN-ddd.js': chunk('assets/zh-CN-ddd.js', { name: 'zh-CN', facadeModuleId: '/work/frontend/src/i18n/zh-CN.json' }),
    'assets/Thread-eee.js': chunk('assets/Thread-eee.js', { name: 'Thread', facadeModuleId: '/work/frontend/src/features/thread/Thread.tsx' }),
    'assets/inter-fff.woff2': { type: 'asset', fileName: 'assets/inter-fff.woff2' },
  };
  expect(firstWaveFiles(bundle)).toEqual(['assets/shell-bbb.js']);
});

it('keeps feature words outside the first vocabulary and gives both languages the same parts', { timeout: 120_000 }, async () => {
  const plan = await vocabularyPlan(process.cwd());
  const initial = plan.parts.get('initial');
  expect(initial).toBeDefined();
  expect(initial?.keys.has('common.cancel')).toBe(true);
  expect(initial?.keys.has('settings.interface.reading.title')).toBe(false);
  const feature = plan.moduleParts.get('routes/settings/interface/index.tsx');
  expect(feature).toBeDefined();
  expect(plan.parts.get(feature ?? '')?.keys.has('settings.interface.reading.title')).toBe(true);
  for (const part of plan.parts.values()) expect(Object.keys(part.words.en)).toEqual(Object.keys(part.words['zh-CN']));
});
