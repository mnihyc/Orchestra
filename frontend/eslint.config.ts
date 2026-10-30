/** Frontend lint configuration placeholder; no rules are wired yet.
 * The lint rules of the web application: TypeScript's strict, type-aware set (which refuses `any`),
 * the rules of hooks, and the rules that keep the application's own promises: every sentence a person
 * reads comes from a catalogue, pages never reach past the shared controls into the primitives under
 * them, and HTTP is spoken in one place.
 */
import js from '@eslint/js';
import { defineConfig, globalIgnores } from 'eslint/config';
import i18next from 'eslint-plugin-i18next';
import reactHooks from 'eslint-plugin-react-hooks';
import globals from 'globals';
import tseslint from 'typescript-eslint';
import {utilityPlugin} from './build/utility-classes.ts';

/** Attributes whose value a person reads or hears, and which therefore take no literal. */
const SPOKEN_ATTRIBUTES = [
  'alt',
  'aria-description',
  'aria-label',
  'aria-placeholder',
  'aria-roledescription',
  'aria-valuetext',
  'description',
  'hint',
  'label',
  'placeholder',
  'title',
];

export default defineConfig([
  globalIgnores([
    'dist',
    'node_modules',
    'playwright-report',
    'test-results',
    'tmp',
    'public',
    // Written by generators, never by hand.
    'src/generated',
    'src/routeTree.gen.ts',
  ]),
  js.configs.recommended,
  tseslint.configs.strictTypeChecked,
  tseslint.configs.stylisticTypeChecked,
  {
    languageOptions: {
      parserOptions: { projectService: true, tsconfigRootDir: import.meta.dirname },
    },
    rules: {
      '@typescript-eslint/consistent-type-imports': ['error', { fixStyle: 'inline-type-imports' }],
      '@typescript-eslint/no-unused-vars': ['error', { argsIgnorePattern: '^_', varsIgnorePattern: '^_' }],
      // A number in a template is what one means; everything else is said explicitly.
      '@typescript-eslint/restrict-template-expressions': ['error', { allowNumber: true }],
      // An event handler may be asynchronous: React ignores what it returns.
      '@typescript-eslint/no-misused-promises': ['error', { checksVoidReturn: { attributes: false } }],
      '@typescript-eslint/no-confusing-void-expression': ['error', { ignoreArrowShorthand: true }],
      // Routes are redirected, and refused, by throwing what the router gives for it.
      '@typescript-eslint/only-throw-error': [
        'error',
        { allow: [{ from: 'package', package: '@tanstack/router-core', name: ['Redirect', 'NotFoundError'] }] },
      ],
      eqeqeq: ['error', 'always'],
      'no-console': ['error', { allow: ['warn', 'error'] }],
    },
  },
  {
    files: ['src/**/*.{ts,tsx}', 'tests/unit/**', 'tests/component/**', 'tests/setup.ts'],
    languageOptions: { globals: globals.browser },
  },
  {
    files: ['*.config.ts', 'e2e/**', 'tests/doubles/**', 'tests/e2e/**', 'tests/accessibility/**'],
    languageOptions: { globals: globals.node },
  },
  {
    files: ['src/**/*.{ts,tsx}'],
    extends: [reactHooks.configs.flat['recommended-latest']],
    plugins:{orch:utilityPlugin},
    rules: {
      'orch/built-utilities':'error',
      'no-restricted-imports': [
        'error',
        {
          patterns: [
            {
              group: ['radix-ui', '@radix-ui/*'],
              message: 'Primitives are wrapped once, in src/ui; a screen uses the shared control.',
            },
            {
              group: ['openapi-fetch'],
              message: 'HTTP is spoken by src/api/client.ts alone.',
            },
          ],
        },
      ],
      'no-restricted-globals': [
        'error',
        { name: 'fetch', message: 'HTTP is spoken by src/api/client.ts alone, through TanStack Query.' },
      ],
    },
  },
  {
    files: ['src/ui/**/*.{ts,tsx}'],
    rules: { 'no-restricted-imports': 'off' },
  },
  {
    files: ['src/api/client.ts'],
    rules: { 'no-restricted-imports': 'off', 'no-restricted-globals': 'off' },
  },
  {
    // Every sentence a person reads comes from a catalogue: no literal text in markup, and none in
    // an attribute that is read or spoken.
    files: ['src/**/*.tsx'],
    plugins: { i18next },
    rules: {
      'i18next/no-literal-string': [
        'error',
        {
          mode: 'jsx-only',
          // Preserve the plugin's translation/metadata exclusions, adding only the permission binding.
          callees: { exclude: ['i18n(ext)?', 't', 'require', 'addEventListener', 'removeEventListener', 'postMessage', 'getElementById', 'dispatch', 'commit', 'includes', 'indexOf', 'endsWith', 'startsWith', 'adminSpec'] },
          'jsx-attributes': { include: SPOKEN_ATTRIBUTES },
          // Punctuation and digits between two translated parts are no sentence.
          words: { exclude: ['[0-9!-/:-@[-`{-~\\s·…—–→←↑↓⌘⌥⇧⌃]+'] },
        },
      ],
    },
  },
  {
    files: ['tests/**', 'e2e/**'],
    rules: {
      // A test says what it expects in the words of the screen.
      '@typescript-eslint/no-non-null-assertion': 'off',
    },
  },
]);
