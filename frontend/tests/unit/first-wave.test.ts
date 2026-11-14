/** The page names the files its entry would only find once it had run, so that they are fetched with it. */
import { expect, it } from 'vitest';

import { type BuiltChunk, firstWaveFiles } from '../../build/first-wave';

const chunk = (fileName: string, rest: Partial<BuiltChunk> = {}): BuiltChunk => ({ type: 'chunk', fileName, ...rest });

it('names the workbench frame and the English words, and nothing else', () => {
  const bundle = {
    'assets/initial-aaa.js': chunk('assets/initial-aaa.js', { name: 'initial' }),
    'assets/shell-bbb.js': chunk('assets/shell-bbb.js', { name: 'shell' }),
    'assets/en-ccc.js': chunk('assets/en-ccc.js', { name: 'en', facadeModuleId: '/work/frontend/src/i18n/en.json' }),
    'assets/zh-CN-ddd.js': chunk('assets/zh-CN-ddd.js', { name: 'zh-CN', facadeModuleId: '/work/frontend/src/i18n/zh-CN.json' }),
    'assets/Thread-eee.js': chunk('assets/Thread-eee.js', { name: 'Thread', facadeModuleId: '/work/frontend/src/features/thread/Thread.tsx' }),
    'assets/inter-fff.woff2': { type: 'asset', fileName: 'assets/inter-fff.woff2' },
  };
  expect(firstWaveFiles(bundle)).toEqual(['assets/en-ccc.js', 'assets/shell-bbb.js']);
});
