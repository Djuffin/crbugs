use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Complete, self-contained representation of a single Chromium issue.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IssueBundle {
    pub issue_id: i64,
    pub url: String,
    pub title: String,
    pub issue_type: String,
    pub status: String,
    pub priority: String,
    pub severity: String,
    pub component_id: i64,
    pub component_path: Vec<String>,
    pub reporter: Option<String>,
    pub assignee: Option<String>,
    pub verifier: Option<String>,
    pub ccs: Vec<String>,
    pub collaborators: Vec<String>,
    pub created_time: Option<DateTime<Utc>>,
    pub modified_time: Option<DateTime<Utc>>,
    pub resolved_time: Option<DateTime<Utc>>,
    pub verified_time: Option<DateTime<Utc>>,
    pub vote_count: i64,
    pub version: Option<i64>,
    pub hotlist_ids: Vec<i64>,
    pub blocked_by_ids: Vec<i64>,
    pub blocking_ids: Vec<i64>,
    pub duplicate_issue_ids: Vec<i64>,
    pub canonical_issue_id: Option<i64>,
    pub parent_issue_ids: Vec<i64>,
    pub found_in_versions: Vec<String>,
    pub targeted_to_versions: Vec<String>,
    pub verified_in_versions: Vec<String>,
    pub in_prod: bool,
    pub is_archived: bool,
    pub access_level: String,
    pub custom_fields: Vec<ResolvedCustomField>,
    pub description: Option<CommentEntry>,
    pub comments: Vec<CommentEntry>,
    pub attachments: Vec<AttachmentMeta>,
}

/// Custom field definition joined with its current value on the issue.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResolvedCustomField {
    pub id: i64,
    pub name: String,
    pub field_type: String,
    pub value: String,
}

/// A chronological entry in the issue timeline (either the initial description,
/// a numbered comment, an attachment upload group, or a field-update event).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommentEntry {
    /// 1 for the initial description, 2..=N for subsequent comments, or None for
    /// standalone attachment/field updates.
    pub comment_number: Option<i32>,
    pub author: String,
    pub timestamp: Option<DateTime<Utc>>,
    pub modified_time: Option<DateTime<Utc>>,
    pub body: String,
    pub formatting_mode: FormattingMode,
    pub redacted: bool,
    pub version: Option<i64>,
    pub attachments: Vec<AttachmentMeta>,
    pub field_updates: Vec<FieldDiff>,
}

/// Map from custom field ID to `(name, field_type)`.
pub type CustomFieldDefMap = std::collections::HashMap<i64, (String, String)>;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum FormattingMode {
    #[default]
    Plain,
    Markdown,
    Literal,
}

/// Metadata and download state for an issue attachment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachmentMeta {
    pub attachment_id: i64,
    pub issue_id: i64,
    pub comment_number: Option<i32>,
    pub filename: String,
    pub sanitized_filename: String,
    pub content_type: String,
    pub size_bytes: u64,
    pub download_url: String,
    pub is_deleted: bool,
    /// Path relative to the Markdown file (for links) when downloaded.
    pub relative_path: Option<String>,
    /// Local filesystem path where the file was written.
    pub local_path: Option<PathBuf>,
    pub download_status: AttachmentDownloadStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", content = "detail", rename_all = "snake_case")]
pub enum AttachmentDownloadStatus {
    Downloaded,
    Skipped,
    SkippedTooLarge,
    DeletedOnServer,
    Failed(String),
}

/// A human-readable summary of a single field change within an `IssueUpdate`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FieldDiff {
    pub field: String,
    pub summary: String,
}

/// Paginated or bounded search results from `POST /action/issues/list`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchIssuesResult {
    pub query: String,
    pub sort_by: String,
    pub total_size: usize,
    pub total_size_accurate: bool,
    pub next_page_token: Option<String>,
    pub issues: Vec<IssueBundle>,
}

