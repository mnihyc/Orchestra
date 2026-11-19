// @vitest-environment node
/** A sentence of the product never shows a person a hash, an identifier, a revision counter or a serial.
 *
 * A thing is named by its name, its kind and its time. Where support or an operator needs its identifier, it is one
 * Copy ID act in the thing's details or menu (`ui/CopyId.tsx`), never a word of a sentence. Two checks keep it so:
 * the placeholders of both catalogues, and the values the sources hand to `t(...)`. What they let through is written
 * down here with its reason; a new entry needs a reason a person of the product would accept. */
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';

import english from '@/i18n/en.json';
import chinese from '@/i18n/zh-CN.json';

import { identifierArguments, identifierPlaceholders, isIdentifierName } from './identifier-words';

const sourceRoot = resolve(import.meta.dirname, '../../src');
/** Reading every source file once takes a few seconds on the shared builder. */
const SCAN_BUDGET_MS = 20_000;

/** The placeholders that may be named like an identifier, by catalogue key, with why. */
const ALLOWED_PLACEHOLDERS: Readonly<Record<string, string>> = {
  'admin.overview.build': 'The About of the system: its build number belongs to the version details, as every desktop product keeps it in its About box.',
  'places.rules.slugOffer': 'The short name of a place is what its person typed for its address.',
  'places.rules.slugTakenBy': 'The short name of a place is what its person typed for its address.',
  'places.rules.slugTakenDeleted': 'The short name of a place is what its person typed for its address.',
  'processes.confirm.interrupt': 'A process is told from another of its name by its number in every process list, and the row being acted on shows the same number.',
  'processes.confirm.kill': 'A process is told from another of its name by its number in every process list, and the row being acted on shows the same number.',
  'processes.confirm.terminate': 'A process is told from another of its name by its number in every process list, and the row being acted on shows the same number.',
  'processes.menuFor': 'A process is told from another of its name by its number in every process list, and the row being acted on shows the same number.',
  'resourceDetails.skill.origin.gitReference': 'The branch, tag or revision of a Git source is what its person typed when the skill was added.',
  'templates.source.image': 'The image reference is what its person typed or chose: a name and a tag.',
  'templates.source.imageReference': 'The image reference is what its person typed or chose: a name and a tag.',
};

/** The sources that may hand an identifier-named value to a sentence, as `file: key (property)`, with why. */
const ALLOWED_ARGUMENTS: Readonly<Record<string, string>> = {
  'features/admin/Overview.tsx: admin.overview.build (build)': 'The About of the system, as above.',
  'features/inspector/Processes.tsx: processes.confirm. (pid)': 'As above: the number of a process in a process list.',
  'features/inspector/Processes.tsx: processes.menuFor (pid)': 'As above: the number of a process in a process list.',
  'features/resources/templates/TemplateList.tsx: templates.source.image (reference)': 'As above: an image reference a person typed.',
  'features/settings/CallsRules.tsx: calls.menuOf (name)': 'The id of an automatic-retry rule is the name its person gave it.',
  'features/timeline/items/UserMessage.tsx: calls.automaticDetail (rule)': 'The id of an automatic-retry rule is the name its person gave it.',
};

describe('no machine-made string is a word of the product', () => {
  it('knows an identifier by its name', () => {
    for (const name of ['id', 'hash', 'digest', 'revision', 'snapshot_id', 'unit_name', 'request_id', 'specHash', 'machineId', 'fingerprint_sha256', 'generation', 'serial']) expect(isIdentifierName(name), name).toBe(true);
    for (const name of ['name', 'valid', 'paid', 'host', 'machine', 'version', 'count', 'time', 'size', 'state', 'identity', 'provider']) expect(isIdentifierName(name), name).toBe(false);
  });

  for (const [language, catalogue] of [['en', english], ['zh-CN', chinese]] as const) {
    it(`keeps identifier placeholders out of the ${language} catalogue except where they are named for a reason`, () => {
      const loose = identifierPlaceholders(catalogue).filter((line) => ALLOWED_PLACEHOLDERS[line.slice(0, line.indexOf(' '))] === undefined);
      expect(loose, `Show a name, a kind or a time, and keep the identifier behind Copy ID:\n${loose.join('\n')}`).toEqual([]);
    });
  }

  it('lets no allowed placeholder outlive its sentence', () => {
    const present = new Set(identifierPlaceholders(english).map((line) => line.slice(0, line.indexOf(' '))));
    expect(Object.keys(ALLOWED_PLACEHOLDERS).filter((key) => !present.has(key))).toEqual([]);
  });

  it('keeps identifiers out of what the sources hand to a sentence except where they are named for a reason', () => {
    const loose = identifierArguments(sourceRoot).filter((line) => ALLOWED_ARGUMENTS[line] === undefined);
    expect(loose, `Show a name, a kind or a time, and keep the identifier behind Copy ID:\n${loose.join('\n')}`).toEqual([]);
  }, SCAN_BUDGET_MS);

  it('lets no allowed argument outlive its call', () => {
    const present = new Set(identifierArguments(sourceRoot));
    expect(Object.keys(ALLOWED_ARGUMENTS).filter((line) => !present.has(line))).toEqual([]);
  }, SCAN_BUDGET_MS);
});
