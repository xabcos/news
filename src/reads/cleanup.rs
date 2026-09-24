//! Editorial cleanup selects original Markdown blocks; it never rewrites source text.

use super::{full_text::OPTIONS, invalid_source};
use crate::error::Result;
use pulldown_cmark::{Event, Parser, Tag};
use serde::Serialize;
use std::{collections::HashSet, ops::Range};

#[derive(Serialize)]
pub(super) struct Block<'a> {
    id: usize,
    text: &'a str,
    removable: bool,
    #[serde(skip)]
    range: Range<usize>,
}

pub(super) fn blocks(body: &str) -> Vec<Block<'_>> {
    let mut blocks = Vec::new();
    let (mut depth, mut start, mut removable) = (0, 0, false);
    for (event, range) in Parser::new_ext(body, OPTIONS).into_offset_iter() {
        match event {
            Event::Start(tag) => {
                if depth == 0 {
                    start = range.start;
                    // Structural blocks, including references in lists, are never AI-deletable.
                    removable = matches!(tag, Tag::Paragraph);
                }
                if matches!(tag, Tag::Image { .. }) {
                    removable = false;
                }
                depth += 1;
            }
            Event::End(_) => {
                depth -= 1;
                if depth == 0 {
                    blocks.push(Block {
                        id: blocks.len(),
                        text: &body[start..range.end],
                        removable,
                        range: start..range.end,
                    });
                }
            }
            Event::Code(_) | Event::FootnoteReference(_) => removable = false,
            _ if depth == 0 => blocks.push(Block {
                id: blocks.len(),
                text: &body[range.clone()],
                removable: false,
                range,
            }),
            _ => {}
        }
    }
    // A caption often occupies a separate paragraph immediately after its illustration.
    for i in 1..blocks.len() {
        if blocks[i - 1].text.contains("reads-image:") {
            blocks[i].removable = false;
        }
    }
    blocks
}

pub(super) fn clean(body: &str, remove: &[usize]) -> Result<String> {
    if remove.is_empty() {
        return Ok(body.to_owned());
    }
    let blocks = blocks(body);
    let selected: HashSet<_> = remove.iter().copied().collect();
    if selected.len() != remove.len()
        || selected
            .iter()
            .any(|id| blocks.get(*id).is_none_or(|block| !block.removable))
    {
        return Err(invalid_source("正文清洗标记无效或涉及受保护内容"));
    }
    let mut output = String::with_capacity(body.len());
    let mut offset = 0;
    for block in blocks.iter().filter(|block| selected.contains(&block.id)) {
        output.push_str(&body[offset..block.range.start]);
        offset = block.range.end;
    }
    output.push_str(&body[offset..]);
    let output = output.trim().to_owned();
    let retained = output.chars().count();
    // Large edits indicate uncertain boundaries, not a safe removal of residual widgets.
    if retained < 600 || retained * 5 < body.chars().count() * 4 {
        return Err(invalid_source("正文清洗删减过多，无法确认全文完整性"));
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removes_only_selected_blocks_without_rewriting_the_article() {
        let prose =
            "Sharing and subscriptions are the subject of this detailed experiment. ".repeat(20);
        let body = format!(
            "## Findings\n\n{prose}\n\nSubscribe to our newsletter.\n\n```rust\nshare(data);\n```\n\n![Result](reads-image:0)\n\nFigure 1: the result.\n\n> A quotation.\n\n| Input | Output |\n| --- | --- |\n| 1 | 2 |\n\n## References\n\n- [Source](https://example.com/paper)\n"
        );
        let blocks = blocks(&body);
        let subscription = blocks
            .iter()
            .find(|b| b.text.starts_with("Subscribe"))
            .unwrap();
        let cleaned = clean(&body, &[subscription.id]).unwrap();
        assert_eq!(cleaned, body.replacen(subscription.text, "", 1).trim());
        assert!(cleaned.contains(&prose));
        for block in blocks.iter().filter(|b| !b.removable) {
            assert!(clean(&body, &[block.id]).is_err(), "{}", block.text);
        }
        assert!(clean(&body, &[subscription.id, subscription.id]).is_err());
        assert!(clean(&body, &[blocks.len()]).is_err());
        assert!(clean(&body, &[1]).is_err());
        assert_eq!(clean(&body, &[]).unwrap(), body);
    }
}
