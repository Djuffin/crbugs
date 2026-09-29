use std::fmt::Write;

use crate::attachments::format_byte_size;
use crate::models::{
    AttachmentDownloadStatus, AttachmentMeta, CommentEntry, FormattingMode, IssueBundle,
};

/// Renders an `IssueBundle` into a complete Markdown document.
pub fn render_issue_markdown(bundle: &IssueBundle) -> String {
    let mut out = String::with_capacity(8192);

    let component_display = if bundle.component_path.is_empty() {
        bundle.component_id.to_string()
    } else {
        bundle.component_path.join(" > ")
    };

    // 1. YAML Frontmatter
    let _ = writeln!(out, "---");
    let _ = writeln!(out, "issue_id: {}", bundle.issue_id);
    let _ = writeln!(out, "url: {}", yaml_quote(&bundle.url));
    let _ = writeln!(out, "title: {}", yaml_quote(&bundle.title));
    let _ = writeln!(out, "type: {}", bundle.issue_type);
    let _ = writeln!(out, "status: {}", bundle.status);
    let _ = writeln!(out, "priority: {}", bundle.priority);
    let _ = writeln!(out, "severity: {}", bundle.severity);
    let _ = writeln!(out, "component_id: {}", bundle.component_id);
    let _ = writeln!(out, "component: {}", yaml_quote(&component_display));
    if let Some(ref rep) = bundle.reporter {
        let _ = writeln!(out, "reporter: {}", yaml_quote(rep));
    } else {
        let _ = writeln!(out, "reporter: null");
    }
    if let Some(ref asg) = bundle.assignee {
        let _ = writeln!(out, "assignee: {}", yaml_quote(asg));
    } else {
        let _ = writeln!(out, "assignee: null");
    }
    if let Some(ref ver) = bundle.verifier {
        let _ = writeln!(out, "verifier: {}", yaml_quote(ver));
    }
    if let Some(t) = bundle.created_time {
        let _ = writeln!(out, "created_time: \"{}\"", t.to_rfc3339());
    }
    if let Some(t) = bundle.modified_time {
        let _ = writeln!(out, "modified_time: \"{}\"", t.to_rfc3339());
    }
    if let Some(t) = bundle.resolved_time {
        let _ = writeln!(out, "resolved_time: \"{}\"", t.to_rfc3339());
    }
    if let Some(t) = bundle.verified_time {
        let _ = writeln!(out, "verified_time: \"{}\"", t.to_rfc3339());
    }
    if !bundle.custom_fields.is_empty() {
        let _ = writeln!(out, "custom_fields:");
        for cf in &bundle.custom_fields {
            let _ = writeln!(out, "  {}: {}", yaml_key(&cf.name), yaml_quote(&cf.value));
        }
    }
    let _ = writeln!(out, "---\n");

    // 2. Title
    let _ = writeln!(
        out,
        "# [Issue {}]({}): {}\n",
        bundle.issue_id,
        bundle.url,
        escape_md_heading(&bundle.title)
    );

    // 3. Metadata Table
    let _ = writeln!(out, "## Metadata\n");
    let _ = writeln!(out, "| Field | Value |");
    let _ = writeln!(out, "| :--- | :--- |");
    let _ = writeln!(out, "| **Type** | `{}` |", bundle.issue_type);
    let _ = writeln!(out, "| **Status** | `{}` |", bundle.status);
    let _ = writeln!(
        out,
        "| **Priority / Severity** | `{}` / `{}` |",
        bundle.priority, bundle.severity
    );
    if bundle.component_path.is_empty() {
        let _ = writeln!(out, "| **Component** | `{}` |", bundle.component_id);
    } else {
        let _ = writeln!(
            out,
            "| **Component** | `{}` (`{}`) |",
            escape_table_cell(&component_display),
            bundle.component_id
        );
    }
    let _ = writeln!(
        out,
        "| **Reporter** | {} |",
        bundle
            .reporter
            .as_deref()
            .map(|s| format!("`{}`", escape_table_cell(s)))
            .unwrap_or_else(|| "*None*".to_string())
    );
    let _ = writeln!(
        out,
        "| **Assignee** | {} |",
        bundle
            .assignee
            .as_deref()
            .map(|s| format!("`{}`", escape_table_cell(s)))
            .unwrap_or_else(|| "*Unassigned*".to_string())
    );
    if let Some(ref ver) = bundle.verifier {
        let _ = writeln!(out, "| **Verifier** | `{}` |", escape_table_cell(ver));
    }
    if !bundle.ccs.is_empty() {
        let ccs_fmt = bundle
            .ccs
            .iter()
            .map(|c| format!("`{}`", escape_table_cell(c)))
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(out, "| **CC** | {} |", ccs_fmt);
    }
    if !bundle.collaborators.is_empty() {
        let collab_fmt = bundle
            .collaborators
            .iter()
            .map(|c| format!("`{}`", escape_table_cell(c)))
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(out, "| **Collaborators** | {} |", collab_fmt);
    }
    if let Some(t) = bundle.created_time {
        let _ = writeln!(
            out,
            "| **Created** | {} |",
            t.format("%Y-%m-%d %H:%M:%S UTC")
        );
    }
    if let Some(t) = bundle.modified_time {
        let _ = writeln!(
            out,
            "| **Modified** | {} |",
            t.format("%Y-%m-%d %H:%M:%S UTC")
        );
    }
    if let Some(t) = bundle.resolved_time {
        let _ = writeln!(
            out,
            "| **Resolved** | {} |",
            t.format("%Y-%m-%d %H:%M:%S UTC")
        );
    }
    if let Some(t) = bundle.verified_time {
        let _ = writeln!(
            out,
            "| **Verified** | {} |",
            t.format("%Y-%m-%d %H:%M:%S UTC")
        );
    }
    if bundle.vote_count > 0 {
        let _ = writeln!(out, "| **Votes** | `{}` |", bundle.vote_count);
    }
    if let Some(canon) = bundle.canonical_issue_id {
        let _ = writeln!(
            out,
            "| **Duplicate Of** | [{}](https://issues.chromium.org/issues/{}) |",
            canon, canon
        );
    }
    if !bundle.duplicate_issue_ids.is_empty() {
        let links = format_issue_links(&bundle.duplicate_issue_ids);
        let _ = writeln!(out, "| **Duplicates** | {} |", links);
    }
    if !bundle.blocked_by_ids.is_empty() {
        let links = format_issue_links(&bundle.blocked_by_ids);
        let _ = writeln!(out, "| **Blocked By** | {} |", links);
    }
    if !bundle.blocking_ids.is_empty() {
        let links = format_issue_links(&bundle.blocking_ids);
        let _ = writeln!(out, "| **Blocking** | {} |", links);
    }
    if !bundle.parent_issue_ids.is_empty() {
        let links = format_issue_links(&bundle.parent_issue_ids);
        let _ = writeln!(out, "| **Parent Issues** | {} |", links);
    }
    if !bundle.hotlist_ids.is_empty() {
        let hs = bundle
            .hotlist_ids
            .iter()
            .map(|id| format!("`{}`", id))
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(out, "| **Hotlists** | {} |", hs);
    }
    if !bundle.found_in_versions.is_empty() {
        let _ = writeln!(
            out,
            "| **Found In** | `{}` |",
            escape_table_cell(&bundle.found_in_versions.join(", "))
        );
    }
    if !bundle.targeted_to_versions.is_empty() {
        let _ = writeln!(
            out,
            "| **Targeted To** | `{}` |",
            escape_table_cell(&bundle.targeted_to_versions.join(", "))
        );
    }
    if !bundle.verified_in_versions.is_empty() {
        let _ = writeln!(
            out,
            "| **Verified In** | `{}` |",
            escape_table_cell(&bundle.verified_in_versions.join(", "))
        );
    }
    if bundle.in_prod {
        let _ = writeln!(out, "| **In Production** | `true` |");
    }
    if !bundle.access_level.is_empty() && bundle.access_level != "UNSPECIFIED" {
        let _ = writeln!(out, "| **Access Level** | `{}` |", bundle.access_level);
    }

    for cf in &bundle.custom_fields {
        let _ = writeln!(
            out,
            "| **{}** *(Custom)* | `{}` |",
            escape_table_cell(&cf.name),
            escape_table_cell(&cf.value)
        );
    }
    let _ = writeln!(out);

    // 4. Top-level Attachments Summary Table (if any)
    if !bundle.attachments.is_empty() {
        let _ = writeln!(out, "## Attachments ({})\n", bundle.attachments.len());
        let _ = writeln!(
            out,
            "| ID | File | Type | Size | Comment | Status |"
        );
        let _ = writeln!(
            out,
            "| :--- | :--- | :--- | :--- | :--- | :--- |"
        );
        for att in &bundle.attachments {
            let link_target = att
                .relative_path
                .as_deref()
                .unwrap_or(&att.download_url);
            let comment_label = att
                .comment_number
                .map(|n| format!("#{}", n))
                .unwrap_or_else(|| "-".to_string());
            let status_label = match &att.download_status {
                AttachmentDownloadStatus::Downloaded => {
                    if let Some(ref rel) = att.relative_path {
                        format!("Downloaded (`{}`)", escape_table_cell(rel))
                    } else {
                        "Downloaded".to_string()
                    }
                }
                AttachmentDownloadStatus::Skipped => "Skipped (`--skip-attachments`)".to_string(),
                AttachmentDownloadStatus::SkippedTooLarge => {
                    "Skipped (exceeds `--max-attachment-size`)".to_string()
                }
                AttachmentDownloadStatus::DeletedOnServer => "Deleted".to_string(),
                AttachmentDownloadStatus::Failed(err) => {
                    format!("Failed ({})", escape_table_cell(err))
                }
            };
            let _ = writeln!(
                out,
                "| `{}` | [{}]({}) | `{}` | {} | {} | {} |",
                att.attachment_id,
                escape_table_cell(&att.filename),
                link_target,
                escape_table_cell(&att.content_type),
                format_byte_size(att.size_bytes),
                comment_label,
                status_label
            );
        }
        let _ = writeln!(out);
    }

    // 5. Description (Comment #1)
    let _ = writeln!(out, "---\n");
    let _ = writeln!(out, "## Description\n");
    if let Some(ref desc) = bundle.description {
        render_comment_body(&mut out, desc);
    } else {
        let _ = writeln!(out, "*No description provided.*\n");
    }

    // 6. Comments Thread
    if !bundle.comments.is_empty() {
        let _ = writeln!(out, "---\n");
        let _ = writeln!(out, "## Comments ({})\n", bundle.comments.len());
        for (idx, comment) in bundle.comments.iter().enumerate() {
            if idx > 0 {
                let _ = writeln!(out, "---\n");
            }
            if let Some(num) = comment.comment_number {
                let _ = writeln!(out, "### Comment #{}\n", num);
            } else {
                let _ = writeln!(out, "### Update\n");
            }
            render_comment_body(&mut out, comment);
        }
    }

    out
}

fn render_comment_body(out: &mut String, entry: &CommentEntry) {
    let date_str = entry
        .timestamp
        .map(|t| t.format("%Y-%m-%d %H:%M:%S UTC").to_string())
        .unwrap_or_else(|| "Unknown date".to_string());

    let _ = writeln!(
        out,
        "**Author:** `{}` | **Date:** {}\n",
        entry.author, date_str
    );

    if !entry.field_updates.is_empty() {
        let _ = writeln!(out, "**Field Updates:**");
        for diff in &entry.field_updates {
            let _ = writeln!(out, "- `{}`: {}", diff.field, diff.summary);
        }
        let _ = writeln!(out);
    }

    if !entry.body.trim().is_empty() {
        match entry.formatting_mode {
            FormattingMode::Literal => {
                let _ = writeln!(out, "```text\n{}\n```\n", entry.body.trim_end());
            }
            FormattingMode::Plain | FormattingMode::Markdown => {
                let _ = writeln!(out, "{}\n", entry.body.trim_end());
            }
        }
    }

    if !entry.attachments.is_empty() {
        let _ = writeln!(out, "**Attachments:**");
        for att in &entry.attachments {
            render_attachment_item(out, att);
        }
        let _ = writeln!(out);
    }
}

fn render_attachment_item(out: &mut String, att: &AttachmentMeta) {
    let target = att
        .relative_path
        .as_deref()
        .unwrap_or(&att.download_url);
    let _ = writeln!(
        out,
        "- [{}]({}) (`{}`, {})",
        att.filename,
        target,
        att.content_type,
        format_byte_size(att.size_bytes)
    );
    if att.download_status == AttachmentDownloadStatus::Downloaded
        && att.content_type.starts_with("image/")
    {
        if let Some(ref rel) = att.relative_path {
            let _ = writeln!(out, "  ![{}]({})", att.filename, rel);
        }
    }
}

fn format_issue_links(ids: &[i64]) -> String {
    ids.iter()
        .map(|id| format!("[{}](https://issues.chromium.org/issues/{})", id, id))
        .collect::<Vec<_>>()
        .join(", ")
}

fn yaml_quote(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| format!("\"{}\"", s.replace('"', "\\\"")))
}

fn yaml_key(s: &str) -> String {
    if s.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        s.to_string()
    } else {
        yaml_quote(s)
    }
}

fn escape_table_cell(s: &str) -> String {
    s.replace('|', "\\|").replace('\n', " ")
}

fn escape_md_heading(s: &str) -> String {
    s.replace('\n', " ")
}
