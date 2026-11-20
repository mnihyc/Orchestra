# Web application

The browser application of Orch: the workbench where people work beside agents, with the screens for threads, documents, chats, resources, settings and management, in English and Simplified Chinese. It is a single-page application that talks to the API over HTTP and one realtime socket.

The stack is React 19, TypeScript 5 (strict) and Vite 8, with TanStack Router (routes from the files under `src/routes`) and TanStack Query, Zustand for what the live connection keeps, Tailwind CSS 4 on the tokens of `src/styles/tokens.css`, Radix primitives wrapped once in `src/ui`, lucide icons, i18next, and Inter and JetBrains Mono served from the build. The workbench is a layout tree of its own: its borders are drawn by react-resizable-panels and its tabs are moved with @dnd-kit. Documents are written in Tiptap (on ProseMirror), with KaTeX for formulas and Mermaid for diagrams, both loaded only on a page that has one. CodeMirror (the code view) and xterm (the terminal) load when first used. Vitest and Testing Library run the unit and component tests, and Playwright with axe runs the end-to-end and accessibility tests.

## Working on it

Use the Node version in `.nvmrc` and npm only. Run `npm ci`, then:

| Command | What it does |
|---------|--------------|
| `npm run dev` | The development server. It sends `/api` and `/health` to the API at `ORCH_DEV_API` (`http://127.0.0.1:18080` when unset) |
| `npm run typecheck` | Checks the translation keys ([build/check-translation-keys.ts](build/check-translation-keys.ts)), then runs the TypeScript compiler |
| `npm run lint` | ESLint, including the rule that a static utility class must produce CSS ([build/utility-classes.ts](build/utility-classes.ts)). With no arguments it checks the files in bounded, separate batches so that typed lint fits the memory of CI runners |
| `npm run test` | The unit and component tests |
| `npm run build` | The distributable application |
| `npm run test:e2e` | Playwright against the built application, served by `vite preview`, with the fake API of `tests/doubles` behind it |
| `node tests/doubles/server.ts` | The fake API by itself (`127.0.0.1:41871`), to use the application without a system behind it: run `ORCH_DEV_API=http://127.0.0.1:41871 npm run dev` and sign in as `ada` with `correct horse battery`. A second person, `dora`, in another browser sees what `ada` writes in the chats as it is written |

`/dev/ui` shows every shared control in both themes, both densities and both languages. It exists in a development build only. `src/generated/` and `src/routeTree.gen.ts` are written by generators and are never edited by hand.

## Rules the checks enforce

- **Translation keys.** Every key used in the source exists in both catalogues, and both catalogues have the same keys, placeholders and aliases ([translation-keys.test.ts](tests/unit/translation-keys.test.ts)).
- **Spacing and shape.** Layout gaps and page, list and form insets use 4, 8, 12, 16 and 24 px (the scale in `src/styles/tokens.css`). Radii follow hierarchy: 4 px for inline marks, 6 px for controls and rows, 8 px for contained surfaces and 12 px for dialogs. Row and button icons are 16 px and rail icons 20 px, with one 1.75 stroke. Text fields keep the shared input's 12 px inner margin.
- **Utility classes.** A static utility class that produces no CSS fails lint. Semantic marker classes stay allowed.
- **Tests.** There are no structural tests and no placeholder tests: what is tested is what a person can do and see, and what the client promises of the wire.

## Finder, search and help

- One field (`Mod+K`, or the magnifier in the rail) finds everything that has a name, as you type: places, people and resources from the server, settings, commands and help from what the page holds. A sign at the start keeps it to one kind of answer (`>` `@` `:` `?` `/`). Enter opens here and `Mod+Enter` in a new browser tab. A screen adds commands with `useRegisterCommands` while it is in view.
- `/search` keeps its whole state in its address and finds what was said in threads, chats and documents. The same search kept to one thread is the bar under a thread's header.
- A "?" beside a term opens a card. The help view (`F1`, the status bar, a hint's "More" or the finder) says what the screen in front is for, lists its hints and articles, searches the guide and lists the bound keys. `/help` and `/help/<article>` are the same help as pages. [src/help](src/help/README.md) reads the user guide from `docs/user/` into the build.

## Where things are

| Home | What it holds |
|------|----------------|
| `src/main.tsx`, `src/app.tsx`, `src/shell/` | Startup, provider composition, the workbench as the shell, banners and shortcuts |
| `src/routes/` | Authentication, hierarchy, resources, settings and management route entries |
| `src/api/` | HTTP transport, auth lifecycle and typed domain clients |
| `src/api/personActions.ts` | Explicit person notices and the actions available beside one readable thing. |
| `src/api/firstSteps.ts` | One current read of observed setup facts for the chosen org through the existing authenticated transport |
| `src/api/machinePrograms.ts` | Stored machine program snapshots and one acknowledged native registry mutation |
| `src/realtime/` | Socket recovery, cursors, deltas, repairs and terminals |
| `src/realtime/stores/` | Server-backed thread/topic/environment/tree/user/chat/document state, resource change hints and ephemeral watch state |
| [src/generated/](src/generated/README.md) | Generated API types and the settings catalogue |
| [src/ui/](src/ui/README.md) | Shared accessible primitives and domain-neutral controls |
| `src/lib/` | Formatting, Markdown, ANSI, time, graphemes and shortcuts |
| `src/lib/commandLine.ts` | Executable/argv input and readable round-trip quoting, with no shell interpretation |
| [src/help/](src/help/README.md) | The user guide read from `docs/user/` into the build, loaded on first use; the tutorials |
| `src/i18n/` | English and Simplified Chinese catalogues, held to the same keys by a test |
| `src/styles/` | The design tokens in both themes and both densities, and the base styles |
| `public/` | The application's mark and manifest |
| `tests/` | Unit, component, end-to-end and accessibility tests with the fake API they run against |

## Features

Each feature README lists its screens, panels and controls. Work on a screen pairs what is visible with what the server has committed, including pending, denied, blocked, stale, offline and recovery states. The conversation is chat-first with expandable tool activity; what stands beside it, and whether it or the code is in the middle, is each person's own arrangement.

| Feature home | What it covers |
|------|------------------|
| [workbench](src/features/workbench/README.md) | The workbench: areas, views and arrangements |
| [auth](src/features/auth/README.md) | Authentication, setup and invitations |
| [onboarding](src/features/onboarding/README.md) | First-run guidance |
| [navigation](src/features/navigation/README.md) | Organization and work navigation |
| [finder](src/features/finder/README.md) | The finder and the commands screens offer |
| [help](src/features/help/README.md) | Hints and help |
| [search](src/features/search/README.md) | Authorized global search |
| [org](src/features/org/README.md) | Organization home, the same at every level |
| [project](src/features/project/README.md) | Project topics and environments |
| [everything](src/features/everything/README.md) | The reader's paged hierarchy, search and filters in one main-area page |
| [fleet](src/features/fleet/README.md) | Fleet: environments, provider runtimes and recycling |
| [topic](src/features/topic/README.md) | Topic: threads, environments and collaboration |
| [announcements](src/features/announcements/README.md) | Configurable human broadcasts |
| [thread](src/features/thread/README.md) | Shared conversation screen |
| [timeline](src/features/timeline/README.md) | Conversation rendering and navigation |
| [composer](src/features/composer/README.md) | Inputs and delivery queue |
| [inspector](src/features/inspector/README.md) | The views of a thread: workspace, runtime and people |
| [files](src/features/files/README.md) | Files and code |
| [access](src/features/access/README.md) | Access: the permission page |
| [resources](src/features/resources/README.md) | Providers, profiles, environments, hosts, connected machines and templates |
| [settings](src/features/settings/README.md) | Personal and hierarchical configuration |
| [admin](src/features/admin/README.md) | System management |
| [start](src/features/start/README.md) | Type-and-go and explicit binding |
| [change](src/features/change/README.md) | Changing where and on what work runs: fork, switch, environment update, move and recycle, several at once |
| [side-threads](src/features/side-threads/README.md) | Side threads: asked on the side, shown beside the conversation, kept or closed |
| [inbox](src/features/inbox/README.md) | Notification inbox |
| [approvals](src/features/approvals/README.md) | All approval kinds |
| [usage](src/features/usage/README.md) | Usage at three depths, caps, rates and passing a limit |
| [chat](src/features/chat/README.md) | Chats among people |
| [forward](src/features/forward/README.md) | Selecting and forwarding |
| [documents](src/features/documents/README.md) | Documents written in blocks |
| [source](src/features/source/README.md) | A turn's files restored with an undo, Commit and Push, a thread's links, observed work, comments on lines |
| [browser](src/features/browser/README.md) | An environment's browsers, watched and driven a press apart |
| [vnc](src/features/vnc/README.md) | An environment's displays, drawn by noVNC, watched and worked in |

Management (`/admin/<section>`) is for a system administrator in a browser session. Its entry is in the account menu of whoever is one and nowhere else, and the address of a section read by anybody else shows the refusal.
