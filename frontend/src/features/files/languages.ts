/** The languages the code viewer highlights, each loaded when a file of it is first opened; anything else is text.
 *
 * A language is one entry: the names, extensions and first lines a file is known by, and the module
 * that parses it, which is fetched the first time a file of that language is opened and not before.
 * The list is the second of the two lists a further kind of file is added to (the first is the list
 * of viewers): a further language is a line here.
 */
import type { StreamParser } from '@codemirror/language';
import type { Extension } from '@codemirror/state';

export interface Language {
  /** The name it is kept under, and shown under. */
  id: string;
  /** Its name in words: a proper noun, the same in every language of the interface. */
  name: string;
  /** The extensions a file of it ends in, lower case, without the dot. */
  extensions: readonly string[];
  /** Whole file names that are of it, lower case: `dockerfile`, `makefile`. */
  filenames?: readonly string[];
  /** The first line of a script that names its interpreter. */
  firstLine?: RegExp;
  /** Brings the parser. */
  load: () => Promise<Extension>;
}

/** Wraps an old-style parser as a language of the editor. */
async function legacy(parser: Promise<StreamParser<unknown>>): Promise<Extension> {
  const [{ StreamLanguage }, mode] = await Promise.all([import('@codemirror/language'), parser]);
  return StreamLanguage.define(mode);
}

const clike = async (name: 'csharp' | 'kotlin' | 'scala'): Promise<Extension> => legacy(import('@codemirror/legacy-modes/mode/clike').then((module) => module[name]));

export const LANGUAGES: readonly Language[] = [
  {
    id: 'javascript',
    name: 'JavaScript',
    extensions: ['js', 'mjs', 'cjs'],
    firstLine: /^#!.*\b(node|deno|bun)\b/,
    load: () => import('@codemirror/lang-javascript').then((module) => module.javascript()),
  },
  {
    id: 'jsx',
    name: 'JSX',
    extensions: ['jsx'],
    load: () => import('@codemirror/lang-javascript').then((module) => module.javascript({ jsx: true })),
  },
  {
    id: 'typescript',
    name: 'TypeScript',
    extensions: ['ts', 'mts', 'cts'],
    load: () => import('@codemirror/lang-javascript').then((module) => module.javascript({ typescript: true })),
  },
  {
    id: 'tsx',
    name: 'TSX',
    extensions: ['tsx'],
    load: () => import('@codemirror/lang-javascript').then((module) => module.javascript({ typescript: true, jsx: true })),
  },
  {
    id: 'python',
    name: 'Python',
    extensions: ['py', 'pyw', 'pyi'],
    firstLine: /^#!.*\bpython/,
    load: () => import('@codemirror/lang-python').then((module) => module.python()),
  },
  { id: 'rust', name: 'Rust', extensions: ['rs'], load: () => import('@codemirror/lang-rust').then((module) => module.rust()) },
  { id: 'go', name: 'Go', extensions: ['go'], load: () => import('@codemirror/lang-go').then((module) => module.go()) },
  { id: 'java', name: 'Java', extensions: ['java'], load: () => import('@codemirror/lang-java').then((module) => module.java()) },
  { id: 'kotlin', name: 'Kotlin', extensions: ['kt', 'kts'], load: () => clike('kotlin') },
  {
    id: 'cpp',
    name: 'C and C++',
    extensions: ['c', 'h', 'cc', 'cpp', 'cxx', 'hh', 'hpp', 'hxx', 'ino'],
    load: () => import('@codemirror/lang-cpp').then((module) => module.cpp()),
  },
  { id: 'csharp', name: 'C#', extensions: ['cs', 'csx'], load: () => clike('csharp') },
  { id: 'swift', name: 'Swift', extensions: ['swift'], load: () => legacy(import('@codemirror/legacy-modes/mode/swift').then((module) => module.swift)) },
  { id: 'php', name: 'PHP', extensions: ['php', 'phtml'], load: () => import('@codemirror/lang-php').then((module) => module.php()) },
  {
    id: 'ruby',
    name: 'Ruby',
    extensions: ['rb', 'rake', 'gemspec', 'ru'],
    filenames: ['gemfile', 'rakefile'],
    firstLine: /^#!.*\bruby/,
    load: () => legacy(import('@codemirror/legacy-modes/mode/ruby').then((module) => module.ruby)),
  },
  { id: 'lua', name: 'Lua', extensions: ['lua'], load: () => legacy(import('@codemirror/legacy-modes/mode/lua').then((module) => module.lua)) },
  { id: 'r', name: 'R', extensions: ['r'], load: () => legacy(import('@codemirror/legacy-modes/mode/r').then((module) => module.r)) },
  { id: 'scala', name: 'Scala', extensions: ['scala', 'sc'], load: () => clike('scala') },
  {
    id: 'perl',
    name: 'Perl',
    extensions: ['pl', 'pm', 't'],
    firstLine: /^#!.*\bperl/,
    load: () => legacy(import('@codemirror/legacy-modes/mode/perl').then((module) => module.perl)),
  },
  {
    id: 'shell',
    name: 'Shell',
    extensions: ['sh', 'bash', 'zsh', 'ksh', 'env'],
    filenames: ['.bashrc', '.zshrc', '.profile', '.bash_profile', 'makefile', 'gnumakefile'],
    firstLine: /^#!.*\b(ba|z|k|da)?sh\b/,
    load: () => legacy(import('@codemirror/legacy-modes/mode/shell').then((module) => module.shell)),
  },
  {
    id: 'powershell',
    name: 'PowerShell',
    extensions: ['ps1', 'psm1', 'psd1'],
    load: () => legacy(import('@codemirror/legacy-modes/mode/powershell').then((module) => module.powerShell)),
  },
  { id: 'sql', name: 'SQL', extensions: ['sql'], load: () => import('@codemirror/lang-sql').then((module) => module.sql()) },
  {
    id: 'html',
    name: 'HTML',
    extensions: ['html', 'htm', 'xhtml', 'svelte'],
    load: () => import('@codemirror/lang-html').then((module) => module.html()),
  },
  { id: 'css', name: 'CSS', extensions: ['css'], load: () => import('@codemirror/lang-css').then((module) => module.css()) },
  { id: 'scss', name: 'SCSS', extensions: ['scss', 'sass'], load: () => import('@codemirror/lang-sass').then((module) => module.sass({ indented: false })) },
  { id: 'vue', name: 'Vue', extensions: ['vue'], load: () => import('@codemirror/lang-vue').then((module) => module.vue()) },
  {
    id: 'json',
    name: 'JSON',
    extensions: ['json', 'jsonc', 'json5', 'webmanifest', 'geojson', 'map'],
    filenames: ['.eslintrc', '.prettierrc'],
    load: () => import('@codemirror/lang-json').then((module) => module.json()),
  },
  { id: 'yaml', name: 'YAML', extensions: ['yml', 'yaml'], load: () => import('@codemirror/lang-yaml').then((module) => module.yaml()) },
  { id: 'toml', name: 'TOML', extensions: ['toml'], filenames: ['cargo.lock', 'pipfile'], load: () => legacy(import('@codemirror/legacy-modes/mode/toml').then((module) => module.toml)) },
  {
    id: 'xml',
    name: 'XML',
    extensions: ['xml', 'xsd', 'xsl', 'xslt', 'svg', 'plist', 'rss', 'atom', 'csproj'],
    load: () => import('@codemirror/lang-xml').then((module) => module.xml()),
  },
  {
    id: 'ini',
    name: 'INI',
    extensions: ['ini', 'cfg', 'conf', 'properties', 'editorconfig', 'gitconfig'],
    filenames: ['.editorconfig', '.gitconfig'],
    load: () => legacy(import('@codemirror/legacy-modes/mode/properties').then((module) => module.properties)),
  },
  { id: 'markdown', name: 'Markdown', extensions: ['md', 'markdown', 'mdx'], load: () => import('@codemirror/lang-markdown').then((module) => module.markdown()) },
  {
    id: 'dockerfile',
    name: 'Dockerfile',
    extensions: ['dockerfile'],
    filenames: ['dockerfile', 'containerfile'],
    load: () => legacy(import('@codemirror/legacy-modes/mode/dockerfile').then((module) => module.dockerFile)),
  },
  { id: 'cmake', name: 'CMake', extensions: ['cmake'], filenames: ['cmakelists.txt'], load: () => legacy(import('@codemirror/legacy-modes/mode/cmake').then((module) => module.cmake)) },
  { id: 'protobuf', name: 'Protobuf', extensions: ['proto'], load: () => legacy(import('@codemirror/legacy-modes/mode/protobuf').then((module) => module.protobuf)) },
  { id: 'diff', name: 'Diff', extensions: ['diff', 'patch'], load: () => legacy(import('@codemirror/legacy-modes/mode/diff').then((module) => module.diff)) },
];

/** The lowercase extension of a file name: the part after its last dot, none for a name that begins with the dot alone. */
export function extensionOf(name: string): string {
  const base = name.slice(name.lastIndexOf('/') + 1).toLowerCase();
  const dot = base.lastIndexOf('.');
  return dot <= 0 ? '' : base.slice(dot + 1);
}

/** The language of a file by its whole name, its extension or its first line; `null` for text that has none. */
export function languageOf(name: string, firstLine: string | null = null): Language | null {
  const base = name.slice(name.lastIndexOf('/') + 1).toLowerCase();
  const byName = LANGUAGES.find((language) => language.filenames?.includes(base) === true);
  if (byName !== undefined) return byName;
  const extension = extensionOf(name);
  if (extension !== '') {
    const byExtension = LANGUAGES.find((language) => language.extensions.includes(extension));
    if (byExtension !== undefined) return byExtension;
  }
  if (firstLine?.startsWith('#!') === true) return LANGUAGES.find((language) => language.firstLine?.test(firstLine) === true) ?? null;
  return null;
}

/** A language by the name it is kept under: what a person chose from the menu of the tab. */
export function languageById(id: string): Language | null {
  return LANGUAGES.find((language) => language.id === id) ?? null;
}
