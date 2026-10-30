//! Documents: the twenty block types and their inline content, the operations that change them, comments, versions cut by the clock with what each changed and who wrote it, who last changed each block, files, limits, Markdown in, and Markdown, HTML and Typst out with what an export is asked to hold.
//!
//! A document is a page of blocks written beside a node. A client reads it
//! as a [`DocumentSync`]: the [`Document`] and every live [`DocumentBlock`],
//! and builds the tree from `parent_id` and `position`. It changes it with
//! batches of [`DocumentOp`]s, each applied whole or not at all, and keeps
//! up through the document's realtime feed. A block has a [`BlockType`], the
//! [`BlockProps`] that type takes, and content that is [`Inline`] text or,
//! for a table, rows. Discussions sit beside the text and are no part of it.
//! The server cuts [`DocumentVersion`]s and renders exports.

use orch_types::{Hash, Id, Seq, Time};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::access::PagePreview;
use crate::chats::Reaction;
use crate::common::{Listed, Page};
use crate::nodes::{NodeRef, PageState};
use crate::routes::Access::{Permission, Rule, User};
use crate::users::UserRef;
use crate::workspace::ArchiveListing;

/// The most characters of the title of a document; the least is one.
pub const DOCUMENT_TITLE_MAX_CHARS: u32 = 200;
/// The most characters of the description of a document, as of a place's or a thread's.
pub const DOCUMENT_DESCRIPTION_MAX_CHARS: u32 = 4_000;
/// The most characters of the line a list shows for a document beside its title: a line, not a paragraph, as long
/// as the opening of a place's description in an agent's listing.
pub const DOCUMENT_LINE_MAX_CHARS: u32 = 200;
/// The most operations in one batch.
pub const DOCUMENT_OPS_MAX: u32 = 500;
/// The most bytes of the props and the content of one block together.
pub const DOCUMENT_BLOCK_MAX_BYTES: i64 = 64 * 1024;
/// The deepest blocks may be nested.
pub const DOCUMENT_NESTING_MAX: u32 = 16;
/// The fewest `column` blocks a `columns` block holds.
pub const COLUMNS_MIN: u32 = 2;
/// The most `column` blocks a `columns` block holds.
pub const COLUMNS_MAX: u32 = 6;
/// The smallest level of a heading.
pub const HEADING_LEVEL_MIN: u32 = 1;
/// The largest level of a heading.
pub const HEADING_LEVEL_MAX: u32 = 6;
/// The most blocks a sync from a position holds; with more changed, or from below what was purged,
/// the client loads without a position.
pub const DOCUMENT_SYNC_MAX_CHANGES: i64 = 2_000;
/// The most characters of the words a discussion is made on.
pub const DISCUSSION_QUOTE_MAX_CHARS: u32 = 400;

/// The block types, twenty in all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BlockType {
    /// A paragraph of text.
    Paragraph,
    /// A heading, of a level from [`HEADING_LEVEL_MIN`] to [`HEADING_LEVEL_MAX`].
    Heading,
    /// An item of a bulleted list.
    Bullet,
    /// An item of a numbered list.
    Numbered,
    /// An item of a list of things to do, ticked or not.
    Todo,
    /// A line that folds what is under it.
    Toggle,
    /// A quotation.
    Quote,
    /// A text set apart by a tone.
    Callout,
    /// Code, in a language.
    Code,
    /// A formula, as LaTeX.
    Math,
    /// A diagram, as Mermaid.
    Diagram,
    /// A dividing line.
    Divider,
    /// A page break.
    PageBreak,
    /// An image the document holds.
    Image,
    /// A film, a file the document holds or an address.
    Video,
    /// A sound, a file the document holds or an address.
    Audio,
    /// A file the document holds.
    File,
    /// A table; its content is rows.
    Table,
    /// Columns side by side; it holds `column` blocks alone.
    Columns,
    /// One of the columns of a `columns` block.
    Column,
}

/// A way the text of a run is marked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Mark {
    /// Bold.
    Bold,
    /// Italic.
    Italic,
    /// Underlined.
    Underline,
    /// Struck through.
    Strike,
    /// Code.
    Code,
}

/// A colour of the palette, from 0 up to one less than [`crate::users::COLOR_COUNT`], stored by
/// its index so that a page reads right in a light and in a dark theme.
pub type Tint = u32;

/// What a chip in a line of text names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum InlineRefKind {
    /// A node.
    Node,
    /// A document.
    Document,
    /// A chat: of a place or a thread, or a direct or group chat.
    Chat,
    /// A file attached to a document or a chat.
    File,
    /// A file at a workspace-relative path in an environment.
    EnvironmentFile,
}

/// The part of a thing that a reference names. A block of a document is held by its identifier, so that
/// writing above it moves nothing; a turn of a thread's conversation and a message of a chat by their
/// numbers, which never change. Agents read and write a part as the number `read_shared` counts it by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RefPart {
    /// A block of a document.
    Block(Id),
    /// A turn of a thread's conversation, by its number.
    Turn(i64),
    /// A message of a chat, by its number.
    Message(i64),
    /// A line of a text file, counted from one.
    Line(i64),
}

/// What a block's text is made of.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Inline {
    /// A run of text.
    Text {
        /// The text.
        text: String,
        /// How it is marked.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        marks: Option<Vec<Mark>>,
        /// Where it links to, when the text is a link: `http`, `https` or `mailto`, or a path of
        /// this system.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        href: Option<String>,
        /// Its colour.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        color: Option<Tint>,
        /// The colour behind it, as with a highlighter.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        background: Option<Tint>,
    },
    /// A person named with `@`.
    Mention {
        /// The person.
        user_id: Id,
        /// Who they are; filled in when the document is read.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        user: Option<UserRef>,
    },
    /// A chip that names a thing in Orch, and may name one of its parts.
    Ref {
        /// The document's saved chip presentation; absent follows the place's default.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        display: Option<ReferenceSize>,
        /// What it names.
        kind: InlineRefKind,
        /// The node, the document or the chat.
        id: Id,
        /// For an environment file, its path from the workspace root.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<String>,
        /// The block, the turn or the message it names; absent for the thing whole.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        part: Option<RefPart>,
        /// The name shared when this reference was made; absent on older references.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        /// When read: its name, or `null` where the reader may not see it. Left out when written.
        #[serde(
            default,
            deserialize_with = "crate::common::absent_or_null",
            skip_serializing_if = "Option::is_none"
        )]
        #[schemars(with = "Option<String>")]
        name: Option<Option<String>>,
    },
    /// A formula in the line, as LaTeX.
    Math {
        /// The formula.
        source: String,
    },
    /// A line break inside a block.
    Break,
}

/// The presentation of a reference, shared by its chip and its card in document flow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReferenceSize {
    /// Glyph and name.
    Small,
    /// Glyph, name and a muted hint or size.
    Medium,
    /// A preview card in the flow, with the target's permitted metadata.
    Large,
}

/// One cell of a table.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TableCell {
    /// What it holds.
    pub content: Vec<Inline>,
    /// How many columns it spans; the cells it covers are left out of their rows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub colspan: Option<u32>,
    /// How many rows it spans; the cells it covers are left out of their rows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rowspan: Option<u32>,
    /// Its text colour.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<Tint>,
    /// Its background.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<Tint>,
}

/// One row of a table.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TableRow {
    /// Its cells.
    pub cells: Vec<TableCell>,
}

/// How a block of text is aligned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TextAlign {
    /// To the left.
    Left,
    /// Centred.
    Center,
    /// To the right.
    Right,
    /// Justified.
    Justify,
}

/// How an image is aligned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ImageAlign {
    /// To the left.
    Left,
    /// Centred.
    Center,
    /// To the right.
    Right,
}

/// The tone of a callout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CalloutTone {
    /// Something to know.
    Info,
    /// Something that went well.
    Success,
    /// Something to take care of.
    Warning,
    /// Something dangerous.
    Danger,
}

/// The props of a paragraph, a bullet, a toggle and a quote; also the part of other text blocks'
/// props that colours and aligns the whole block.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TextProps {
    /// The colour of the text of the whole block.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<Tint>,
    /// The colour behind the whole block.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<Tint>,
    /// How the text is aligned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub align: Option<TextAlign>,
}

/// The props of a heading.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HeadingProps {
    /// The colour of the text of the whole block.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<Tint>,
    /// The colour behind the whole block.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<Tint>,
    /// How the text is aligned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub align: Option<TextAlign>,
    /// Its level, from [`HEADING_LEVEL_MIN`] to [`HEADING_LEVEL_MAX`].
    #[schemars(range(min = 1, max = 6))]
    pub level: u32,
    /// Whether it folds what is under it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub toggle: Option<bool>,
}

/// The props of an item of a numbered list.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NumberedProps {
    /// The colour of the text of the whole block.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<Tint>,
    /// The colour behind the whole block.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<Tint>,
    /// How the text is aligned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub align: Option<TextAlign>,
    /// Where its list begins counting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<u32>,
}

/// The props of an item of a list of things to do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TodoProps {
    /// The colour of the text of the whole block.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<Tint>,
    /// The colour behind the whole block.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<Tint>,
    /// How the text is aligned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub align: Option<TextAlign>,
    /// Whether it is ticked.
    pub checked: bool,
}

/// The props of a callout.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CalloutProps {
    /// The colour of the text of the whole block.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<Tint>,
    /// The colour behind the whole block.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<Tint>,
    /// How the text is aligned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub align: Option<TextAlign>,
    /// Its tone.
    pub tone: CalloutTone,
}

/// The props of a code block, whose content is one unmarked text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CodeProps {
    /// The language it is written in.
    pub language: String,
    /// Whether long lines wrap.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wrap: Option<bool>,
}

/// The props of a formula, whose source is LaTeX, and of a diagram, whose source is Mermaid. Both
/// have no content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SourceProps {
    /// The source.
    pub source: String,
}

/// The props of an image, whose content is its caption.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImageProps {
    /// The document's saved presentation of this file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display: Option<ReferenceSize>,
    /// The file of the document it shows.
    pub file_id: Id,
    /// The width it is shown at.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<f64>,
    /// How it is aligned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub align: Option<ImageAlign>,
}

/// The props of a film and of a sound, whose content is the caption: a file the document holds, or
/// an `https` address. Orch fetches nothing from an address, and neither does a reader's browser
/// until that reader asks.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MediaProps {
    /// The document's saved presentation of this file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display: Option<ReferenceSize>,
    /// A file the document holds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_id: Option<Id>,
    /// An address.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// The width it is shown at.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<f64>,
}

/// The props of a file block, whose content is the caption.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FileProps {
    /// The document's saved presentation of this file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display: Option<ReferenceSize>,
    /// The file of the document it names.
    pub file_id: Id,
}

/// The props of a table, whose content is its rows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TableProps {
    /// Whether its first row is a header.
    pub header_row: bool,
    /// Whether its first column is a header.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header_column: Option<bool>,
    /// Its columns' shares of the width.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub widths: Option<Vec<f64>>,
}

/// The props of a column of a `columns` block.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ColumnProps {
    /// Its share of the width, as a weight; equal shares when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub share: Option<f64>,
}

/// The props of a block that takes none: a divider, a page break and a `columns` block.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NoProps {}

/// What a block's type takes, told apart by the members it holds. They are those of the block's
/// `type`, which an operation that changes only the props does not repeat, so a reader takes a
/// block's props as those of the type the block has: a member that type does not take, a tint
/// outside the palette, a heading level outside 1 to 6, a span that reaches outside its table or
/// overlaps another, and a `columns` block with fewer than two or more than six columns are each
/// refused when a batch is applied, with `request.invalid`. A `{}` reads as the props of a text
/// block; it is also what a divider, a page break and a `columns` block have.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum BlockProps {
    /// A heading.
    Heading(HeadingProps),
    /// An item of a list of things to do.
    Todo(TodoProps),
    /// A callout.
    Callout(CalloutProps),
    /// Code.
    Code(CodeProps),
    /// A table.
    Table(TableProps),
    /// A formula or a diagram.
    Source(SourceProps),
    /// An image.
    Image(ImageProps),
    /// A file.
    File(FileProps),
    /// A paragraph, a bullet, a toggle or a quote.
    Text(TextProps),
    /// An item of a numbered list.
    Numbered(NumberedProps),
    /// A column.
    Column(ColumnProps),
    /// A film or a sound.
    Media(MediaProps),
    /// A divider, a page break or a `columns` block.
    Empty(NoProps),
}

/// What a block holds: inline content, or the rows of a table. A block without content has `[]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum BlockContent {
    /// Text and what goes in a line of it.
    Inline(Vec<Inline>),
    /// The rows of a table.
    Rows(Vec<TableRow>),
}

/// An agent that was let write, named by its thread.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct AuthorAgent {
    /// The agent's name.
    pub agent: String,
    /// The thread it worked in.
    pub thread: NodeRef,
}

/// Who wrote something: a person, or an agent that was let write.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Author {
    /// A person.
    User(UserRef),
    /// An agent.
    Agent(AuthorAgent),
}

/// One block as it stands.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DocumentBlock {
    /// The block's identifier, chosen by the editor that made it.
    pub id: Id,
    /// The document.
    pub document_id: Id,
    /// The block it is under; `null` at the top level.
    pub parent_id: Option<Id>,
    /// Its place among its siblings: a decimal number as text, ascending; equal positions are
    /// ordered by id.
    pub position: String,
    /// What it is.
    #[serde(rename = "type")]
    pub block_type: BlockType,
    /// What its type takes.
    pub props: BlockProps,
    /// Inline content, or rows for a table.
    pub content: BlockContent,
    /// Whether it is gone; sent so that a reader takes it away.
    pub removed: bool,
    /// The document sequence at which it was made.
    pub seq_created: Seq,
    /// The document sequence at which it last changed.
    pub seq_updated: Seq,
    /// Who changed it last.
    pub updated_by: Option<Author>,
    /// When it last changed.
    pub updated_at: Time,
}

/// A block to insert, with any children.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NewBlock {
    /// Chosen by the client: a UUID nobody used.
    pub id: Id,
    /// What it is.
    #[serde(rename = "type")]
    pub block_type: BlockType,
    /// What its type takes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub props: Option<BlockProps>,
    /// Inline content, or rows for a table.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<BlockContent>,
    /// The blocks under it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub children: Option<Vec<NewBlock>>,
}

/// One change. A batch of at most [`DOCUMENT_OPS_MAX`] is applied whole or not at all.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum DocumentOp {
    /// Puts a block, with its children, at a place.
    Insert {
        /// The block.
        block: NewBlock,
        /// The block it goes under; `null` for the top level. It is a live block of the document
        /// that may have children.
        parent_id: Option<Id>,
        /// The sibling it goes after; `null` for first among its siblings. One that is gone, or is
        /// no child of that parent any more, puts it at the end.
        after_id: Option<Id>,
    },
    /// Changes the type, the props or the content of a block.
    Update {
        /// The block.
        id: Id,
        /// The block's `seq_updated` that the change was written on; the batch is refused when it
        /// was changed since.
        base_seq: Seq,
        /// Its new type.
        #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
        block_type: Option<BlockType>,
        /// Its new props.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        props: Option<BlockProps>,
        /// Its new content.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content: Option<BlockContent>,
    },
    /// Moves a block, with what is under it.
    Move {
        /// The block.
        id: Id,
        /// The block it goes under; `null` for the top level. It may have children and is not under
        /// the block itself.
        parent_id: Option<Id>,
        /// The sibling it goes after; `null` for first among its siblings.
        after_id: Option<Id>,
    },
    /// Removes a block, with everything under it. Removing what is gone changes nothing.
    Remove {
        /// The block.
        id: Id,
    },
}

/// One comment of a discussion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DocumentComment {
    /// The comment's identifier.
    pub id: Id,
    /// The discussion it is in.
    pub discussion_id: Id,
    /// Who wrote it.
    pub author: UserRef,
    /// What it says; empty for a deleted one.
    pub content: Vec<Inline>,
    /// How people reacted.
    pub reactions: Vec<Reaction>,
    /// When it was last edited.
    pub edited_at: Option<Time>,
    /// Whether it was deleted; it stays as an emptied line.
    pub deleted: bool,
    /// When it was written.
    pub created_at: Time,
    /// The document sequence at which it last changed.
    pub seq_updated: Seq,
}

/// One indexed window of a discussion's comments, in creation order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DocumentCommentPage {
    /// At most the requested number of comments.
    pub items: Vec<DocumentComment>,
    /// The boundary for older comments; absent at the beginning of the discussion.
    pub next_cursor: Option<String>,
    /// Whether the caller wrote anywhere in this discussion, including other pages.
    pub wrote: bool,
}

/// Who resolved a discussion, and when.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DiscussionResolved {
    /// When.
    pub at: Time,
    /// Who.
    pub by: UserRef,
}

/// A discussion beside a document, about one place in it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DocumentDiscussion {
    /// The discussion's identifier.
    pub id: Id,
    /// The document.
    pub document_id: Id,
    /// The block it is about; `null` once that block is gone, and then it is about the document as
    /// a whole.
    pub block_id: Option<Id>,
    /// The words it was made on, as they read then, at most [`DISCUSSION_QUOTE_MAX_CHARS`]
    /// characters; empty for the whole block.
    pub quote: String,
    /// Whether those words are still in that block, and are marked there.
    pub anchored: bool,
    /// When and by whom it was resolved.
    pub resolved: Option<DiscussionResolved>,
    /// Who started it.
    pub created_by: UserRef,
    /// When it was started.
    pub created_at: Time,
    /// The latest fifty comments in creation order; older comments use DocumentCommentList.
    pub comments: Vec<DocumentComment>,
    /// The document sequence at which it last changed.
    pub seq_updated: Seq,
}

/// Which discussions beside a document a person wants to hear about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DocumentDiscussionLevel {
    /// Every new discussion and its answers.
    All,
    /// Those they take part in, answer or are named in.
    #[default]
    Involved,
}

/// The fixed kind of a shared text document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DocumentMark {
    /// Editable text with its own permission page.
    Document,
}

/// A document, without its blocks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DocumentRef {
    /// Native reach alone gives View; the same marker as the document's page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub admin_access: Option<crate::access::AdminAccess>,
    /// The document's identifier.
    pub id: Id,
    /// Its title, one to [`DOCUMENT_TITLE_MAX_CHARS`] characters.
    pub title: String,
    /// Its description, whose first line is shown where documents are listed; at most
    /// [`DOCUMENT_DESCRIPTION_MAX_CHARS`] characters, empty when nobody wrote one.
    pub description: String,
    /// Who maintains the description: whoever wrote it, a person or an agent, or automation.
    pub description_source: crate::nodes::FieldSource,
    /// What a list shows beside its title: the first line of its description, or while it has none the first
    /// line of its first paragraph; at most [`DOCUMENT_LINE_MAX_CHARS`] characters, empty when it has neither.
    pub description_line: String,
    /// The org, project, topic or thread it is written beside.
    pub node: NodeRef,
    /// Its first owner.
    pub owner: UserRef,
    /// How many people own it.
    pub owners: i64,
    /// Who made it: a person, or an agent that may edit the documents of its node.
    pub created_by: Author,
    /// Whether every row of its page follows its node's row of the same name; somebody changed
    /// one; or its View no longer follows, which the screen calls private.
    pub page: PageState,
    /// Its sequence, which moves with every change.
    pub seq: Seq,
    /// How many live blocks it has.
    pub block_count: i64,
    /// The size of its text.
    pub bytes: i64,
    /// The size of the files it holds, together.
    pub files_bytes: i64,
    /// How many versions were cut so far.
    pub version: i64,
    /// How many discussions beside it nobody resolved.
    pub open_discussions: i64,
    /// When it last changed.
    pub updated_at: Time,
    /// Who changed it last.
    pub updated_by: Option<Author>,
    /// The caller's discussion bell; involved when they have not chosen.
    #[serde(default)]
    pub discussion_level: DocumentDiscussionLevel,
    /// The caller's own: the sequence they last had on screen; `null` for never.
    pub seen_seq: Option<Seq>,
    /// Whether its node is archived: the document is read-only.
    pub archived: bool,
    /// Whether it was deleted and can be restored until it is purged.
    pub deleted: bool,
}

/// What a document may hold, as resolved at its node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DocumentLimits {
    /// The most bytes of all its blocks together.
    pub bytes: i64,
    /// The most blocks.
    pub blocks: i64,
    /// The most bytes of one file.
    pub file_bytes: i64,
    /// The most bytes of all its files together.
    pub files_bytes: i64,
}

/// What the caller holds on a document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DocumentAccess {
    /// The actual source of native-only permissions, even when ordinary View leaves no marker.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub admin_place: Option<crate::nodes::NodeOrSystem>,
    /// Native reach alone gives View; absent when the person's ordinary pages already do.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub admin_access: Option<crate::access::AdminAccess>,
    /// Held permissions absent from the ordinary frame, in catalogue order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub admin_only: Vec<String>,
    /// A personal/group assignment here, independent of View and As admin.
    #[serde(default, skip_serializing_if = "crate::common::is_false")]
    pub participating: bool,
    /// The permissions they hold on it, `doc.*` and `access.*`.
    pub permissions: Vec<String>,
    /// Whether they administer the place it is in.
    pub administers: bool,
    /// Whether they are one of its owners.
    pub owns: bool,
}

/// A file a document holds beside its text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DocumentFile {
    /// The file's identifier, which an `image` or a `file` block names.
    pub id: Id,
    /// Its name, unique in its document.
    pub name: String,
    /// Its type.
    pub media_type: String,
    /// Its size in bytes.
    pub size: i64,
    /// Whether its bytes are text: it can be read and searched where it lies.
    pub is_text: bool,
    /// Who put it there.
    pub added_by: Option<Author>,
    /// When.
    pub added_at: Time,
}

/// A document, as the caller reads it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Document {
    /// Deletion facts on an explicitly requested read-only inspection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inspection: Option<crate::nodes::DeletedInspection>,
    /// The document without its blocks.
    #[serde(flatten)]
    pub base: DocumentRef,
    /// The way down to its node, from the root org.
    pub path: Vec<NodeRef>,
    /// Current typed placement and independently redacted document-folder ancestry.
    pub location: crate::document_tree::DocumentLocation,
    /// What it holds beside its text: images, films, data, scripts, of any kind.
    pub files: Vec<DocumentFile>,
    /// What it may hold, as resolved at its node.
    pub limits: DocumentLimits,
    /// When it was made.
    pub created_at: Time,
    /// What the caller holds on it.
    pub access: DocumentAccess,
}

/// Everything a client needs to show a document, or what changed since it last did.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DocumentSync {
    /// The document, always.
    pub document: Document,
    /// Every live block, or those with `seq_updated` above the position asked from, removed ones
    /// among them.
    pub blocks: Vec<DocumentBlock>,
    /// Up to fifty recent discussions, or changed discussions since, each with its latest fifty comments.
    /// The complete discussion list and comment histories are available through their paged endpoints.
    pub discussions: Vec<DocumentDiscussion>,
    /// The document's sequence.
    pub seq: Seq,
    /// The sequence the caller asked from, if it did.
    pub since: Option<Seq>,
    /// False when more than [`DOCUMENT_SYNC_MAX_CHANGES`] blocks changed, or the position is below
    /// what was purged; the client loads again without a position.
    pub complete: bool,
}

/// Why a version was cut.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum VersionCause {
    /// It was being written and the window had passed.
    Window,
    /// It had been left alone since its last change.
    Idle,
    /// The whole document was about to be replaced.
    Rewrite,
    /// A version was put back.
    Restore,
}

/// What happened to a named file during a span of document changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DocumentFileChangeKind {
    /// A file was put into the document.
    Added,
    /// New bytes replaced a file of the same name.
    Replaced,
    /// A file was taken out of the document.
    Removed,
}

/// A file named in the document's history, under that span's people and threads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DocumentFileChange {
    /// The stored name, never the machine's local path.
    pub name: String,
    /// What changed.
    pub kind: DocumentFileChangeKind,
}

/// How many blocks a span of changes touched, and which files it changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct VersionChanged {
    /// Inserted.
    pub inserted: i64,
    /// Updated.
    pub updated: i64,
    /// Moved.
    pub moved: i64,
    /// Removed.
    pub removed: i64,
    /// Named file changes; older entries and block-only changes have none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<DocumentFileChange>,
}

/// One entry of a document's history: a span of changes, and the state it led to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DocumentVersion {
    /// Its number; `null` for the open span, what was changed since the last version was cut.
    pub version: Option<i64>,
    /// The title then.
    pub title: String,
    /// The size of its text then.
    pub bytes: i64,
    /// How many blocks it had then.
    pub block_count: i64,
    /// Why it was cut; `null` for the open span.
    pub cause: Option<VersionCause>,
    /// Who changed it between the version before and this one: people, and agents that were let
    /// edit, alike.
    pub authors: Vec<Author>,
    /// What was changed in that span.
    pub changed: VersionChanged,
    /// The first change of the span.
    pub from: Time,
    /// The last change of the span.
    pub to: Time,
}

/// How a title differs between two states of a document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DocumentTitleChange {
    /// The title in the earlier state.
    pub before: String,
    /// The title in the later state.
    pub after: String,
}

/// How a block differs between two states of a document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DocumentBlockChange {
    /// The block.
    pub id: Id,
    /// How it was; `null` when it was inserted.
    pub before: Option<DocumentBlock>,
    /// How it is; `null` when it was removed.
    pub after: Option<DocumentBlock>,
    /// Who changed it last between the two, a person or an agent.
    pub by: Option<Author>,
}

/// What differs between two states of a document, block by block. A block that changed in type,
/// props, content or place is listed; unchanged blocks are left out.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DocumentChanges {
    /// The earlier version; 0 is the document when it was empty.
    pub from: i64,
    /// The later version; `null` for the document as it stands.
    pub to: Option<i64>,
    /// The title, when it differs.
    pub title: Option<DocumentTitleChange>,
    /// The blocks.
    pub blocks: Vec<DocumentBlockChange>,
    /// Who changed it between the two.
    pub authors: Vec<Author>,
}

/// What the server writes a document as. A PDF is made from `typst` in the reader's browser, so
/// that no service of Orch compiles a document.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DocumentExport {
    /// As much of the document as Markdown can say; the export that always works.
    #[default]
    Markdown,
    /// One file that carries its images within it.
    Html,
    /// Typst source with the images it names.
    Typst,
    /// The blocks, as a sync gives them, and with comments the discussions.
    Json,
}

/// Whether an export holds a part that is not always in it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DocumentExportChoice {
    /// Left out.
    #[default]
    Omit,
    /// Put in.
    Include,
}

/// What an export holds beside its text and its images, which it always has.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExportOptions {
    /// The files that are no images: left out and named where they stood (as shipped), or put into
    /// the archive under their names.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachments: Option<DocumentExportChoice>,
    /// The discussions beside it: left out (as shipped), or written after the text, each under the
    /// words it is about.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comments: Option<DocumentExportChoice>,
}

/// How a new document's page starts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NewDocumentAccess {
    /// Its rows follow its node's.
    #[default]
    Follows,
    /// Its rows are cut loose: its owners alone.
    Private,
}

/// Whether a list holds the documents that were deleted.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum DeletedFilter {
    /// Only those deleted and not purged yet, for whoever could have deleted them.
    Only,
}

/// Which discussions a list holds.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DiscussionResolvedFilter {
    /// Those nobody resolved.
    #[default]
    Exclude,
    /// All of them.
    Include,
    /// Only the resolved ones.
    Only,
}

/// The documents written beside a node that the caller may read, the most recently changed first.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NodeDocumentListQuery {
    /// Only ordinary page access, independent of native administrator reach; before paging.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub participating: Option<bool>,
    /// Also those of the nodes below it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub under: Option<bool>,
    /// `only`: those deleted and not purged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted: Option<DeletedFilter>,
    /// How many entries, at most [`crate::common::MAX_LIMIT`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// The `next_cursor` of the page before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// First document pages for expanded navigation places, each under its native access check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NodeDocumentPagesGetQuery {
    /// Comma-separated node identifiers; the first is the node in the request path.
    pub node_ids: String,
    /// The page size for each place, with the native document-list bounds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// One admitted first page or that place's native refusal, without hiding other readable places.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct NodeDocumentPage {
    /// The requested place.
    pub node_id: Id,
    /// The native first page, absent only when this place's read failed.
    pub page: Option<crate::common::Page<crate::common::Listed<DocumentRef>>>,
    /// The native error, absent when the page was read.
    pub error: Option<crate::common::Problem>,
}

/// Makes a document, the caller's own: they are its owner, whatever they hold on the documents
/// of others there. At most one of `blocks` and `markdown`; `markdown` is read into blocks by the
/// server, the way in for a script and for an import.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NodeDocumentCreateBody {
    /// Its title, one to [`DOCUMENT_TITLE_MAX_CHARS`] characters.
    pub title: String,
    /// Its blocks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocks: Option<Vec<NewBlock>>,
    /// Its text, as Markdown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub markdown: Option<String>,
    /// Whether its page follows its node's or is private; `follows` unless given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access: Option<NewDocumentAccess>,
}

/// Where a sync reads from.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DocumentSyncGetQuery {
    /// Inspect retained deleted content under this object's current restoration authority.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inspect_deleted: Option<bool>,
    /// Only what changed after this document sequence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<Seq>,
}

/// Renames a document, changes its description, or both; at least one of the two is given. A description that
/// is written is the writer's from then on: automation leaves it alone.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DocumentUpdateBody {
    /// The new title, one to [`DOCUMENT_TITLE_MAX_CHARS`] characters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The new description, at most [`DOCUMENT_DESCRIPTION_MAX_CHARS`] characters; empty takes it away.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// A batch of changes. Every operation is applied, the blocks it touched and the document are
/// stamped with one new sequence, and the feed delivers them; or nothing is.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DocumentOpsCreateBody {
    /// The live snapshot used to prepare a selective inverse. A mismatch refuses the whole batch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_seq: Option<Seq>,
    /// A client-generated UUID: a repeated pair of author and value returns what the first did.
    pub client_id: String,
    /// The operations, at most [`DOCUMENT_OPS_MAX`].
    pub ops: Vec<DocumentOp>,
}

/// What a batch did.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DocumentOpsCreateReply {
    /// The document as it stands.
    pub document: DocumentRef,
    /// The blocks the batch touched, as they stand.
    pub blocks: Vec<DocumentBlock>,
    /// The document's sequence after the batch.
    pub seq: Seq,
}

/// Which entries of a document's history are wanted.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DocumentVersionListQuery {
    /// Inspect retained deleted content under this object's current restoration authority.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inspect_deleted: Option<bool>,
    /// Only the entries one author wrote in: a user id, or `thread:` and an identifier for an
    /// agent's thread.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// How many entries, at most [`crate::common::MAX_LIMIT`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// The `next_cursor` of the page before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// The two states to compare. For an entry of the history, `from` is the version before it and `to`
/// its own: what that span changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DocumentChangesGetQuery {
    /// Inspect retained deleted content under this object's current restoration authority.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inspect_deleted: Option<bool>,
    /// A version number, `0` for the empty document, or `seen`: the last version cut before the
    /// caller last had the document open. A query string holds it as text.
    pub from: String,
    /// A version; the document as it stands unless given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<i64>,
}

/// A document as it stood at a version.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DocumentVersionGetReply {
    /// The entry of the history.
    pub version: DocumentVersion,
    /// The blocks then.
    pub blocks: Vec<DocumentBlock>,
}

/// Whether a file of the same name is replaced.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DocumentFileCreateQuery {
    /// Replace a file the document has of that name; otherwise it is `409 conflict.exists`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overwrite: Option<bool>,
}

/// A file that is put into a document from a finished upload session instead of a raw body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DocumentFileBlobBody {
    /// The upload's hash.
    pub blob: Hash,
    /// Its name.
    pub name: String,
}

/// How a file of a document is served.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DocumentFileGetQuery {
    /// Return a bounded static preview instead of the original file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<bool>,
    /// Inspect retained deleted content under this object's current restoration authority.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inspect_deleted: Option<bool>,
    /// Whether to offer it as a download.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub download: Option<bool>,
}

/// What to list of a file that is an archive.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DocumentFileArchiveListQuery {
    /// Inspect retained deleted content under this object's current restoration authority.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inspect_deleted: Option<bool>,
    /// A directory within it to list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inside: Option<String>,
    /// How many entries at most, [`crate::workspace::ARCHIVE_LIST_LIMIT_DEFAULT`] when left out and
    /// at most [`crate::workspace::ARCHIVE_LIST_LIMIT_MAX`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// Which entry of an archive to read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DocumentFileArchiveEntryGetQuery {
    /// Inspect retained deleted content under this object's current restoration authority.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inspect_deleted: Option<bool>,
    /// A file within it.
    pub entry: String,
}

/// Says how far the caller has had a document on screen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DocumentSeenBody {
    /// The document sequence they had on screen.
    pub seq: Seq,
}

/// How a document is exported. Markdown is one `.md` file, or with images or attachments an archive
/// that holds it beside them. HTML is one file with its images inside it, or with attachments an
/// archive. Typst is an archive of the source with what it names. JSON is the blocks as a sync gives
/// them, and with comments the discussions. An archive is a zip.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DocumentExportGetQuery {
    /// What to write it as; `markdown` unless given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<DocumentExport>,
    /// Whether the files that are no images go with it; `omit` unless given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachments: Option<DocumentExportChoice>,
    /// Whether the discussions go with it; `omit` unless given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comments: Option<DocumentExportChoice>,
}

/// The discussions beside a document, the newest answer first.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DocumentDiscussionListQuery {
    /// Inspect retained deleted content under this object's current restoration authority.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inspect_deleted: Option<bool>,
    /// Which; `exclude` unless given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved: Option<DiscussionResolvedFilter>,
    /// How many entries, at most [`crate::common::MAX_LIMIT`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// The `next_cursor` of the page before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// A window of comments, including retained-content inspection under current restoration authority.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DocumentCommentListQuery {
    /// Inspect retained deleted content under this object's current restoration authority.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inspect_deleted: Option<bool>,
    /// How many entries, at most [`crate::common::MAX_LIMIT`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// The older-page boundary from the preceding reply.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// Opens a discussion on a block, with its first comment. Under an archived node nothing is added.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DocumentDiscussionCreateBody {
    /// The block.
    pub block_id: Id,
    /// The words it is about, at most [`DISCUSSION_QUOTE_MAX_CHARS`] characters, when some were
    /// selected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quote: Option<String>,
    /// The first comment.
    pub content: Vec<Inline>,
    /// A client-generated UUID: a repeated request answers as the first did.
    pub client_id: String,
}

/// Answers in a discussion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DocumentCommentCreateBody {
    /// What it says.
    pub content: Vec<Inline>,
    /// A client-generated UUID: a repeated request answers as the first did.
    pub client_id: String,
}

/// Resolves a discussion, or opens it again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DocumentDiscussionResolveBody {
    /// Whether it is resolved.
    pub resolved: bool,
}

/// Edits a comment. Modern clients pair a request identity with the revision on
/// which their local draft was based; content-only legacy calls remain supported.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DocumentCommentUpdateBody {
    /// What it says now.
    pub content: Vec<Inline>,
    /// A client-generated UUID for this exact edit attempt. Paired with `expected_seq`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    /// The comment revision when the draft was authored, never a newer revision
    /// silently adopted on retry. Paired with `client_id`. A stale unrecorded
    /// attempt returns `conflict.stale_revision` with the current visible
    /// `DocumentComment` under `current_comment`; an exact retained attempt
    /// returns its original result without changing the current comment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_seq: Option<Seq>,
}

/// Puts the caller's reaction to a comment on, or takes it off.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DocumentCommentReactionSetBody {
    /// The emoji.
    pub emoji: String,
    /// Whether the reaction is on.
    pub on: bool,
}

/// The node a document is to stand beside.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DocumentMoveBody {
    /// The node.
    pub node_id: Id,
}

/// Changes the caller's discussion bell without changing their read position.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DocumentSubscriptionSetBody {
    /// Which discussions they want to be told of.
    pub level: DocumentDiscussionLevel,
}

/// Restores only the current deletion selected by an audit event.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DocumentRestoreBody {
    /// The event's deletion group; absent for a separately confirmed current restore.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_deletion_id: Option<Id>,
}

/// Copies current readable material into the selected creation place.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NodeDocumentCopyBody {
    /// The readable source document.
    pub source_document_id: Id,
    /// A new title, or the source title when omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Fresh destination defaults or an explicit private choice.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access: Option<NewDocumentAccess>,
    /// Stable identity of this copy, reused after a lost answer.
    pub client_id: String,
}

/// The supported representation of an uploaded document import source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DocumentImportRequestFileFormat {
    /// Markdown text.
    Markdown,
    /// Inertly converted HTML text, XHTML included.
    Html,
    /// A Word document (`.docx`), converted to HTML first and then read as HTML.
    Docx,
    /// Plain text: paragraphs between blank lines, and nothing in it read as Markdown.
    Text,
    /// One archive containing supported text entries.
    Archive,
}

/// One retained uploaded source, interpreted only as its stated format.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DocumentImportRequestFile {
    /// Its blob.
    pub blob: Hash,
    /// Its source name.
    pub name: String,
    /// Its source representation.
    pub format: DocumentImportRequestFileFormat,
}

/// Several Markdown/HTML files, or one bounded archive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DocumentImportRequest {
    /// The selected uploaded sources.
    pub files: Vec<DocumentImportRequestFile>,
    /// Stable identity of this import across retries.
    pub client_id: String,
}

/// The refusal of one import entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DocumentImportResultError {
    /// Its ordinary refusal code.
    pub code: String,
    /// Its safe explanation.
    pub message: String,
}

/// One truthful made/refused outcome and every conversion loss.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DocumentImportResultEntry {
    /// The source entry's name.
    pub name: String,
    /// The created document, unchanged on a retry; null when refused.
    pub document: Option<DocumentRef>,
    /// Conversion losses, once each.
    pub warnings: Vec<String>,
    /// A refusal, or null after successful creation.
    pub error: Option<DocumentImportResultError>,
}

/// Every validated entry's outcome, in source order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DocumentImportResult {
    /// Successful and refused entries alike.
    pub entries: Vec<DocumentImportResultEntry>,
}

/// What a text that is converted into blocks is written in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DocumentConvertFormat {
    /// Orch Markdown: GFM with formulas, diagrams, alerts and the closed set of tags.
    Markdown,
    /// HTML or XHTML, read inertly.
    Html,
    /// Plain text.
    Text,
}

/// Turns text into blocks without storing them: the one reader that a paste goes through. With `document_id`
/// the document's own files and people are known to it, and the pictures a pasted page carries inside it are
/// stored as files of that document; without one they are left out and the answer says so.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DocumentConvertBody {
    /// What the text is written in.
    pub format: DocumentConvertFormat,
    /// The text.
    pub text: String,
    /// The document the blocks are for; the caller needs Edit on it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub document_id: Option<Id>,
}

/// One thing a reader kept as words or left out. A screen shows its own words for a `code` it knows, with `params` in
/// them, and `message` (English) only for one it does not.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReaderWarning {
    /// A stable code, such as `reader.image_missing`.
    pub code: String,
    /// What it is about: a path, the name of a tag, an address.
    pub params: std::collections::BTreeMap<String, String>,
    /// The English sentence, with the parameters in it.
    pub message: String,
}

/// The blocks a text says, each with a fresh identifier, and what could not be kept of it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DocumentConvertReply {
    /// The blocks, in order, with the blocks under each.
    pub blocks: Vec<NewBlock>,
    /// What was left out or kept as words, once each.
    pub warnings: Vec<ReaderWarning>,
    /// The files of the document that the conversion stored, which the blocks name.
    pub files: Vec<DocumentFile>,
}

/// The most characters of a part's text that a reference's card shows.
pub const REFERENCE_PREVIEW_TEXT_MAX_CHARS: u32 = 280;

/// Which thing, and which part of it, a reference's card is asked for: one of `block`, `turn` and
/// `message` at most, the one of the thing's kind.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReferencePreviewGetQuery {
    /// What kind of thing.
    pub kind: InlineRefKind,
    /// The node, the document or the chat.
    pub id: Id,
    /// A block of the document.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub block: Option<Id>,
    /// A turn of the thread's conversation, by its number.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<i64>,
    /// A message of the chat, by its number.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<i64>,
}

/// The reference card's query, including file paths and line numbers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReferencePreviewQuery {
    /// What kind of thing.
    pub kind: InlineRefKind,
    /// Its identifier; for an environment file, the environment.
    pub id: Id,
    /// A block of a document.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub block: Option<Id>,
    /// A turn of a thread.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<i64>,
    /// A message of a chat.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<i64>,
    /// A workspace-relative file path, required for an environment file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// A line of a text file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<i64>,
}

/// File metadata shown by a reference card, only after the reader's access is checked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReferencePreviewFile {
    /// The exact content hash when the owner still holds it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blob: Option<Hash>,
    /// The kind supplied by the authorized owner's file description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    /// The path inside an environment, absent for an attachment.
    pub path: Option<String>,
    /// Its current size in bytes, absent when a workspace file is gone or offline.
    pub size: Option<i64>,
    /// The attachment's owner, absent for a workspace file.
    pub owner_kind: Option<InlineRefKind>,
    /// The document or chat that owns an attachment.
    pub owner_id: Option<Id>,
    /// The owning message of a chat attachment, for opening it in place.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<Id>,
    /// The owner's readable title, or the environment's name.
    pub owner_name: String,
    /// Whether the environment is switched off. Readable metadata stays available.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub disabled: bool,
    /// The existing Enable controls, only for someone who may manage that environment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enable_path: Option<String>,
    /// Whether the file is known to be missing now.
    pub missing: bool,
}

/// The part a reference names, as its card shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReferencePreviewPart {
    /// The part as the reference keeps it.
    pub part: RefPart,
    /// Its number now, as `read_shared` counts it: a document's top-level block, a turn, a message.
    pub number: i64,
    /// Where it opens: the block, or the message, by its identifier; `null` for a turn, whose number is
    /// its address.
    pub anchor: Option<Id>,
    /// Its text, as far as a card shows it: at most [`REFERENCE_PREVIEW_TEXT_MAX_CHARS`] characters.
    pub text: String,
    /// Who wrote it, where that is known.
    pub by: Option<Author>,
}

/// What a reference's card shows of the thing it names, as the caller may read it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReferencePreview {
    /// A readable thread's icon and safe particulars; absent for other kinds or without thread View.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_display: Option<crate::threads::ThreadDisplay>,
    /// What kind of thing.
    pub kind: InlineRefKind,
    /// The thing.
    pub id: Id,
    /// For a node, which kind of node; `null` for a document and a chat.
    pub node_kind: Option<crate::nodes::NodeKind>,
    /// Its name now: a document's title, a place's or a thread's name, a chat's.
    pub title: String,
    /// Where it lies, named as the caller may know it: the node a document is written beside, the place a
    /// node lies in, the node a chat belongs to; `null` for a root org and for a direct or group chat.
    pub place: Option<NodeRef>,
    /// The first line of its description; for a document that has none, of its first paragraph. Empty
    /// where it has neither.
    pub description: String,
    /// When it last changed, where that is known.
    pub updated_at: Option<Time>,
    /// Who changed it last, where that is known.
    pub updated_by: Option<Author>,
    /// The part the reference names, where it names one that is still there.
    pub part: Option<ReferencePreviewPart>,
    /// File metadata; absent for places, threads, documents and chats.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<ReferencePreviewFile>,
    /// A safe unavailable state, with no current name or other metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unavailable: Option<ReferenceUnavailable>,
    /// When a direct personal grant next expires and this preview must be checked again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_at: Option<Time>,
    /// Open access requests the caller may decide here; absent for other readers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access_requests: Option<Vec<crate::approvals::Approval>>,
}

/// The safe card of a reference that the caller cannot currently read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReferenceUnavailable {
    /// Whether the target has been deleted.
    pub deleted: bool,
    /// The permission object, without a name; absent for a deleted target.
    pub object: Option<crate::access::AccessRequestObject>,
    /// The least permission needed to read or use it.
    pub permission: Option<String>,
    /// Whether the caller may ask for that permission.
    pub requestable: bool,
    /// The caller's open request, without its title or any target metadata.
    pub pending: Option<Id>,
}

/// A visible person who can be named in a document or a comment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DocumentMentionCandidate {
    /// The person to name.
    pub user: UserRef,
    /// Whether they can already read the document.
    pub can_view: bool,
}

crate::endpoints! {
    /// Reads the first document page of each expanded place under its own current authority.
    NodeDocumentPagesGet: GET "/nodes/{id}/document-pages", User, query NodeDocumentPagesGetQuery, none, json Vec<NodeDocumentPage>, 200;
    /// Lists readers first, then visible people the caller can share this document with.
    DocumentMentionList: GET "/documents/{id}/mentions", Permission("doc.view"), query crate::chats::ChatMentionListQuery, none, json Page<DocumentMentionCandidate>, 200;
    /// Copies readable current material with fresh identity, ownership and access.
    NodeDocumentCopy: POST "/nodes/{id}/documents/copy", Rule("destination doc.create with docs.enabled and source doc.view"), none, json NodeDocumentCopyBody, json Document, 201;
    /// Turns Markdown, HTML or plain text into blocks, without storing any: what a paste is read by.
    DocumentConvert: POST "/documents/convert", Rule("any signed-in caller; with document_id, Edit on that document"), none, json DocumentConvertBody, json DocumentConvertReply, 200;
    /// Imports several text files or one bounded archive and reports every entry.
    NodeDocumentImport: POST "/nodes/{id}/documents/import", Permission("doc.create"), none, json DocumentImportRequest, json DocumentImportResult, 200;
    /// Changes the caller's discussion bell on this document.
    DocumentSubscriptionSet: PUT "/documents/{id}/subscription", Permission("doc.view"), none, json DocumentSubscriptionSetBody, none, 204;
    /// Lists the documents written beside a node that the caller may read, the most recently changed first.
    NodeDocumentList: GET "/nodes/{id}/documents", Permission("node.view"), query NodeDocumentListQuery, none, json Page<Listed<DocumentRef>>, 200;
    /// Makes a document beside a node, from blocks or from Markdown.
    NodeDocumentCreate: POST "/nodes/{id}/documents", Permission("doc.create"), none, json NodeDocumentCreateBody, json Document, 201;
    /// Reads a document without its blocks.
    DocumentGet: GET "/documents/{id}", Permission("doc.view"), query crate::nodes::DeletedInspectionQuery, none, json Document, 200;
    /// Reads a document and every live block, or what changed since a document sequence.
    DocumentSyncGet: GET "/documents/{id}/sync", Permission("doc.view"), query DocumentSyncGetQuery, none, json DocumentSync, 200;
    /// What a reference's card shows: the thing it names and the part, as the caller may read them; anything the caller may not read is not found.
    ReferencePreviewGet: GET "/references/preview", Rule("reading the reference's target or attachment owner, or files.view on its environment"), query ReferencePreviewQuery, none, json ReferencePreview, 200;
    /// Renames a document, or changes its description.
    DocumentUpdate: PATCH "/documents/{id}", Permission("doc.edit"), none, json DocumentUpdateBody, json Document, 200;
    /// Applies a batch of operations to a document, whole or not at all.
    DocumentOpsCreate: POST "/documents/{id}/ops", Permission("doc.edit"), none, json DocumentOpsCreateBody, json DocumentOpsCreateReply, 200;
    /// Lists the entries of a document's history, newest first; the open span, when something changed since the last version was cut, comes first.
    DocumentVersionList: GET "/documents/{id}/versions", Permission("doc.view"), query DocumentVersionListQuery, none, json Page<DocumentVersion>, 200;
    /// Says what differs between two states of a document, block by block.
    DocumentChangesGet: GET "/documents/{id}/changes", Permission("doc.view"), query DocumentChangesGetQuery, none, json DocumentChanges, 200;
    /// Reads a document as it stood at a version.
    DocumentVersionGet: GET "/documents/{id}/versions/{version}", Permission("doc.view"), query crate::nodes::DeletedInspectionQuery, none, json DocumentVersionGetReply, 200;
    /// Puts a version back: the present state is cut as a version first, then the blocks of that version stand again.
    DocumentVersionRestore: POST "/documents/{id}/versions/{version}/restore", Permission("doc.manage"), none, none, json DocumentSync, 200;
    /// Puts a file of any kind into a document, from a raw body with a filename header or from a finished upload.
    DocumentFileCreate: POST "/documents/{id}/files", Permission("doc.edit"), query DocumentFileCreateQuery, bytes, json DocumentFile, 200;
    /// Removes a file from a document, unless a live block names it.
    DocumentFileDelete: DELETE "/documents/{id}/files/{file_id}", Permission("doc.edit"), none, none, none, 204;
    /// Serves a file of a document: images, films and sounds inline, with `Range`; anything else as an attachment.
    DocumentFileGet: GET "/documents/{id}/files/{file_id}", Permission("doc.view"), query DocumentFileGetQuery, none, raw, 200;
    /// Lists the entries of a file of a document that is an archive.
    DocumentFileArchiveList: GET "/documents/{id}/files/{file_id}/archive", Permission("doc.view"), query DocumentFileArchiveListQuery, none, json ArchiveListing, 200;
    /// Serves one entry of an archive that is a file of a document.
    DocumentFileArchiveEntryGet: GET "/documents/{id}/files/{file_id}/archive/entry", Permission("doc.view"), query DocumentFileArchiveEntryGetQuery, none, raw, 200;
    /// Says up to which sequence the caller has had a document on screen.
    DocumentSeen: POST "/documents/{id}/seen", Permission("doc.view"), none, json DocumentSeenBody, none, 204;
    /// Writes a document as Markdown, HTML, Typst or JSON, as a file download. It takes the document's Export row, and reading the document as well.
    DocumentExportGet: GET "/documents/{id}/export", Permission("doc.export"), query DocumentExportGetQuery, none, raw, 200;
    /// Lists the discussions beside a document, the newest answer first.
    DocumentDiscussionList: GET "/documents/{id}/discussions", Permission("doc.view"), query DocumentDiscussionListQuery, none, json Page<DocumentDiscussion>, 200;
    /// Opens a discussion on a block of a document, with its first comment.
    DocumentDiscussionCreate: POST "/documents/{id}/discussions", Permission("doc.comment"), none, json DocumentDiscussionCreateBody, json DocumentDiscussion, 201;
    /// Lists a discussion's comments in the order they were written, a window at a time from the newest.
    DocumentCommentList: GET "/documents/{id}/discussions/{discussion_id}/comments", Permission("doc.view"), query DocumentCommentListQuery, none, json DocumentCommentPage, 200;
    /// Answers in a discussion.
    DocumentCommentCreate: POST "/documents/{id}/discussions/{discussion_id}/comments", Permission("doc.comment"), none, json DocumentCommentCreateBody, json DocumentComment, 201;
    /// Resolves a discussion, or opens it again.
    DocumentDiscussionResolve: POST "/documents/{id}/discussions/{discussion_id}/resolve", Rule("doc.edit, or having written in that discussion"), none, json DocumentDiscussionResolveBody, json DocumentDiscussion, 200;
    /// Edits a comment.
    DocumentCommentUpdate: PATCH "/documents/{id}/comments/{comment_id}", Rule("its author"), none, json DocumentCommentUpdateBody, json DocumentComment, 200;
    /// Deletes a comment, which stays as an emptied line.
    DocumentCommentDelete: DELETE "/documents/{id}/comments/{comment_id}", Rule("its author, or doc.manage"), none, none, none, 204;
    /// Puts the caller's reaction to a comment on, or takes it off.
    DocumentCommentReactionSet: PUT "/documents/{id}/comments/{comment_id}/reaction", Permission("doc.comment"), none, json DocumentCommentReactionSetBody, json DocumentComment, 200;
    /// Says who gains and who loses what when a document's rows follow those of another node.
    DocumentMovePreview: POST "/documents/{id}/move/preview", Rule("as the move"), none, json DocumentMoveBody, json PagePreview, 200;
    /// Moves a document beside another node.
    DocumentMove: POST "/documents/{id}/move", Rule("doc.manage on the document, and doc.create on the node it goes to, with docs.enabled there"), none, json DocumentMoveBody, json Document, 200;
    /// Deletes a document, which can be restored until it is purged.
    DocumentDelete: DELETE "/documents/{id}", Permission("doc.manage"), none, none, none, 204;
    /// Restores a deleted document.
    DocumentRestore: POST "/documents/{id}/restore", Rule("doc.manage on it as it was"), none, json DocumentRestoreBody, json Document, 200;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_discussion_bell_selects_all_or_involved_without_an_edit_or_seen_value() {
        for level in ["all", "involved"] {
            let value = json!({ "level": level });
            let read: DocumentSubscriptionSetBody = serde_json::from_value(value.clone()).unwrap();
            assert_eq!(serde_json::to_value(read).unwrap(), value);
        }
        assert!(
            serde_json::from_value::<DocumentSubscriptionSetBody>(json!({ "level": "off" }))
                .is_err()
        );
        assert!(
            serde_json::from_value::<DocumentSubscriptionSetBody>(
                json!({ "level": "all", "seq": 1 })
            )
            .is_err()
        );
    }

    #[test]
    fn a_reference_keeps_its_part_as_the_kind_has_it_and_a_reference_without_one_reads_as_before() {
        for written in [
            json!({ "type": "ref", "kind": "document", "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5e03",
                    "part": { "block": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5e04" } }),
            json!({ "type": "ref", "kind": "node", "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5e05",
                    "part": { "turn": 4 } }),
            json!({ "type": "ref", "kind": "chat", "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5e06",
                    "part": { "message": 120 } }),
            json!({ "type": "ref", "kind": "file", "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5e08",
                    "part": { "line": 12 } }),
            json!({ "type": "ref", "kind": "environment_file", "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5e09",
                    "path": "src/auth.rs", "part": { "line": 42 } }),
            json!({ "type": "ref", "kind": "document", "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5e07" }),
        ] {
            let read: Inline = serde_json::from_value(written.clone()).unwrap();
            assert_eq!(serde_json::to_value(read).unwrap(), written);
        }
        let preview: ReferencePreview = serde_json::from_value(json!({
            "kind": "document", "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5e03", "node_kind": null,
            "title": "Rollout plan", "place": null, "description": "How we ship",
            "updated_at": "2026-10-08T17:04:05.123Z", "updated_by": null,
            "part": { "part": { "block": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5e04" }, "number": 3,
                      "anchor": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5e04", "text": "Ship it.", "by": null }
        }))
        .unwrap();
        assert_eq!(preview.part.map(|part| part.number), Some(3));
        assert!(
            serde_json::from_value::<ReferencePreviewGetQuery>(
                json!({ "kind": "chat", "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5e06", "page": 1 })
            )
            .is_err(),
            "an unknown member is refused"
        );
    }

    #[test]
    fn a_change_of_a_document_names_its_title_its_description_or_both() {
        for value in [
            json!({ "title": "Guide" }),
            json!({ "description": "How we ship\nand roll back" }),
            json!({ "title": "Guide", "description": "" }),
        ] {
            let read: DocumentUpdateBody = serde_json::from_value(value.clone()).unwrap();
            assert_eq!(serde_json::to_value(read).unwrap(), value);
        }
        assert!(
            serde_json::from_value::<DocumentUpdateBody>(json!({ "summary": "x" })).is_err(),
            "an unknown member is refused"
        );
    }

    fn user(n: u8) -> serde_json::Value {
        json!({
            "id": format!("0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a{n:02x}"), "kind": "human", "username": format!("user{n}"),
            "display_name": format!("User {n}"), "avatar": { "text": "U", "color": 3 }
        })
    }

    #[test]
    fn a_heading_is_read_with_its_level_its_text_and_who_changed_it_last() {
        let written = json!({
            "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5e01",
            "document_id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5e02",
            "parent_id": null,
            "position": "1.5",
            "type": "heading",
            "props": { "level": 2, "toggle": true, "color": 4 },
            "content": [
                { "type": "text", "text": "Release ", "marks": ["bold"] },
                { "type": "mention", "user_id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a01", "user": user(1) },
                { "type": "ref", "kind": "document", "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5e03", "name": null },
                { "type": "math", "source": "x^2" },
                { "type": "break" }
            ],
            "removed": false,
            "seq_created": 3,
            "seq_updated": 9,
            "updated_by": { "agent": "codex", "thread": {
                "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5e04", "kind": "thread", "name": null, "redacted": true
            } },
            "updated_at": "2026-10-08T17:04:05.123Z"
        });
        let block: DocumentBlock = serde_json::from_value(written.clone()).unwrap();
        assert_eq!(block.block_type, BlockType::Heading);
        assert!(matches!(&block.props, BlockProps::Heading(p) if p.level == 2));
        assert!(matches!(block.updated_by, Some(Author::Agent(_))));
        match &block.content {
            BlockContent::Inline(runs) => {
                assert_eq!(runs.len(), 5);
                assert!(matches!(
                    &runs[2],
                    Inline::Ref {
                        display: None,
                        path: None,
                        name: Some(None),
                        ..
                    }
                ));
            }
            other => panic!("not inline content: {other:?}"),
        }
        assert_eq!(serde_json::to_value(&block).unwrap(), written);
    }

    #[test]
    fn a_table_holds_rows_of_cells_that_may_span() {
        let written = json!({
            "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5e11",
            "type": "table",
            "props": { "header_row": true, "widths": [1.0, 2.5] },
            "content": [
                { "cells": [
                    { "content": [{ "type": "text", "text": "Name" }], "colspan": 2, "background": 1 }
                ] },
                { "cells": [
                    { "content": [{ "type": "text", "text": "a" }] },
                    { "content": [] }
                ] }
            ]
        });
        let block: NewBlock = serde_json::from_value(written.clone()).unwrap();
        assert!(matches!(block.props, Some(BlockProps::Table(_))));
        match block.content {
            Some(BlockContent::Rows(ref rows)) => {
                assert_eq!(rows.len(), 2);
                assert_eq!(rows[0].cells[0].colspan, Some(2));
            }
            ref other => panic!("not rows: {other:?}"),
        }
        assert_eq!(serde_json::to_value(&block).unwrap(), written);
    }

    #[test]
    fn props_are_told_apart_by_the_members_they_hold() {
        let read =
            |written: serde_json::Value| serde_json::from_value::<BlockProps>(written).unwrap();
        assert!(matches!(read(json!({})), BlockProps::Text(_)));
        assert!(matches!(
            read(json!({ "align": "justify" })),
            BlockProps::Text(_)
        ));
        assert!(matches!(
            read(json!({ "checked": false })),
            BlockProps::Todo(_)
        ));
        assert!(matches!(
            read(json!({ "tone": "danger", "color": 2 })),
            BlockProps::Callout(_)
        ));
        assert!(matches!(
            read(json!({ "language": "rust", "wrap": true })),
            BlockProps::Code(_)
        ));
        assert!(matches!(
            read(json!({ "source": "graph TD" })),
            BlockProps::Source(_)
        ));
        assert!(matches!(
            read(json!({ "start": 4 })),
            BlockProps::Numbered(_)
        ));
        assert!(matches!(
            read(json!({ "share": 2.0 })),
            BlockProps::Column(_)
        ));
        assert!(matches!(
            read(json!({ "url": "https://example.com/clip.mp4", "width": 480.5 })),
            BlockProps::Media(_)
        ));
        assert!(
            serde_json::from_value::<BlockProps>(json!({ "level": 2, "tone": "info" })).is_err(),
            "a member that no type takes together is refused"
        );
        assert!(
            serde_json::from_value::<BlockProps>(json!({ "colour": 2 })).is_err(),
            "an unknown member is refused"
        );
    }

    #[test]
    fn a_batch_of_operations_is_told_by_op_and_refuses_what_it_does_not_know() {
        let body: DocumentOpsCreateBody = serde_json::from_value(json!({
            "client_id": "7b7a4c1e-0f0e-4d57-9d5e-6f3a9a8b2c11",
            "ops": [
                { "op": "insert",
                  "block": { "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5f01", "type": "toggle",
                             "content": [{ "type": "text", "text": "More" }],
                             "children": [{ "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5f02", "type": "paragraph" }] },
                  "parent_id": null, "after_id": null },
                { "op": "update", "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5f03", "base_seq": 8,
                  "type": "quote", "content": [] },
                { "op": "move", "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5f01",
                  "parent_id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5f04", "after_id": null },
                { "op": "remove", "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5f02" }
            ]
        }))
        .unwrap();
        assert_eq!(body.ops.len(), 4);
        match &body.ops[1] {
            DocumentOp::Update {
                block_type, props, ..
            } => {
                assert_eq!(*block_type, Some(BlockType::Quote));
                assert!(props.is_none());
            }
            other => panic!("not an update: {other:?}"),
        }
        assert!(
            serde_json::from_value::<DocumentOp>(
                json!({ "op": "remove", "id": "x", "force": true })
            )
            .is_err()
        );
        assert!(
            serde_json::from_value::<DocumentOp>(json!({ "op": "rename", "id": "x" })).is_err()
        );
    }

    #[test]
    fn the_twenty_block_types_are_written_in_snake_case() {
        let all = [
            BlockType::Paragraph,
            BlockType::Heading,
            BlockType::Bullet,
            BlockType::Numbered,
            BlockType::Todo,
            BlockType::Toggle,
            BlockType::Quote,
            BlockType::Callout,
            BlockType::Code,
            BlockType::Math,
            BlockType::Diagram,
            BlockType::Divider,
            BlockType::PageBreak,
            BlockType::Image,
            BlockType::Video,
            BlockType::Audio,
            BlockType::File,
            BlockType::Table,
            BlockType::Columns,
            BlockType::Column,
        ];
        assert_eq!(all.len(), 20);
        assert_eq!(
            serde_json::to_value(BlockType::PageBreak).unwrap(),
            json!("page_break")
        );
    }

    #[test]
    fn a_version_of_the_open_span_has_no_number_and_no_cause() {
        let version: DocumentVersion = serde_json::from_value(json!({
            "version": null, "title": "Handbook", "bytes": 2048, "block_count": 12, "cause": null,
            "authors": [user(1)],
            "changed": { "inserted": 2, "updated": 5, "moved": 0, "removed": 1 },
            "from": "2026-10-08T17:04:05.123Z", "to": "2026-10-08T17:20:00.000Z"
        }))
        .unwrap();
        assert_eq!(version.version, None);
        assert_eq!(version.cause, None);
        assert!(matches!(version.authors[0], Author::User(_)));
    }
}
