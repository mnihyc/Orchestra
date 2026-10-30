//! The one reader at the size of a document that a place may be set to hold, and of a text written to be slow: every path is linear in the text, and the depth of what it makes is bounded.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use orch_api_contract::documents::{BlockContent, BlockType, Inline, NewBlock};
use orch_documents::convert::{self, Format};
use orch_documents::markdown::{NoDirectory, Read};

fn read(text: &str) -> Read {
    convert::prepare(Format::Markdown, text.as_bytes())
        .unwrap()
        .finish(&NoDirectory)
}

/// How deep the blocks stand in one another, counted without recursion.
fn depth(blocks: &[NewBlock]) -> usize {
    let mut deepest = 0;
    let mut stack: Vec<(&NewBlock, usize)> = blocks.iter().map(|block| (block, 1)).collect();
    while let Some((block, level)) = stack.pop() {
        deepest = deepest.max(level);
        for child in block.children.iter().flatten() {
            stack.push((child, level + 1));
        }
    }
    deepest
}

fn words(block: &NewBlock) -> String {
    match &block.content {
        Some(BlockContent::Inline(runs)) => runs
            .iter()
            .filter_map(|run| match run {
                Inline::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect(),
        _ => String::new(),
    }
}

#[test]
fn a_document_of_a_hundred_thousand_blocks_is_read_whole() {
    let text: String = (0..100_000)
        .map(|index| {
            format!(
                "Paragraph {index} with **bold** and a [link](https://example.org/{index}).\n\n"
            )
        })
        .collect();
    let found = read(&text);
    assert_eq!(found.blocks.len(), 100_000);
    assert!(words(&found.blocks[99_999]).starts_with("Paragraph 99999 with "));
    assert!(found.warnings.is_empty());
}

#[test]
fn a_hundred_thousand_layout_comments_keep_one_block_each() {
    let text: String = (0..100_000).map(|index| format!(
        "<!-- orch:block {{\"kind\":\"paragraph\",\"props\":{{\"color\":1,\"align\":\"center\"}}}} -->\n\nParagraph {index}\n\n<!-- /orch:block -->\n\n"
    )).collect();
    let found = read(&text);
    assert_eq!(found.blocks.len(), 100_000);
    assert_eq!(words(&found.blocks[99_999]), "Paragraph 99999");
    assert!(found.warnings.is_empty(), "{:?}", found.warnings);
    for block in &found.blocks {
        let props = serde_json::to_value(&block.props).unwrap();
        assert_eq!(props["color"], 1);
        assert_eq!(props["align"], "center");
    }
}

#[test]
fn hundreds_of_thousands_of_broken_links_are_listed_in_a_page_and_read_at_once() {
    let text: String = (0..100_000)
        .map(|index| format!("See [it](missing/{index}.md) and ![x](images/{index}.png).\n\n"))
        .collect();
    let found = read(&text);
    assert_eq!(found.blocks.len(), 100_000);
    // The warnings and the missing paths are listed to a bound, with one more warning saying there are more.
    assert!(found.warnings.len() <= 201, "{}", found.warnings.len());
    assert!(found.warnings.iter().any(|each| each.code == "reader.more"));
    assert!(found.missing.len() <= 200, "{}", found.missing.len());
}

#[test]
fn a_thousand_toggles_and_columns_are_paired_in_one_pass() {
    let mut text = String::new();
    for index in 0..5_000 {
        text.push_str(&format!(
            "<details>\n<summary>Toggle {index}</summary>\n\nInside {index}\n\n</details>\n\n"
        ));
        text.push_str("<columns>\n<column ratio=\"1\">\n\nleft\n\n</column>\n<column ratio=\"1\">\n\nright\n\n</column>\n</columns>\n\n");
    }
    let found = read(&text);
    assert_eq!(found.blocks.len(), 10_000);
    assert_eq!(found.blocks[0].block_type, BlockType::Toggle);
    assert_eq!(found.blocks[1].block_type, BlockType::Columns);
}

#[test]
fn tags_that_never_close_are_words_and_do_not_slow_the_pairing() {
    let mut text = "<details>\n".repeat(50_000);
    text.push_str(&"</column>\n".repeat(50_000));
    text.push_str("\nThe end.\n");
    let found = read(&text);
    assert!(
        found
            .blocks
            .iter()
            .all(|block| block.block_type != BlockType::Toggle)
    );
    assert!(
        found
            .warnings
            .iter()
            .any(|each| each.code == "reader.tag_kept_as_text")
    );
}

#[test]
fn containers_nested_deeper_than_a_document_holds_are_read_flat() {
    // A toggle inside a toggle, a hundred thousand times.
    let mut text = "<details>\n".repeat(100_000);
    text.push_str("\ninside\n\n");
    text.push_str(&"</details>\n".repeat(100_000));
    let found = read(&text);
    assert!(depth(&found.blocks) <= 64, "{}", depth(&found.blocks));

    // Quotes and list items, opened on one line a hundred thousand times.
    let quotes = format!("{} deep\n", ">".repeat(100_000));
    let found = read(&quotes);
    assert!(depth(&found.blocks) <= 64, "{}", depth(&found.blocks));
    let items = format!("{}deep\n", "- ".repeat(100_000));
    let found = read(&items);
    assert!(depth(&found.blocks) <= 64, "{}", depth(&found.blocks));
}

#[test]
fn a_line_of_a_million_looks_is_words_beyond_the_depth_a_writing_needs() {
    let opened = format!("{}words\n", "<u>".repeat(300_000));
    let found = read(&opened);
    assert_eq!(found.blocks.len(), 1);
    assert!(words(&found.blocks[0]).ends_with("words"));
    let closed = format!("{}words{}\n", "<u>".repeat(300_000), "</u>".repeat(300_000));
    let found = read(&closed);
    assert_eq!(found.blocks.len(), 1);
}
