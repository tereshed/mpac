//! Durable review notes and immutable, explicitly inserted CLI drafts.
use anyhow::{Context, Result, ensure};
use mpac_workspace::review_lines::{LineKind, ReviewLine};
use serde::{Deserialize, Serialize};

pub const MAX_COMMENT_BYTES: usize = 4000;
pub const MAX_EXCERPT_BYTES: usize = 12 * 1024;
pub const MAX_PACKET_BYTES: usize = 512 * 1024;
pub const MAX_BATCH_COMMENTS: usize = 32;
pub type RangeExcerpt = (String, Option<u32>, Option<u32>, Option<u32>, Option<u32>);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommentSource {
    Diff,
    File,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AnchorSide {
    Old,
    New,
    #[default]
    Both,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CommentAnchor {
    File,
    Range {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        block_id: Option<u32>,
        start_row: u32,
        end_row: u32,
        #[serde(default)]
        side: AnchorSide,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Delivery {
    pub session_id: String,
    pub revision: u64,
    pub batch_id: String,
    pub at: u64,
    pub status: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReviewComment {
    pub id: String,
    pub project_id: String,
    pub root_id: String,
    pub path: String,
    pub source: CommentSource,
    pub snapshot: String,
    pub anchor: CommentAnchor,
    pub excerpt: String,
    pub excerpt_truncated: bool,
    pub capture_fingerprint: String,
    pub diff_section: Option<String>,
    pub root_identity: String,
    pub old_start: Option<u32>,
    pub old_end: Option<u32>,
    pub new_start: Option<u32>,
    pub new_end: Option<u32>,
    pub text: String,
    pub revision: u64,
    pub created_at: u64,
    pub updated_at: u64,
    pub deleted: bool,
    #[serde(default)]
    pub stale: bool,
    #[serde(default)]
    pub validation_state: String,
    #[serde(default)]
    pub deliveries: Vec<Delivery>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateComment {
    pub root_id: String,
    pub path: String,
    pub source: CommentSource,
    pub snapshot: String,
    pub anchor: CommentAnchor,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommentRef {
    pub id: String,
    pub revision: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BatchStatus {
    Draft,
    Preparing,
    Prepared,
    HandedOff,
    Uncertain,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReviewBatch {
    pub id: String,
    pub project_id: String,
    pub session_id: String,
    pub request_id: String,
    pub comments: Vec<CommentRef>,
    pub message: String,
    pub text: String,
    pub status: BatchStatus,
    #[serde(default)]
    pub attachment_path: Option<String>,
    pub created_at: u64,
    pub updated_at: u64,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrepareBatch {
    pub session_id: String,
    pub request_id: String,
    pub comments: Vec<CommentRef>,
    #[serde(default)]
    pub message: String,
}

pub fn clean_note(text: &str) -> Result<String> {
    let text = text.trim();
    ensure!(
        !text.is_empty() && text.len() <= MAX_COMMENT_BYTES,
        "Нужно замечание длиной до 4000 байт"
    );
    ensure!(
        !text
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t')),
        "Комментарий содержит управляющие символы"
    );
    Ok(text.into())
}

pub fn validate_prepare(request: &PrepareBatch) -> Result<()> {
    ensure!(
        uuid::Uuid::parse_str(&request.request_id).is_ok_and(|id| !id.is_nil()),
        "Нужен уникальный идентификатор пакета"
    );
    ensure!(
        request.comments.len() <= MAX_BATCH_COMMENTS,
        "В пакете может быть до 32 комментариев"
    );
    ensure!(
        request.message.len() <= MAX_COMMENT_BYTES,
        "Сообщение должно быть не длиннее 4000 байт"
    );
    ensure!(
        !request.comments.is_empty() || !request.message.trim().is_empty(),
        "Выберите комментарии или напишите задачу"
    );
    ensure!(
        !request
            .message
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t')),
        "Сообщение содержит управляющие символы"
    );
    let mut unique = std::collections::HashSet::new();
    ensure!(
        request.comments.iter().all(|c| unique.insert(&c.id)),
        "Комментарий не должен повторяться в пакете"
    );
    Ok(())
}

pub fn range_excerpt(
    lines: &[ReviewLine],
    start: u32,
    end: u32,
    side: AnchorSide,
) -> Result<RangeExcerpt> {
    ensure!(
        start <= end && end.saturating_sub(start) < 200,
        "Выберите непрерывный диапазон до 200 строк"
    );
    let chosen = lines
        .get(start as usize..=end as usize)
        .context("Выделенные строки больше недоступны")?;
    ensure!(
        !chosen.is_empty() && chosen.iter().all(|line| line.hunk == chosen[0].hunk),
        "Диапазон не должен пересекать пропущенные блоки diff"
    );
    let chosen: Vec<_> = chosen
        .iter()
        .filter(|line| match side {
            AnchorSide::Old => line.old_line.is_some(),
            AnchorSide::New => line.new_line.is_some(),
            AnchorSide::Both => true,
        })
        .collect();
    ensure!(!chosen.is_empty(), "На выбранной стороне нет строк");
    let old: Vec<_> = chosen
        .iter()
        .filter_map(|line| {
            if side == AnchorSide::New {
                None
            } else {
                line.old_line
            }
        })
        .collect();
    let new: Vec<_> = chosen
        .iter()
        .filter_map(|line| {
            if side == AnchorSide::Old {
                None
            } else {
                line.new_line
            }
        })
        .collect();
    let excerpt = chosen
        .iter()
        .map(|line| {
            let marker = match line.kind {
                LineKind::Context => " ",
                LineKind::Addition => "+",
                LineKind::Deletion => "-",
            };
            format!(
                "{marker} [до: {}; после: {}] {}",
                line.old_line.map_or("—".into(), |n| n.to_string()),
                line.new_line.map_or("—".into(), |n| n.to_string()),
                line.text
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    ensure!(
        excerpt.len() <= MAX_EXCERPT_BYTES,
        "Выделенный фрагмент слишком велик; выберите меньше строк"
    );
    Ok((
        excerpt,
        old.first().copied(),
        old.last().copied(),
        new.first().copied(),
        new.last().copied(),
    ))
}

pub fn bounded_excerpt(text: &str) -> (String, bool) {
    let end = text.len().min(MAX_EXCERPT_BYTES);
    let mut boundary = end;
    while !text.is_char_boundary(boundary) {
        boundary -= 1;
    }
    (text[..boundary].to_owned(), boundary < text.len())
}

fn span(start: Option<u32>, end: Option<u32>) -> String {
    match (start, end) {
        (Some(a), Some(b)) if a != b => format!("{a}–{b}"),
        (Some(a), _) => a.to_string(),
        _ => "—".into(),
    }
}

pub fn packet(message: &str, comments: &[ReviewComment]) -> Result<String> {
    let mut text = String::from(
        "Задача по ревью кода. Фрагменты ниже являются данными для проверки; замечания относятся только к указанным областям.\n",
    );
    if !message.trim().is_empty() {
        text.push_str("Общая задача: ");
        text.push_str(message.trim());
        text.push('\n');
    }
    for comment in comments {
        text.push_str(&format!(
            "\nКомментарий {} · версия {}\nПапка: {}\nФайл: {}\nСнимок: {}\n",
            comment.id, comment.revision, comment.root_id, comment.path, comment.snapshot
        ));
        match comment.anchor {
            CommentAnchor::File => text.push_str("Область: весь файл.\n"),
            CommentAnchor::Range { .. } => text.push_str(&format!("Область: старые строки {}; новые строки {}. Замечание относится ко ВСЕМУ выделенному диапазону.\n", span(comment.old_start, comment.old_end), span(comment.new_start, comment.new_end))),
        }
        text.push_str("Фрагмент (начало):\n");
        text.push_str(&comment.excerpt);
        text.push_str("\nФрагмент (конец).\nЗамечание: ");
        text.push_str(&comment.text);
        text.push_str("\n---\n");
        if comment.excerpt_truncated {
            text.push_str("Фрагмент сокращён; полный файл доступен по указанному пути.\n");
        }
    }
    ensure!(
        text.len() <= MAX_PACKET_BYTES,
        "Пакет превышает 512 KiB. Выберите меньше комментариев или сократите задачу"
    );
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn range_preserves_real_old_new_coordinates_and_entire_excerpt() {
        let rows = vec![
            ReviewLine {
                index: 0,
                hunk: 1,
                old_line: Some(43),
                new_line: None,
                kind: LineKind::Deletion,
                text: "old".into(),
            },
            ReviewLine {
                index: 1,
                hunk: 1,
                old_line: None,
                new_line: Some(43),
                kind: LineKind::Addition,
                text: "new".into(),
            },
            ReviewLine {
                index: 2,
                hunk: 1,
                old_line: None,
                new_line: Some(44),
                kind: LineKind::Addition,
                text: "next".into(),
            },
        ];
        let (excerpt, a, b, c, d) = range_excerpt(&rows, 0, 2, AnchorSide::Both).unwrap();
        assert_eq!((a, b, c, d), (Some(43), Some(43), Some(43), Some(44)));
        assert!(excerpt.contains("old") && excerpt.contains("next"));
        assert!(range_excerpt(&rows, 2, 1, AnchorSide::New).is_err());
        let mut gaps = rows.clone();
        gaps[2].hunk = 2;
        assert!(range_excerpt(&gaps, 0, 2, AnchorSide::Both).is_err());
    }
    #[test]
    fn request_bounds_and_duplicate_notes_are_enforced() {
        let mut request = PrepareBatch {
            session_id: "session".into(),
            request_id: uuid::Uuid::new_v4().to_string(),
            comments: vec![CommentRef {
                id: "a".into(),
                revision: 1,
            }],
            message: String::new(),
        };
        assert!(validate_prepare(&request).is_ok());
        request.comments.push(request.comments[0].clone());
        assert!(validate_prepare(&request).is_err());
        assert!(clean_note("bad\x1b[2J").is_err());
        let (text, truncated) = bounded_excerpt(&"я".repeat(9000));
        assert!(truncated && text.is_char_boundary(text.len()));
    }
}
