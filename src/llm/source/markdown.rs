//! Markdown documentation as the model reads pages: headings and paragraphs as lines and a
//! table row as `| cell | cell |`. Sites that draw their pages in the browser publish the
//! same text as Markdown, where a table may also be a component with `columns` and `rows`.
use super::{Link, Writer};
use url::Url;

/// HTML that documentation Markdown embeds; its tags are dropped and its text kept.
const TAGS: [&str; 22] = [
    "a", "b", "br", "code", "div", "em", "font", "i", "img", "li", "mark", "ol", "p", "small",
    "span", "strong", "sub", "sup", "table", "td", "tr", "u",
];
/// Struck-through text, such as the old price beside a discount, is dropped with its tags.
const STRUCK: [&str; 3] = ["del", "s", "strike"];

pub(crate) fn read(
    text: &str,
    base: &Url,
    domains: &[String],
    limit: usize,
) -> (Vec<String>, Vec<Link>) {
    let mut writer = Writer::new(base, domains, limit);
    let mut lines = text.lines();
    let mut fenced = false;
    while let Some(line) = lines.next() {
        if writer.full() {
            break;
        }
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            fenced = !fenced;
            continue;
        }
        if fenced
            || trimmed.is_empty()
            || trimmed.starts_with("<!--")
            || trimmed.starts_with("import ")
        {
            continue;
        }
        // Component definitions are code, not page text.
        if trimmed.starts_with("export ") {
            for next in lines.by_ref() {
                if next.trim() == "};" {
                    break;
                }
            }
            continue;
        }
        if trimmed.starts_with('<') && trimmed[1..].starts_with(|c: char| c.is_ascii_uppercase()) {
            let mut tag = trimmed.to_owned();
            while !closed(&tag) {
                let Some(next) = lines.next() else {
                    break;
                };
                tag.push('\n');
                tag.push_str(next);
            }
            for row in component_table(&tag, &mut writer) {
                writer.push(&row);
            }
            continue;
        }
        if trimmed.starts_with("</") && trimmed[2..].starts_with(|c: char| c.is_ascii_uppercase()) {
            continue;
        }
        let body = trimmed.trim_start_matches(['>', ' ']);
        if body.starts_with('|') {
            if let Some(row) = table_row(body, &mut writer) {
                writer.push(&row);
            }
            continue;
        }
        let heading = body.len() - body.trim_start_matches('#').len();
        let line = if heading > 0 && body[heading..].starts_with(' ') {
            format!(
                "{} {}",
                "#".repeat(heading),
                inline(&body[heading..], &mut writer)
            )
        } else if let Some(item) = ["* ", "- ", "+ "]
            .iter()
            .find_map(|marker| body.strip_prefix(marker))
        {
            format!("- {}", inline(item, &mut writer))
        } else {
            inline(body, &mut writer)
        };
        writer.push(&line);
    }
    writer.flush();
    (writer.lines, writer.links)
}

/// Whether a component's opening tag has ended: its attribute braces are balanced and it
/// closes with `>`.
fn closed(tag: &str) -> bool {
    let mut depth = 0i32;
    let mut quote = None;
    let mut escaped = false;
    for c in tag.chars() {
        match quote {
            Some(_) if escaped => escaped = false,
            Some(_) if c == '\\' => escaped = true,
            Some(open) if c == open => quote = None,
            Some(_) => {}
            None => match c {
                '"' | '\'' | '`' => quote = Some(c),
                '{' => depth += 1,
                '}' => depth -= 1,
                _ => {}
            },
        }
    }
    depth <= 0 && quote.is_none() && tag.trim_end().ends_with('>')
}

/// A table component's header and rows: `columns={[{ title: "…" }]}` and
/// `rows={[["…", "…"]]}`. Other components carry no table and are dropped.
fn component_table(tag: &str, writer: &mut Writer<'_>) -> Vec<String> {
    let (Some(columns), Some(rows)) = (tag.find("columns={"), tag.find("rows={")) else {
        return Vec::new();
    };
    let titles: Vec<String> = strings(&tag[columns..rows.max(columns)], Some("title"))
        .into_iter()
        .map(|title| inline(&title, writer))
        .collect();
    let mut table = Vec::new();
    if !titles.is_empty() {
        table.push(format!("| {} |", titles.join(" | ")));
    }
    let mut depth = 0;
    let mut row = String::new();
    let mut quote = None;
    let mut escaped = false;
    for c in tag[rows..].chars() {
        if let Some(open) = quote {
            row.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == open {
                quote = None;
            }
            continue;
        }
        match c {
            '"' | '\'' | '`' if depth >= 2 => {
                quote = Some(c);
                row.push(c);
            }
            '[' => {
                depth += 1;
                if depth == 2 {
                    row.clear();
                }
            }
            ']' => {
                if depth == 2 {
                    let cells: Vec<String> = strings(&row, None)
                        .into_iter()
                        .map(|cell| inline(&cell, writer))
                        .collect();
                    if cells.iter().any(|cell| !cell.is_empty()) {
                        table.push(format!("| {} |", cells.join(" | ")));
                    }
                }
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            _ if depth >= 2 => row.push(c),
            _ => {}
        }
    }
    table
}

/// The string literals in a piece of component code, or only those given as `key: "…"`.
fn strings(code: &str, key: Option<&str>) -> Vec<String> {
    let mut found = Vec::new();
    let chars: Vec<char> = code.chars().collect();
    let mut index = 0;
    while index < chars.len() {
        let open = chars[index];
        if !matches!(open, '"' | '\'' | '`') {
            index += 1;
            continue;
        }
        let before: String = chars[..index].iter().collect();
        let keyed = key.is_none_or(|key| {
            before
                .trim_end()
                .strip_suffix(':')
                .is_some_and(|name| name.trim_end().ends_with(key))
        });
        let mut value = String::new();
        index += 1;
        while index < chars.len() && chars[index] != open {
            if chars[index] == '\\' && index + 1 < chars.len() {
                index += 1;
            }
            value.push(chars[index]);
            index += 1;
        }
        index += 1;
        if keyed {
            found.push(value);
        }
    }
    found
}

/// A Markdown table row; the separator row gives nothing. Rows continuing a cell spanned
/// from above leave it empty, and like HTML tables they start at their first cell.
fn table_row(line: &str, writer: &mut Writer<'_>) -> Option<String> {
    let mut cells = vec![String::new()];
    let mut escaped = false;
    for c in line.chars() {
        let cell = cells.last_mut().expect("a cell");
        if escaped {
            if c != '|' {
                cell.push('\\');
            }
            cell.push(c);
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else if c == '|' {
            cells.push(String::new());
        } else {
            cell.push(c);
        }
    }
    // The leading and trailing pipes leave empty ends.
    if cells.first().is_some_and(|cell| cell.trim().is_empty()) {
        cells.remove(0);
    }
    if cells.last().is_some_and(|cell| cell.trim().is_empty()) {
        cells.pop();
    }
    let separator = cells
        .iter()
        .all(|cell| cell.trim().chars().all(|c| matches!(c, '-' | ':')));
    if separator {
        return None;
    }
    let cells: Vec<String> = cells.iter().map(|cell| inline(cell, writer)).collect();
    let first = cells.iter().position(|cell| !cell.is_empty())?;
    Some(format!("| {} |", cells[first..].join(" | ")))
}

/// Inline Markdown as plain text: escapes resolved, links as their text, emphasis and
/// embedded HTML dropped, struck-through text left out.
fn inline(text: &str, writer: &mut Writer<'_>) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::new();
    let mut index = 0;
    while index < chars.len() {
        let c = chars[index];
        let next = chars.get(index + 1).copied();
        match c {
            '\\' if next.is_some_and(|next| next.is_ascii_punctuation()) => {
                out.extend(next);
                index += 2;
            }
            '<' => match tag(&chars, index) {
                Some((name, closing, end)) if STRUCK.contains(&name.as_str()) && !closing => {
                    index = struck_end(&chars, end, &name);
                }
                Some((_, _, end)) => {
                    out.push(' ');
                    index = end;
                }
                None => {
                    out.push('<');
                    index += 1;
                }
            },
            '~' if next == Some('~') => {
                let rest: String = chars[index + 2..].iter().collect();
                index = match rest.find("~~") {
                    Some(end) => index + 2 + rest[..end].chars().count() + 2,
                    None => index + 2,
                };
            }
            '!' if next == Some('[') => match link(&chars, index + 1) {
                Some((_, _, end)) => index = end,
                None => {
                    out.push('!');
                    index += 1;
                }
            },
            '[' => match link(&chars, index) {
                Some((label, href, end)) => {
                    let label = inline(&label, writer);
                    writer.add_link(&label, &href);
                    out.push_str(&label);
                    index = end;
                }
                None => {
                    out.push('[');
                    index += 1;
                }
            },
            '*' if next == Some('*') => index += 2,
            '`' => index += 1,
            '&' => {
                let rest: String = chars[index..chars.len().min(index + 8)].iter().collect();
                let entity = [
                    ("&nbsp;", " "),
                    ("&lt;", "<"),
                    ("&gt;", ">"),
                    ("&amp;", "&"),
                    ("&quot;", "\""),
                ]
                .into_iter()
                .find(|(name, _)| rest.starts_with(name));
                match entity {
                    Some((name, value)) => {
                        out.push_str(value);
                        index += name.chars().count();
                    }
                    None => {
                        out.push('&');
                        index += 1;
                    }
                }
            }
            _ => {
                out.push(c);
                index += 1;
            }
        }
    }
    super::collapse(&out)
}

/// An embedded HTML tag at `start`: its name, whether it closes, and where it ends.
fn tag(chars: &[char], start: usize) -> Option<(String, bool, usize)> {
    let mut index = start + 1;
    let closing = chars.get(index) == Some(&'/');
    if closing {
        index += 1;
    }
    let name: String = chars[index..]
        .iter()
        .take_while(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_lowercase();
    if !TAGS.contains(&name.as_str()) && !STRUCK.contains(&name.as_str()) {
        return None;
    }
    let after = chars.get(index + name.len())?;
    if !matches!(after, ' ' | '>' | '/') {
        return None;
    }
    let end = chars[index..].iter().position(|c| *c == '>')? + index + 1;
    Some((name, closing, end))
}

/// Past the tag closing a struck-through span.
fn struck_end(chars: &[char], from: usize, name: &str) -> usize {
    let mut index = from;
    while index < chars.len() {
        if chars[index] == '<'
            && let Some((found, true, end)) = tag(chars, index)
            && found == name
        {
            return end;
        }
        index += 1;
    }
    chars.len()
}

/// A Markdown link at `start` (its `[`): label, address and where it ends.
fn link(chars: &[char], start: usize) -> Option<(String, String, usize)> {
    let mut depth = 0;
    let mut index = start;
    let close = loop {
        match chars.get(index)? {
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    break index;
                }
            }
            _ => {}
        }
        index += 1;
    };
    if chars.get(close + 1) != Some(&'(') {
        return None;
    }
    let end = chars[close + 2..].iter().position(|c| *c == ')')? + close + 2;
    let label: String = chars[start + 1..close].iter().collect();
    let target: String = chars[close + 2..end].iter().collect();
    let href = target.split_whitespace().next().unwrap_or("").to_owned();
    Some((label, href, end + 1))
}
