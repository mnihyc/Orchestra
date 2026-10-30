# Help content

The user guide as the build holds it, and what the help reads it through.

The article list follows first use: Getting started, ordinary workflows and finding/sharing work,
then resource setup, configuration and operation. New extension articles follow in stable id order.
Localized titles preserve that order. An article row is its title, without a repeated section count.

- `guide.ts` reads one article of `docs/user/` (its title, headings with anchors, keywords and the screens it belongs
  to, from a block of front matter) and knows nothing of where the articles come from.
  `heading_keywords.<anchor>: alias, another alias` attaches search words to that section, so a creation
  question opens its actual permission explanation. Localized result titles keep the authored anchor.
  `heading_id.<translated-slug>: <canonical-anchor>` keeps an authored translated heading reachable by the
  same address as its English section; hint links and article links do not need a second destination.
- `library.ts` reads every article of `docs/user/` when the application is built; `index.ts` asks for it the first
  time a hint, the help view or the finder wants an article, so it is a part of the build of its own that nobody pays
  for until the help is opened. `index.ts` is never edited by hand: it holds no article.
- `tutorials/` holds one module for each tutorial that ships: steps name real controls and readable destinations.
  The four decision walks also declare illustrated examples and destinations for an owned-object picker;
  an empty newcomer can finish by reading the examples without changing any work.
- The texts of hints are in the two catalogues of `../i18n/`, by id (`help.hints.<id>`).

A test holds every hint to a text in both languages, and every article a hint leads to to be there at the heading it
names, and every link between articles to resolve. A tutorial step whose target no screen has fails the build.
The four decision walks also fail without a destination, illustration or its text in both catalogues. Every
generated setting key requires its English and Chinese title and description in the same content gate.

- `localized.ts` gives shipped article names from both catalogues at every Help and finder entrance;
  article bodies and heading anchors retain their authored text, with the existing untranslated-body notice.
- `templates.ts`: The template guide has one identity for its controls, search vocabulary, translations and content checks. */
- `files.ts`: File help destinations keep each authored language's heading and are checked with the shipped guide.
- `entryCheck.ts`: Screen audits compare visible guide destinations and their accessible names in the reader's language. */

Write an environment template is included by the ordinary user-guide glob, with a translated title and
English/Chinese search terms. Its five walkthrough sections and both template-screen entrances are
required by the build content gate. Missing the article, a section, a translation or a control is a failure.
