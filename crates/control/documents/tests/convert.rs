//! Every kind of source read into blocks by the one reader: Markdown with its relative links and pictures, HTML as Orch writes it and as other tools write it, a Word file, and plain text.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use std::collections::HashMap;

use orch_api_contract::documents::{
    BlockContent, BlockProps, BlockType, Inline, InlineRefKind, NewBlock,
};
use orch_documents::blocks;
use orch_documents::convert::{self, Format};
use orch_documents::markdown::{Directory, FileRef, Local, TargetKind};
use orch_documents::references::Typed;
use orch_types::{Id, new_id};

/// What an import knows of the files that came with a text.
#[derive(Default)]
struct Neighbours {
    base: String,
    documents: HashMap<String, Id>,
    files: HashMap<String, FileRef>,
}

impl Directory for Neighbours {
    fn local(&self, path: &str) -> Option<Local> {
        let joined = convert::join_path(&self.base, path)?;
        if let Some(id) = self.documents.get(&joined) {
            return Some(Local::Document(Typed {
                kind: InlineRefKind::Document,
                id: *id,
                path: None,
                part: None,
            }));
        }
        self.files.get(&joined).cloned().map(Local::File)
    }
}

fn read(format: Format, text: &str, directory: &dyn Directory) -> orch_documents::markdown::Read {
    convert::prepare(format, text.as_bytes())
        .unwrap()
        .finish(directory)
}

fn text_of(block: &NewBlock) -> String {
    match &block.content {
        Some(BlockContent::Inline(runs)) => blocks::inline_text(runs),
        _ => String::new(),
    }
}

fn types(blocks: &[NewBlock]) -> Vec<BlockType> {
    blocks.iter().map(|block| block.block_type).collect()
}

/// Every block of the tree must be one that the editor could have made.
fn assert_valid(blocks: &[NewBlock]) {
    for block in blocks {
        blocks::check(
            block.block_type,
            block.props.as_ref(),
            block.content.as_ref(),
        )
        .unwrap_or_else(|faults| panic!("{:?} is not a valid block: {faults:?}", block.block_type));
        assert_valid(block.children.as_deref().unwrap_or(&[]));
    }
}

#[test]
fn a_relative_link_that_leads_nowhere_is_the_words_it_was_written_with_and_never_a_fault() {
    let text = "See [the plan](plan/other.md) and [data](data.csv#top), [here](#part), [bad](javascript:alert(1)) and ![chart](img/chart.png).\n";
    let found = read(Format::Markdown, text, &Neighbours::default());
    assert_valid(&found.blocks);
    assert_eq!(found.blocks.len(), 1);
    assert_eq!(
        text_of(&found.blocks[0]),
        "See the plan and data, here, bad and chart."
    );
    let codes: Vec<&str> = found
        .warnings
        .iter()
        .map(|each| each.code.as_str())
        .collect();
    assert!(codes.contains(&"reader.link_unresolved"), "{codes:?}");
    assert!(codes.contains(&"reader.image_missing"), "{codes:?}");
    assert!(codes.contains(&"reader.link_not_followed"), "{codes:?}");
    let mut missing = found.missing.clone();
    missing.sort();
    assert_eq!(missing, ["data.csv", "img/chart.png", "plan/other.md"]);
    let missing = found
        .warnings
        .iter()
        .find(|each| each.code == "reader.image_missing")
        .unwrap();
    assert_eq!(missing.params["path"], "img/chart.png");
}

#[test]
fn a_link_to_another_text_that_is_read_with_it_is_a_reference_and_a_picture_that_came_along_is_a_block()
 {
    let other = new_id();
    let picture = new_id();
    let directory = Neighbours {
        base: "notes".to_owned(),
        documents: HashMap::from([("plans/Plan.md".to_owned(), other)]),
        files: HashMap::from([(
            "notes/images/a b.png".to_owned(),
            FileRef {
                id: picture,
                media_type: "image/png".to_owned(),
            },
        )]),
    };
    let text = "Read [the plan](../plans/Plan.md#3) first.\n\n![A chart](images/a%20b.png)\n";
    let prepared = convert::prepare(Format::Markdown, text.as_bytes()).unwrap();
    let targets = prepared.targets();
    assert_eq!(
        targets
            .iter()
            .map(|target| (target.path.as_str(), target.kind))
            .collect::<Vec<_>>(),
        [
            ("../plans/Plan.md", TargetKind::Link),
            ("images/a b.png", TargetKind::Image)
        ]
    );
    let found = prepared.finish(&directory);
    assert_valid(&found.blocks);
    assert!(found.warnings.is_empty(), "{:?}", found.warnings);
    assert_eq!(
        types(&found.blocks),
        [BlockType::Paragraph, BlockType::Image]
    );
    let Some(BlockContent::Inline(runs)) = &found.blocks[0].content else {
        panic!("text");
    };
    assert!(runs.iter().any(
        |run| matches!(run, Inline::Ref { id, kind: InlineRefKind::Document, .. } if *id == other)
    ));
    let Some(BlockProps::Image(image)) = &found.blocks[1].props else {
        panic!("image");
    };
    assert_eq!(image.file_id, picture);
    assert_eq!(text_of(&found.blocks[1]), "A chart");
}

#[test]
fn a_picture_carried_in_a_data_address_becomes_a_file_to_hold_and_a_block_that_names_it() {
    let png = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==";
    let text = format!("![tiny](data:image/png;base64,{png})\n");
    let found = read(Format::Markdown, &text, &Neighbours::default());
    assert_valid(&found.blocks);
    assert_eq!(types(&found.blocks), [BlockType::Image]);
    assert_eq!(found.embedded.len(), 1);
    assert_eq!(found.embedded[0].media_type, "image/png");
    assert!(found.embedded[0].bytes.starts_with(b"\x89PNG"));
    let Some(BlockProps::Image(image)) = &found.blocks[0].props else {
        panic!("image");
    };
    assert_eq!(image.file_id, found.embedded[0].id);
}

#[test]
fn what_orch_writes_as_html_reads_back_as_the_blocks_it_was() {
    let page = "<h1 class=\"title\">Plan</h1>\n\
        <details class=\"toggle\"><summary>More</summary>\n<p>inside</p></details>\n\
        <aside class=\"callout tone-warning\"><p>Mind the gap</p></aside>\n\
        <pre><code class=\"language-rust\">fn main() {}</code></pre>\n\
        <ul class=\"todos\"><li><input type=\"checkbox\" disabled checked> done</li></ul>\n\
        <div class=\"columns\"><div class=\"column\" style=\"flex-grow:2\"><p>left</p></div><div class=\"column\"><p>right</p></div></div>\n\
        <div class=\"page-break\" role=\"separator\"></div>\n\
        <p class=\"t0 b2\">Coloured <span class=\"t7\">words</span></p>\n\
        <figure><img src=\"data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==\" alt=\"pic\"><figcaption>The pic</figcaption></figure>\n";
    let found = read(Format::Html, page, &Neighbours::default());
    assert_valid(&found.blocks);
    assert_eq!(
        types(&found.blocks),
        [
            BlockType::Heading,
            BlockType::Toggle,
            BlockType::Callout,
            BlockType::Code,
            BlockType::Todo,
            BlockType::Columns,
            BlockType::PageBreak,
            BlockType::Paragraph,
            BlockType::Image,
        ]
    );
    assert_eq!(
        found.embedded.len(),
        1,
        "the picture becomes a file to hold"
    );
    let Some(BlockProps::Code(code)) = &found.blocks[3].props else {
        panic!("code");
    };
    assert_eq!(code.language, "rust");
    let Some(BlockProps::Image(image)) = &found.blocks[8].props else {
        panic!("image");
    };
    assert_eq!(image.file_id, found.embedded[0].id);
    assert_eq!(text_of(&found.blocks[8]), "The pic");
}

#[test]
fn a_page_that_names_a_picture_by_a_path_or_an_address_on_the_web_keeps_the_words_and_says_so() {
    let page = "<p>One <img src=\"https://example.org/x.png\" alt=\"remote\"> two <a href=\"other.html\">next</a> <a href=\"javascript:alert(1)\">bad</a></p>\n<img src=\"missing/a.png\" alt=\"gone\">\n";
    let found = read(Format::Html, page, &Neighbours::default());
    assert_valid(&found.blocks);
    let codes: Vec<&str> = found
        .warnings
        .iter()
        .map(|each| each.code.as_str())
        .collect();
    assert!(codes.contains(&"reader.image_remote"), "{codes:?}");
    assert!(codes.contains(&"reader.image_missing"), "{codes:?}");
    assert!(codes.contains(&"reader.link_unresolved"), "{codes:?}");
    assert!(codes.contains(&"reader.unsafe_link"), "{codes:?}");
    assert!(
        found
            .blocks
            .iter()
            .all(|block| block.block_type != BlockType::Image),
        "no picture was found to show"
    );
}

#[test]
fn a_word_file_with_headings_a_table_and_a_picture_becomes_blocks_and_a_file() {
    let bytes = include_bytes!("fixtures/report.docx");
    let found = convert::prepare(Format::Docx, bytes)
        .unwrap()
        .finish(&Neighbours::default());
    assert_valid(&found.blocks);
    let kinds = types(&found.blocks);
    assert!(kinds.contains(&BlockType::Heading), "{kinds:?}");
    assert!(kinds.contains(&BlockType::Table), "{kinds:?}");
    assert!(kinds.contains(&BlockType::Image), "{kinds:?}");
    assert!(kinds.contains(&BlockType::Bullet), "{kinds:?}");
    assert!(kinds.contains(&BlockType::Numbered), "{kinds:?}");
    assert_eq!(found.embedded.len(), 1);
    assert!(found.embedded[0].bytes.starts_with(b"\x89PNG"));
    let table = found
        .blocks
        .iter()
        .find(|block| block.block_type == BlockType::Table)
        .unwrap();
    let Some(BlockContent::Rows(rows)) = &table.content else {
        panic!("rows");
    };
    assert_eq!(rows.len(), 2);
    assert_eq!(blocks::inline_text(&rows[1].cells[0].content), "alpha");
    let first = found
        .blocks
        .iter()
        .find(|block| block.block_type == BlockType::Heading)
        .unwrap();
    assert_eq!(
        text_of(first),
        "Quarterly report",
        "a Word title is a heading"
    );
    let underlined = found.blocks.iter().any(|block| match &block.content {
        Some(BlockContent::Inline(runs)) => runs.iter().any(|run| {
            matches!(run, Inline::Text { text, marks: Some(marks), .. }
                if text == "underline" && marks.contains(&orch_api_contract::documents::Mark::Underline))
        }),
        _ => false,
    });
    assert!(underlined, "underlined words stay underlined");
}

#[test]
fn a_file_that_is_not_a_word_file_is_refused_not_hung_on() {
    let error = convert::prepare(Format::Docx, b"not a zip at all").unwrap_err();
    assert!(error.to_string().contains("Word"), "{error}");
}

#[test]
fn plain_text_is_paragraphs_and_a_text_that_is_not_utf8_is_refused() {
    let found = read(
        Format::Text,
        "# not a heading\nsecond line\n\n- not a list\n",
        &Neighbours::default(),
    );
    assert_eq!(
        types(&found.blocks),
        [BlockType::Paragraph, BlockType::Paragraph]
    );
    assert!(convert::prepare(Format::Markdown, &[0xff, 0xfe, 0x00]).is_err());
}

#[test]
fn a_notion_style_export_of_two_pages_with_a_picture_reads_as_linked_documents() {
    let second = new_id();
    let picture = new_id();
    let directory = Neighbours {
        base: String::new(),
        documents: HashMap::from([("Second page.md".to_owned(), second)]),
        files: HashMap::from([(
            "Page.files/cat.png".to_owned(),
            FileRef {
                id: picture,
                media_type: "image/png".to_owned(),
            },
        )]),
    };
    let text = "# Page\n\nSee [Second page](Second%20page.md).\n\n![cat](Page.files/cat.png)\n\n<details>\n<summary>More</summary>\n\tindented child\n</details>\n";
    let found = read(Format::Markdown, text, &directory);
    assert_valid(&found.blocks);
    assert_eq!(
        types(&found.blocks),
        [
            BlockType::Heading,
            BlockType::Paragraph,
            BlockType::Image,
            BlockType::Toggle
        ]
    );
    // The link was written with an encoded space and leads to the document that is read with it.
    let Some(BlockContent::Inline(runs)) = &found.blocks[1].content else {
        panic!("text");
    };
    assert!(
        runs.iter()
            .any(|run| matches!(run, Inline::Ref { id, .. } if *id == second)),
        "{runs:?}"
    );
    assert!(found.missing.is_empty(), "{:?}", found.missing);
}
