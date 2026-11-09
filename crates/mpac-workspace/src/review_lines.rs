//! Stable, bounded line coordinates for a particular captured diff/source.
use std::{collections::HashSet, path::Path};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::Review;

const MAX_BLOCK_LINES: usize = 4096;
const MAX_LINES: usize = 20_000;

pub fn review_text_fingerprint(text: &str) -> String {
    let mut hash = Sha256::new();
    hash.update(b"mpac-review-fragment-v1\0");
    hash.update(text.as_bytes());
    format!("{:x}", hash.finalize())
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum LineKind {
    Context,
    Addition,
    Deletion,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReviewLine {
    pub index: u32,
    pub hunk: u32,
    pub old_line: Option<u32>,
    pub new_line: Option<u32>,
    pub kind: LineKind,
    pub text: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReviewBlock {
    pub id: u32,
    pub path: String,
    pub section: String,
    pub lines: Vec<ReviewLine>,
    pub lines_truncated: bool,
    pub raw: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReviewLines {
    pub snapshot: String,
    pub head: String,
    pub truncated: bool,
    pub blocks: Vec<ReviewBlock>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SourceLines {
    pub snapshot: String,
    pub path: String,
    pub lines: Vec<ReviewLine>,
    pub lines_truncated: bool,
}

fn unquote(input: &str) -> Option<String> {
    if !input.starts_with('"') {
        return (!input.contains(['\t', '\r', '\n', '"'])).then(|| input.to_owned());
    }
    if let Ok(decoded) = serde_json::from_str::<String>(input) {
        return Some(decoded);
    }
    let inside = input.strip_prefix('"')?.strip_suffix('"')?;
    let mut bytes = Vec::new();
    let mut cursor = inside.as_bytes().iter().copied().peekable();
    while let Some(byte) = cursor.next() {
        if byte != b'\\' {
            if byte == b'"' || byte < 32 {
                return None;
            }
            bytes.push(byte);
            continue;
        }
        let byte = cursor.next()?;
        if (b'0'..=b'7').contains(&byte) {
            let mut octal = u16::from(byte - b'0');
            for _ in 0..2 {
                if let Some(next) = cursor.peek().copied()
                    && (b'0'..=b'7').contains(&next)
                {
                    cursor.next();
                    octal = octal * 8 + u16::from(next - b'0');
                }
            }
            bytes.push(u8::try_from(octal).ok()?);
        } else {
            bytes.push(match byte {
                b'a' => 7,
                b'b' => 8,
                b't' => 9,
                b'n' => 10,
                b'v' => 11,
                b'f' => 12,
                b'r' => 13,
                b'"' => b'"',
                b'\\' => b'\\',
                _ => return None,
            });
        }
    }
    String::from_utf8(bytes).ok()
}

fn header_value(input: &str) -> Option<Option<String>> {
    let input = if !input.starts_with('"') {
        input.trim_end_matches('\t')
    } else {
        input
    };
    let decoded = unquote(input)?;
    if decoded == "/dev/null" {
        return Some(None);
    }
    let path = decoded
        .strip_prefix("a/")
        .or_else(|| decoded.strip_prefix("b/"))?;
    Some(Some(path.to_owned()))
}

fn header_pairs(header: &str) -> Vec<(String, String)> {
    let Some(body) = header.strip_prefix("diff --git ") else {
        return vec![];
    };
    let mut found = Vec::new();
    for (index, _) in body.match_indices(' ').take(256) {
        let Some(Some(first)) = header_value(&body[..index]) else {
            continue;
        };
        let Some(Some(second)) = header_value(&body[index + 1..]) else {
            continue;
        };
        found.push((first, second));
    }
    found
}

fn metadata_path(raw: &str, known: &HashSet<&str>) -> Option<String> {
    let mut before = None;
    let mut after = None;
    let mut saw_before = false;
    let mut saw_after = false;
    for line in raw.lines().skip(1).take_while(|line| {
        !line.starts_with("@@ ")
            && !line.starts_with("GIT binary patch")
            && !line.starts_with("Binary file")
    }) {
        if let Some(header) = line.strip_prefix("--- ") {
            if saw_before {
                return None;
            }
            saw_before = true;
            before = header_value(header)?;
        }
        if let Some(header) = line.strip_prefix("+++ ") {
            if saw_after {
                return None;
            }
            saw_after = true;
            after = header_value(header)?;
        }
    }
    let mut paths = HashSet::new();
    for (old, new) in header_pairs(raw.lines().next()?) {
        if saw_before && before.as_ref().is_some_and(|path| path != &old) {
            continue;
        }
        if saw_after && after.as_ref().is_some_and(|path| path != &new) {
            continue;
        }
        let path = if saw_after {
            after.as_ref().or(before.as_ref())
        } else {
            Some(&new)
        };
        if let Some(path) = path
            && known.contains(path.as_str())
        {
            paths.insert(path.clone());
        }
    }
    (paths.len() == 1)
        .then(|| paths.into_iter().next())
        .flatten()
}

fn range(part: &str) -> Option<(u32, u32)> {
    let (start, count) = match part.split_once(',') {
        Some((a, b)) => (a.parse().ok()?, b.parse().ok()?),
        None => (part.parse().ok()?, 1),
    };
    (start <= 10_000_000_u32 && count <= 10_000_000_u32).then_some((start, count))
}

fn hunk(line: &str) -> Option<(u32, u32, u32, u32)> {
    let (before, after) = line.strip_prefix("@@ -")?.split_once(" +")?;
    let (after, _) = after.split_once(" @@")?;
    let (old, old_count) = range(before)?;
    let (new, new_count) = range(after)?;
    Some((old, old_count, new, new_count))
}

/// Parse coordinates only for fully understood blocks. An unknown or malformed
/// patch remains available in the raw review but cannot anchor a range comment.
pub fn structured_review(review: &Review) -> ReviewLines {
    let known: HashSet<_> = review.files.iter().map(|f| f.path.as_str()).collect();
    let mut blocks = Vec::new();
    let mut section = "working";
    let mut raw = String::new();
    let mut current_section = String::new();
    let mut total = 0;
    for line in review.diff.split_inclusive('\n') {
        if line.starts_with("# Staged changes") {
            section = "staged";
        } else if line.starts_with("# Working tree changes") {
            section = "working";
        }
        if line.starts_with("diff --git ") {
            if !raw.is_empty() {
                if let Some(block) = parse_block(
                    &raw,
                    &current_section,
                    review,
                    &known,
                    blocks.len() as u32,
                    &mut total,
                ) {
                    blocks.push(block);
                }
                raw.clear();
            }
            current_section = section.into();
        }
        if !raw.is_empty() || line.starts_with("diff --git ") {
            // Section banners do not belong to the previous patch.
            if !line.starts_with("# Staged changes") && !line.starts_with("# Working tree changes")
            {
                raw.push_str(line);
            }
        }
    }
    if !raw.is_empty()
        && let Some(block) = parse_block(
            &raw,
            &current_section,
            review,
            &known,
            blocks.len() as u32,
            &mut total,
        )
    {
        blocks.push(block);
    }
    ReviewLines {
        snapshot: review.snapshot.clone(),
        head: review.head.clone(),
        truncated: review.truncated,
        blocks,
    }
}

fn parse_block(
    raw: &str,
    section: &str,
    review: &Review,
    known: &HashSet<&str>,
    id: u32,
    total: &mut usize,
) -> Option<ReviewBlock> {
    let path = metadata_path(raw, known)?;
    let section = if review
        .files
        .iter()
        .any(|f| f.path == path && f.status == "??")
    {
        "untracked"
    } else {
        section
    };
    let mut lines = Vec::new();
    let (mut old, mut new, mut old_left, mut new_left) = (0_u32, 0_u32, 0_u32, 0_u32);
    let mut hunk_id = 0_u32;
    let mut active = false;
    let mut malformed = !raw.ends_with('\n');
    let mut truncated = false;
    for line in raw.lines().skip(1) {
        if let Some((a, ac, b, bc)) = hunk(line) {
            if old_left != 0 || new_left != 0 {
                malformed = true;
            }
            (old, old_left, new, new_left) = (a, ac, b, bc);
            hunk_id += 1;
            active = true;
            continue;
        }
        if line.starts_with("\\ No newline at end of file") {
            continue;
        }
        if !active {
            continue;
        }
        if old_left == 0 && new_left == 0 {
            if !line.is_empty() {
                malformed = true;
            }
            continue;
        }
        let (kind, old_line, new_line) = match line.as_bytes().first() {
            Some(b' ') if old_left > 0 && new_left > 0 => {
                old_left -= 1;
                new_left -= 1;
                let pair = (LineKind::Context, Some(old), Some(new));
                old += 1;
                new += 1;
                pair
            }
            Some(b'-') if old_left > 0 => {
                old_left -= 1;
                let pair = (LineKind::Deletion, Some(old), None);
                old += 1;
                pair
            }
            Some(b'+') if new_left > 0 => {
                new_left -= 1;
                let pair = (LineKind::Addition, None, Some(new));
                new += 1;
                pair
            }
            _ => {
                malformed = true;
                continue;
            }
        };
        if lines.len() < MAX_BLOCK_LINES && *total < MAX_LINES {
            lines.push(ReviewLine {
                index: lines.len() as u32,
                hunk: hunk_id,
                old_line,
                new_line,
                kind,
                text: line[1..].to_owned(),
            });
            *total += 1;
        } else {
            truncated = true;
        }
    }
    malformed |= old_left != 0 || new_left != 0;
    if malformed {
        *total = total.saturating_sub(lines.len());
        lines.clear();
    }
    Some(ReviewBlock {
        id,
        path,
        section: section.into(),
        lines,
        lines_truncated: truncated || malformed,
        raw: format!("{}\n", raw.trim_end_matches('\n')),
    })
}

pub fn source_lines(root: &Path, path: &str) -> Result<SourceLines> {
    ensure!(!path.is_empty(), "Нужно выбрать файл");
    let text = crate::read_text(root, path)?;
    let again = crate::read_text(root, path).context("Файл изменился во время чтения")?;
    ensure!(text == again, "Файл изменился во время чтения");
    let mut hash = Sha256::new();
    hash.update(b"mpac-source-v1\0");
    hash.update(root.as_os_str().as_encoded_bytes());
    hash.update([0]);
    hash.update(path.as_bytes());
    hash.update([0]);
    hash.update(text.as_bytes());
    let count = text.lines().count();
    let lines = text
        .lines()
        .take(MAX_LINES)
        .enumerate()
        .map(|(i, text)| ReviewLine {
            index: i as u32,
            hunk: 0,
            old_line: None,
            new_line: Some(i as u32 + 1),
            kind: LineKind::Context,
            text: text.into(),
        })
        .collect();
    Ok(SourceLines {
        snapshot: format!("{:x}", hash.finalize()),
        path: path.into(),
        lines,
        lines_truncated: count > MAX_LINES,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ChangedFile;
    fn review(diff: &str, path: &str) -> Review {
        Review {
            head: "abc".into(),
            snapshot: "snap".into(),
            truncated: false,
            diff: diff.into(),
            files: vec![ChangedFile {
                path: path.into(),
                status: " M".into(),
            }],
        }
    }
    #[test]
    fn true_coordinates_and_separate_index_and_working_blocks() {
        let input = review(
            "# Staged changes\n\ndiff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -43,2 +43,3 @@\n-old\n+new\n+extra\n tail\n\n# Working tree changes\n\ndiff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -1 +1 @@\n-new\n+again\n",
            "a.rs",
        );
        let parsed = structured_review(&input);
        assert_eq!(parsed.blocks.len(), 2);
        assert_eq!(parsed.blocks[0].section, "staged");
        assert_eq!(parsed.blocks[1].section, "working");
        assert_eq!(parsed.blocks[0].lines[0].old_line, Some(43));
        assert_eq!(parsed.blocks[0].lines[1].new_line, Some(43));
        assert_eq!(parsed.blocks[0].lines[2].new_line, Some(44));
    }
    #[test]
    fn quoted_utf8_and_deleted_paths_are_not_guessed() {
        let input = review(
            "diff --git \"a/\\320\\261 x\" \"b/\\320\\261 x\"\n--- \"a/\\320\\261 x\"\n+++ /dev/null\n@@ -1 +0,0 @@\n-deleted\n",
            "б x",
        );
        let parsed = structured_review(&input);
        assert_eq!(parsed.blocks[0].path, "б x");
        assert_eq!(parsed.blocks[0].lines[0].new_line, None);
        assert!(
            structured_review(&review(
                "diff --git a/no b/no\n--- a/no\n+++ b/no\n@@ -1 +1 @@\n-a\n+b\n",
                "other"
            ))
            .blocks
            .is_empty()
        );
    }
    #[test]
    fn malformed_hunk_cannot_be_used_as_coordinates() {
        let parsed = structured_review(&review(
            "diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1,2 +1,2 @@\n-a\n+b\n",
            "a",
        ));
        assert!(parsed.blocks[0].lines.is_empty());
        assert!(parsed.blocks[0].lines_truncated);
    }

    #[test]
    fn source_header_like_lines_never_retarget_another_changed_file() {
        let mut input = review(
            "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -1 +1,2 @@\n-old\n+++ b/b.rs\n+--- a/b.rs\ndiff --git a/b.rs b/b.rs\n--- a/b.rs\n+++ b/b.rs\n@@ -1 +1 @@\n-before\n+after\n",
            "a.rs",
        );
        input.files.push(ChangedFile {
            path: "b.rs".into(),
            status: " M".into(),
        });
        let parsed = structured_review(&input);
        assert_eq!(parsed.blocks.len(), 2);
        assert_eq!(parsed.blocks[0].path, "a.rs");
        assert_eq!(parsed.blocks[0].lines[1].text, "++ b/b.rs");
        assert_eq!(parsed.blocks[1].path, "b.rs");
        let mismatch = structured_review(&review(
            "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/b.rs\n@@ -1 +1 @@\n-a\n+b\n",
            "b.rs",
        ));
        assert!(mismatch.blocks.is_empty());
    }
}
