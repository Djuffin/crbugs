use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use serde_json::Value;
use std::collections::HashMap;

use crate::models::{
    AttachmentDownloadStatus, AttachmentMeta, CodeChange, CommentEntry, CustomFieldDefMap,
    FieldDiff, FormattingMode, IssueBundle, ResolvedCustomField, SearchIssuesResult,
};

pub(crate) const GROUPING_WINDOW_SECS: i64 = 3600;

/// Strips the `)]}'\n` XSSI prefix, normalizes sparse JSPB array slots (`[,`, `,,`, `,]`) when needed,
/// and parses the JSON payload.
pub fn parse_xssi_json(raw: &str) -> Result<Value> {
    let trimmed = raw.trim_start();
    let (without_xssi, had_xssi) = if let Some(rest) = trimmed.strip_prefix(")]}'") {
        (rest.trim_start(), true)
    } else {
        (trimmed, false)
    };

    if without_xssi.is_empty() {
        return Err(anyhow!("Empty response from Issue Tracker"));
    }

    let val: Value = if had_xssi {
        let normalized = normalize_sparse_jspb(without_xssi);
        serde_json::from_str(&normalized).context("Failed to parse Issue Tracker JSPB payload")?
    } else {
        serde_json::from_str(without_xssi)
            .or_else(|_| serde_json::from_str(&normalize_sparse_jspb(without_xssi)))
            .context("Failed to parse Issue Tracker JSON payload")?
    };

    if let Some(err_msg) = val.get("message").and_then(Value::as_str) {
        if err_msg.contains("IamPermissionDeniedException") {
            return Err(anyhow!(
                "{} (If this is a restricted issue, use --auth corp or pass --cookie)",
                err_msg
            ));
        }
        return Err(anyhow!("Issue Tracker API error: {}", err_msg));
    }

    Ok(val)
}

/// Replaces empty slots in sparse JSPB arrays (`[,`, `,,`, `,]`) with `null`
/// while preserving string literals.
pub fn normalize_sparse_jspb(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut in_string = false;
    let mut escape_next = false;
    let mut last_sig = '\0';

    for ch in input.chars() {
        if in_string {
            out.push(ch);
            if escape_next {
                escape_next = false;
            } else if ch == '\\' {
                escape_next = true;
            } else if ch == '"' {
                in_string = false;
                last_sig = '"';
            }
            continue;
        }

        match ch {
            '"' => {
                in_string = true;
                out.push(ch);
            }
            ' ' | '\t' | '\r' | '\n' => {
                out.push(ch);
            }
            ',' => {
                if last_sig == '[' || last_sig == ',' {
                    out.push_str("null");
                }
                out.push(',');
                last_sig = ',';
            }
            ']' => {
                if last_sig == ',' {
                    out.push_str("null");
                }
                out.push(']');
                last_sig = ']';
            }
            _ => {
                out.push(ch);
                last_sig = ch;
            }
        }
    }

    out
}

/// Unwraps `[["b.MessageName", ...]]` or `["b.MessageName", ...]` to the inner message array.
fn unwrap_named_envelope<'a>(root: &'a Value, expected_tag: &str) -> Result<&'a [Value]> {
    let arr = root
        .as_array()
        .ok_or_else(|| anyhow!("Expected top-level JSON array for {}", expected_tag))?;

    if arr.first().and_then(Value::as_str) == Some(expected_tag) {
        return Ok(arr);
    }

    if let Some(first_inner) = arr.first().and_then(Value::as_array) {
        if first_inner.first().and_then(Value::as_str) == Some(expected_tag) {
            return Ok(first_inner);
        }
    }

    Err(anyhow!(
        "Unexpected JSPB envelope (expected '{}')",
        expected_tag
    ))
}

pub fn as_i64(v: &Value) -> Option<i64> {
    v.as_i64().or_else(|| v.as_str()?.parse::<i64>().ok())
}

pub fn as_u64(v: &Value) -> Option<u64> {
    v.as_u64().or_else(|| v.as_str()?.parse::<u64>().ok())
}

pub(crate) fn parse_i64_list(v: Option<&Value>) -> Vec<i64> {
    v.and_then(Value::as_array)
        .map(|arr| arr.iter().filter_map(as_i64).collect())
        .unwrap_or_default()
}

pub(crate) fn parse_string_list(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(ToOwned::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// Parses `User` proto (`[email_address, obfuscated_email_address, ...]`).
pub fn parse_user(v: Option<&Value>) -> Option<String> {
    let arr = v?.as_array()?;
    if let Some(email) = arr.first().and_then(Value::as_str) {
        if !email.trim().is_empty() {
            return Some(email.trim().to_string());
        }
    }
    if let Some(obf) = arr.get(1).and_then(Value::as_str) {
        if !obf.trim().is_empty() {
            return Some(obf.trim().to_string());
        }
    }
    None
}

fn parse_user_list(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array)
        .map(|arr| arr.iter().filter_map(|u| parse_user(Some(u))).collect())
        .unwrap_or_default()
}

/// Parses `google.protobuf.Timestamp` (`[seconds, nanos]`).
pub fn parse_timestamp(v: Option<&Value>) -> Option<DateTime<Utc>> {
    let arr = v?.as_array()?;
    let secs = as_i64(arr.first()?)?;
    let nanos = arr.get(1).and_then(as_i64).unwrap_or(0).max(0) as u32;
    DateTime::from_timestamp(secs, nanos)
}

pub fn map_issue_type(code: i64) -> String {
    match code {
        1 => "BUG".to_string(),
        2 => "FEATURE_REQUEST".to_string(),
        3 => "CUSTOMER_ISSUE".to_string(),
        4 => "INTERNAL_CLEANUP".to_string(),
        5 => "PROCESS".to_string(),
        6 => "VULNERABILITY".to_string(),
        7 => "PRIVACY_ISSUE".to_string(),
        8 => "PORTFOLIO".to_string(),
        9 => "PROGRAM".to_string(),
        10 => "PROJECT".to_string(),
        11 => "FEATURE".to_string(),
        12 => "MILESTONE".to_string(),
        13 => "EPIC".to_string(),
        14 => "STORY".to_string(),
        15 => "TASK".to_string(),
        other => format!("TYPE_{}", other),
    }
}

pub fn map_status(code: i64) -> String {
    match code {
        1 => "NEW".to_string(),
        2 => "ASSIGNED".to_string(),
        3 => "ACCEPTED".to_string(),
        4 => "FIXED".to_string(),
        5 => "VERIFIED".to_string(),
        6 => "NOT_REPRODUCIBLE".to_string(),
        7 => "INTENDED_BEHAVIOR".to_string(),
        8 => "OBSOLETE".to_string(),
        9 => "INFEASIBLE".to_string(),
        10 => "DUPLICATE".to_string(),
        11 => "INACTIVE".to_string(),
        other => format!("STATUS_{}", other),
    }
}

pub fn map_priority(code: i64) -> String {
    if (1..=5).contains(&code) {
        format!("P{}", code - 1)
    } else {
        "UNSPECIFIED".to_string()
    }
}

pub fn map_severity(code: i64) -> String {
    if (1..=5).contains(&code) {
        format!("S{}", code - 1)
    } else {
        "UNSPECIFIED".to_string()
    }
}

pub fn map_custom_field_type(code: i64) -> String {
    match code {
        1 => "TEXT".to_string(),
        2 => "DATE".to_string(),
        3 => "ENUM".to_string(),
        4 => "NUMERIC".to_string(),
        5 => "REPEATED_TEXT".to_string(),
        6 => "REPEATED_DATE".to_string(),
        7 => "REPEATED_ENUM".to_string(),
        8 => "REPEATED_NUMERIC".to_string(),
        other => format!("TYPE_{}", other),
    }
}

pub fn map_access_level(v: Option<&Value>) -> String {
    let code = v
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .and_then(as_i64)
        .unwrap_or(0);
    match code {
        1 => "LIMIT_NONE".to_string(),
        2 => "LIMIT_VIEW".to_string(),
        3 => "LIMIT_APPEND".to_string(),
        4 => "LIMIT_VIEW_TRUSTED".to_string(),
        _ => "UNSPECIFIED".to_string(),
    }
}

pub fn map_formatting_mode(code: Option<i64>) -> FormattingMode {
    match code.unwrap_or(1) {
        2 => FormattingMode::Markdown,
        3 => FormattingMode::Literal,
        _ => FormattingMode::Plain,
    }
}

/// Extracts custom field value string from `CustomFieldValue` JSPB array (`tag 1..10`).
fn extract_custom_field_value(cf_val_arr: &[Value]) -> String {
    // tag 10 (index 9): display_string
    if let Some(display) = cf_val_arr.get(9).and_then(Value::as_str) {
        if !display.is_empty() {
            return display.to_string();
        }
    }
    // tag 2 (index 1): text_value, tag 4 (index 3): enum_value
    for idx in [1usize, 3usize] {
        if let Some(s) = cf_val_arr.get(idx).and_then(Value::as_str) {
            if !s.is_empty() {
                return s.to_string();
            }
        }
    }
    // tag 5 (index 4): numeric_value
    if let Some(n) = cf_val_arr.get(4) {
        if let Some(i) = as_i64(n) {
            return i.to_string();
        }
        if let Some(f) = n.as_f64() {
            return f.to_string();
        }
    }
    // tag 6 (index 5): repeated_text_value `[[v1, v2, ...]]`, tag 8 (index 7): repeated_enum_value `[[v1, v2, ...]]`
    for idx in [5usize, 7usize] {
        if let Some(rep) = cf_val_arr
            .get(idx)
            .and_then(Value::as_array)
            .and_then(|a| a.first())
            .and_then(Value::as_array)
        {
            let items: Vec<&str> = rep.iter().filter_map(Value::as_str).collect();
            if !items.is_empty() {
                return items.join(", ");
            }
        }
    }
    String::new()
}

/// Parses `b.IssueFetchResponse` from `GET /action/issues/{id}`.
/// Returns the populated `IssueBundle` and a map of `custom_field_id -> (name, field_type)`.
pub fn parse_issue_fetch_response(
    root: &Value,
    base_url: &str,
) -> Result<(IssueBundle, CustomFieldDefMap)> {
    let envelope = unwrap_named_envelope(root, "b.IssueFetchResponse")?;
    let fe_issue = envelope
        .get(1)
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("Missing fe.Issue at b.IssueFetchResponse[1]"))?;

    // `model.proto`: `google.devtools.issuetracker.v1.Issue it = 23` -> index 22
    let it_issue = fe_issue
        .get(22)
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("Missing issuetracker.v1.Issue at fe.Issue[22]"))?;

    parse_it_issue_array(it_issue, base_url)
}

/// Parses a `google.devtools.issuetracker.v1.Issue` JSPB array into an `IssueBundle`
/// and custom field definition map.
pub fn parse_it_issue_array(
    it_issue: &[Value],
    base_url: &str,
) -> Result<(IssueBundle, CustomFieldDefMap)> {
    let issue_id = it_issue
        .get(1)
        .and_then(as_i64)
        .ok_or_else(|| anyhow!("Missing issue_id at Issue[1]"))?;

    let state = it_issue
        .get(2)
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("Missing IssueState at Issue[2]"))?;

    let component_id = state.first().and_then(as_i64).unwrap_or(0);
    let issue_type = map_issue_type(state.get(1).and_then(as_i64).unwrap_or(0));
    let status = map_status(state.get(2).and_then(as_i64).unwrap_or(0));
    let priority = map_priority(state.get(3).and_then(as_i64).unwrap_or(0));
    let severity = map_severity(state.get(4).and_then(as_i64).unwrap_or(0));
    let title = state
        .get(5)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    let reporter = parse_user(state.get(6));
    let assignee = parse_user(state.get(7));
    let verifier = parse_user(state.get(8));
    let ccs = parse_user_list(state.get(9));
    let canonical_issue_id = state.get(10).and_then(as_i64);
    let blocked_by_ids = parse_i64_list(state.get(11));
    let blocking_ids = parse_i64_list(state.get(12));
    let hotlist_ids = parse_i64_list(state.get(13));
    let found_in_versions = parse_string_list(state.get(16));
    let targeted_to_versions = parse_string_list(state.get(17));
    let verified_in_versions = parse_string_list(state.get(18));
    let in_prod = state.get(19).and_then(Value::as_bool).unwrap_or(false);
    let duplicate_issue_ids = parse_i64_list(state.get(21));
    let collaborators = parse_user_list(state.get(30));
    let access_level = map_access_level(state.get(31));
    let (pending_code_changes, code_changes) = parse_gerrit_changes_list(state.get(34));

    let created_time = parse_timestamp(it_issue.get(4));
    let modified_time = parse_timestamp(it_issue.get(5));
    let resolved_time = parse_timestamp(it_issue.get(6));
    let verified_time = parse_timestamp(it_issue.get(7));
    let vote_count = it_issue.get(9).and_then(as_i64).unwrap_or(0);
    let version = it_issue.get(11).and_then(as_i64);
    let parent_issue_ids = parse_i64_list(it_issue.get(36));
    let is_archived = it_issue
        .get(39)
        .and_then(Value::as_bool)
        .or_else(|| state.get(20).and_then(Value::as_bool))
        .unwrap_or(false);

    // Parse CustomField definitions at `it_issue[14]` (`tag 15`)
    let mut custom_field_defs: CustomFieldDefMap = HashMap::new();
    if let Some(defs) = it_issue.get(14).and_then(Value::as_array) {
        for def in defs {
            if let Some(def_arr) = def.as_array() {
                if let Some(cf_id) = def_arr.first().and_then(as_i64) {
                    let cf_type = map_custom_field_type(def_arr.get(2).and_then(as_i64).unwrap_or(0));
                    let cf_name = def_arr
                        .get(4)
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    if !cf_name.is_empty() {
                        custom_field_defs.insert(cf_id, (cf_name, cf_type));
                    }
                }
            }
        }
    }

    // Parse CustomFieldValue entries at `state[14]` (`tag 15`)
    let mut raw_cf_values = Vec::new();
    if let Some(vals) = state.get(14).and_then(Value::as_array) {
        for val in vals {
            if let Some(val_arr) = val.as_array() {
                if let Some(cf_id) = val_arr.first().and_then(as_i64) {
                    let value_str = extract_custom_field_value(val_arr);
                    raw_cf_values.push((cf_id, value_str));
                }
            }
        }
    }
    let custom_fields = resolve_custom_fields(raw_cf_values, &custom_field_defs);

    // Also parse initial description from `it_issue[43]` (`tag 44`) as fallback
    let fallback_description = it_issue
        .get(43)
        .and_then(Value::as_array)
        .map(|desc_arr| parse_issue_comment_array(desc_arr, reporter.clone(), created_time));

    let url = format!("{}/issues/{}", base_url.trim_end_matches('/'), issue_id);

    let bundle = IssueBundle {
        issue_id,
        url,
        title,
        issue_type,
        status,
        priority,
        severity,
        component_id,
        component_path: Vec::new(),
        reporter,
        assignee,
        verifier,
        ccs,
        collaborators,
        created_time,
        modified_time,
        resolved_time,
        verified_time,
        vote_count,
        version,
        hotlist_ids,
        blocked_by_ids,
        blocking_ids,
        duplicate_issue_ids,
        canonical_issue_id,
        parent_issue_ids,
        found_in_versions,
        targeted_to_versions,
        verified_in_versions,
        in_prod,
        is_archived,
        access_level,
        pending_code_changes,
        code_changes,
        custom_fields,
        description: fallback_description,
        comments: Vec::new(),
        attachments: Vec::new(),
    };

    Ok((bundle, custom_field_defs))
}

pub(crate) fn map_gerrit_state(code: i64) -> String {
    match code {
        1 => "PENDING".to_string(),
        2 => "MERGED".to_string(),
        _ => "STATE_UNSPECIFIED".to_string(),
    }
}

pub(crate) fn format_gerrit_url(host: &str, repo: &str, change_number: i64) -> String {
    if repo.is_empty() {
        format!("https://{}-review.googlesource.com/{}", host, change_number)
    } else {
        format!(
            "https://{}-review.googlesource.com/c/{}/+/{}",
            host, repo, change_number
        )
    }
}

pub(crate) fn parse_gerrit_change_array(arr: &[Value]) -> Option<CodeChange> {
    let host = arr.first()?.as_str()?.trim().to_string();
    let repo = arr
        .get(1)
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    let change_number = arr.get(2).and_then(as_i64)?;
    let state = map_gerrit_state(arr.get(3).and_then(as_i64).unwrap_or(0));
    let branch = arr
        .get(4)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned);
    let url = format_gerrit_url(&host, &repo, change_number);

    Some(CodeChange {
        host,
        repo,
        change_number,
        state,
        branch,
        url,
    })
}

fn parse_gerrit_changes_list(v: Option<&Value>) -> (Vec<CodeChange>, Vec<CodeChange>) {
    let mut pending = Vec::new();
    let mut merged = Vec::new();
    if let Some(arr) = v.and_then(Value::as_array) {
        for item in arr {
            if let Some(item_arr) = item.as_array() {
                if let Some(gc) = parse_gerrit_change_array(item_arr) {
                    if gc.state == "PENDING" {
                        pending.push(gc);
                    } else {
                        merged.push(gc);
                    }
                }
            }
        }
    }
    (pending, merged)
}

pub(crate) fn resolve_custom_fields(
    raw_values: Vec<(i64, String)>,
    defs: &CustomFieldDefMap,
) -> Vec<ResolvedCustomField> {
    let mut custom_fields: Vec<ResolvedCustomField> = raw_values
        .into_iter()
        .filter(|(_, val)| !val.is_empty())
        .map(|(cf_id, value)| {
            let (name, field_type) = defs
                .get(&cf_id)
                .cloned()
                .unwrap_or_else(|| (format!("field_{}", cf_id), "UNKNOWN".to_string()));
            ResolvedCustomField {
                id: cf_id,
                name,
                field_type,
                value,
            }
        })
        .collect();
    custom_fields.sort_by(|a, b| a.name.cmp(&b.name));
    custom_fields
}

/// Parses `b.IssueSearchResponse` from `POST /action/issues/list`.
pub fn parse_issue_search_response(
    root: &Value,
    query: &str,
    sort_by: &str,
    base_url: &str,
) -> Result<SearchIssuesResult> {
    let envelope = unwrap_named_envelope(root, "b.IssueSearchResponse")?;
    // `model.proto`: `google.devtools.issuetracker.v1.ListIssuesResponse it = 6` -> index 6 in named JSPB
    let it_resp = envelope.get(6).and_then(Value::as_array);

    let mut issues = Vec::new();
    let mut next_page_token = None;
    let mut total_size = 0usize;
    let mut total_size_accurate = false;

    if let Some(it) = it_resp {
        if let Some(raw_issues) = it.first().and_then(Value::as_array) {
            for item in raw_issues {
                if let Some(it_issue) = item.as_array() {
                    if let Ok((bundle, _)) = parse_it_issue_array(it_issue, base_url) {
                        issues.push(bundle);
                    }
                }
            }
        }
        next_page_token = it
            .get(1)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned);
        total_size = it
            .get(2)
            .and_then(as_u64)
            .map(|n| n as usize)
            .unwrap_or(issues.len());
        total_size_accurate = it.get(3).and_then(Value::as_bool).unwrap_or(false);
    }

    Ok(SearchIssuesResult {
        query: query.to_string(),
        sort_by: sort_by.to_string(),
        total_size,
        total_size_accurate,
        next_page_token,
        issues,
    })
}

fn parse_issue_comment_array(
    comment_arr: &[Value],
    fallback_author: Option<String>,
    fallback_time: Option<DateTime<Utc>>,
) -> CommentEntry {
    let body = comment_arr
        .first()
        .and_then(Value::as_str)
        .unwrap_or("")
        .replace("\r\n", "\n");
    let last_editor = parse_user(comment_arr.get(2));
    let original_author = parse_user(comment_arr.get(17));
    let author = original_author
        .or(last_editor)
        .or(fallback_author)
        .unwrap_or_else(|| "unknown".to_string());
    let modified_time = parse_timestamp(comment_arr.get(3));
    let created_time = parse_timestamp(comment_arr.get(18))
        .or(fallback_time)
        .or(modified_time);
    let comment_number = comment_arr.get(6).and_then(as_i64).map(|n| n as i32);
    let version = comment_arr.get(7).and_then(as_i64);
    let formatting_mode = map_formatting_mode(comment_arr.get(8).and_then(as_i64));
    let redacted = comment_arr
        .get(12)
        .and_then(Value::as_bool)
        .unwrap_or(false);

    CommentEntry {
        comment_number,
        author,
        timestamp: created_time,
        modified_time,
        body,
        formatting_mode,
        redacted,
        version,
        attachments: Vec::new(),
        field_updates: Vec::new(),
    }
}

/// Parses `b.Component` from `GET /action/components/{componentId}` to extract its path names.
pub fn parse_component_response(root: &Value) -> Vec<String> {
    let Ok(envelope) = unwrap_named_envelope(root, "b.Component") else {
        return Vec::new();
    };
    // In `ModelProto.Component`, `it` (`google.devtools.issuetracker.v1.Component`) is at tag 28 -> index 28 in named JSPB
    let Some(it_comp) = envelope.get(28).and_then(Value::as_array) else {
        return Vec::new();
    };
    // In `issuetracker.v1.Component`, `component_path_info` is tag 7 -> index 6,
    // and `component_path_names` is tag 2 -> index 1 inside `ComponentPathInfo`.
    let Some(names) = it_comp
        .get(6)
        .and_then(Value::as_array)
        .and_then(|cpi| cpi.get(1))
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };

    names
        .iter()
        .filter_map(Value::as_str)
        .filter(|s| !s.is_empty() && !s.chars().all(|c| c.is_ascii_digit()))
        .map(ToOwned::to_owned)
        .collect()
}

/// Intermediate representation of a single issue update shared by JSPB and ProtoJSON parsers.
#[derive(Debug)]
pub(crate) struct RawIssueUpdate {
    pub author: String,
    pub timestamp: Option<DateTime<Utc>>,
    pub comment: Option<CommentEntry>,
    pub comment_number: Option<i32>,
    pub version: Option<i64>,
    pub attachments: Vec<AttachmentMeta>,
    pub field_diffs: Vec<FieldDiff>,
    pub is_initial: bool,
}

/// Groups chronological `RawIssueUpdate` items using Buganizer's `UpdateGroup` rules:
/// - Initial update (comment #1 / version 0) stays in its own group.
/// - Subsequent updates by the same author within 1 hour (`GROUPING_WINDOW_SECS`) with at most
///   one comment are merged into a single `CommentEntry` so attachments uploaded right before/after
///   a comment are coalesced onto that comment.
pub(crate) fn coalesce_issue_updates(
    parsed_updates: Vec<RawIssueUpdate>,
    include_field_updates: bool,
) -> (Option<CommentEntry>, Vec<CommentEntry>, Vec<AttachmentMeta>) {
    let mut groups: Vec<CommentEntry> = Vec::new();

    for upd in parsed_updates {
        let can_merge = if let Some(last) = groups.last() {
            let last_is_initial = last.comment_number == Some(1) || last.version == Some(0);
            let same_author = last.author == upd.author;
            let both_have_comments = !last.body.is_empty() && upd.comment.is_some();
            let within_window = match (last.timestamp, upd.timestamp) {
                (Some(t1), Some(t2)) => (t2 - t1).num_seconds().abs() <= GROUPING_WINDOW_SECS,
                _ => true,
            };
            !last_is_initial
                && !upd.is_initial
                && same_author
                && !both_have_comments
                && within_window
        } else {
            false
        };

        if can_merge {
            let last = groups.last_mut().expect("non-empty");
            if let Some(c) = upd.comment {
                last.comment_number = c.comment_number.or(last.comment_number);
                last.body = c.body;
                last.formatting_mode = c.formatting_mode;
                last.redacted = c.redacted;
                last.modified_time = c.modified_time;
                if c.timestamp.is_some() {
                    last.timestamp = c.timestamp;
                }
            }
            for mut att in upd.attachments {
                att.comment_number = last.comment_number;
                last.attachments.push(att);
            }
            let resolved_num = last.comment_number;
            for att in &mut last.attachments {
                if att.comment_number.is_none() {
                    att.comment_number = resolved_num;
                }
            }
            last.field_updates.extend(upd.field_diffs);
        } else {
            let mut entry = if let Some(c) = upd.comment {
                c
            } else {
                CommentEntry {
                    comment_number: upd.comment_number,
                    author: upd.author,
                    timestamp: upd.timestamp,
                    modified_time: None,
                    body: String::new(),
                    formatting_mode: FormattingMode::Plain,
                    redacted: false,
                    version: upd.version,
                    attachments: Vec::new(),
                    field_updates: Vec::new(),
                }
            };
            let resolved_num = entry.comment_number;
            for mut att in upd.attachments {
                if att.comment_number.is_none() {
                    att.comment_number = resolved_num;
                }
                entry.attachments.push(att);
            }
            entry.field_updates = upd.field_diffs;
            groups.push(entry);
        }
    }

    let mut description: Option<CommentEntry> = None;
    let mut comments: Vec<CommentEntry> = Vec::new();
    let mut all_attachments: Vec<AttachmentMeta> = Vec::new();

    for mut entry in groups {
        if entry.body.trim() == "[Empty comment from Monorail migration]" {
            entry.body.clear();
        }
        if !include_field_updates {
            entry.field_updates.clear();
        }
        for att in &entry.attachments {
            all_attachments.push(att.clone());
        }

        if entry.comment_number == Some(1)
            || (description.is_none() && entry.version == Some(0) && !entry.body.is_empty())
        {
            description = Some(entry);
        } else if !entry.body.is_empty()
            || !entry.attachments.is_empty()
            || (include_field_updates && !entry.field_updates.is_empty())
        {
            comments.push(entry);
        }
    }

    (description, comments, all_attachments)
}

/// Parses `b.ListIssueUpdatesResponse` from `POST /action/issues/{id}/updates`,
/// coalescing attachment-only updates with adjacent comments by the same author.
pub fn parse_updates_response(
    root: &Value,
    issue_id: i64,
    usercontent_url: &str,
    custom_field_defs: &CustomFieldDefMap,
    include_field_updates: bool,
) -> Result<(Option<CommentEntry>, Vec<CommentEntry>, Vec<AttachmentMeta>)> {
    let envelope = unwrap_named_envelope(root, "b.ListIssueUpdatesResponse")?;
    let Some(it_resp) = envelope.get(1).and_then(Value::as_array) else {
        return Ok((None, Vec::new(), Vec::new()));
    };
    let Some(raw_updates) = it_resp.first().and_then(Value::as_array) else {
        return Ok((None, Vec::new(), Vec::new()));
    };

    let mut parsed_updates: Vec<RawIssueUpdate> = Vec::with_capacity(raw_updates.len());

    for (idx, u_val) in raw_updates.iter().enumerate() {
        let Some(u) = u_val.as_array() else {
            continue;
        };
        let author = parse_user(u.first()).unwrap_or_else(|| "unknown".to_string());
        let timestamp = parse_timestamp(u.get(1));
        let comment_number = u.get(3).and_then(as_i64).map(|n| n as i32);
        let version = u.get(6).and_then(as_i64);
        let is_initial = idx == 0 || comment_number == Some(1) || version == Some(0);

        let comment = u.get(2).and_then(Value::as_array).map(|c_arr| {
            let mut entry = parse_issue_comment_array(c_arr, Some(author.clone()), timestamp);
            if entry.comment_number.is_none() {
                entry.comment_number = comment_number;
            }
            if entry.version.is_none() {
                entry.version = version;
            }
            entry
        });

        let mut attachments = Vec::new();
        if let Some(att_list) = u.get(7).and_then(Value::as_array) {
            for att_val in att_list {
                if let Some(att_arr) = att_val.as_array() {
                    if let Some(meta) =
                        parse_attachment_array(att_arr, issue_id, comment_number, usercontent_url)
                    {
                        attachments.push(meta);
                    }
                }
            }
        }

        let field_diffs = parse_field_updates_array(u.get(5), custom_field_defs, is_initial);

        parsed_updates.push(RawIssueUpdate {
            author,
            timestamp,
            comment,
            comment_number,
            version,
            attachments,
            field_diffs,
            is_initial,
        });
    }

    Ok(coalesce_issue_updates(parsed_updates, include_field_updates))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn build_attachment_meta(
    attachment_id: i64,
    issue_id: i64,
    comment_number: Option<i32>,
    raw_filename: String,
    content_type: String,
    size_bytes: u64,
    is_deleted: bool,
    usercontent_url: &str,
) -> AttachmentMeta {
    let clean_name = sanitize_filename::sanitize(&raw_filename);
    let clean_name = if clean_name.is_empty() {
        "attachment.bin".to_string()
    } else {
        clean_name
    };
    let sanitized_filename = format!("{}_{}", attachment_id, clean_name);
    let download_url = format!(
        "{}/download/attachment/{}/{}?download=true",
        usercontent_url.trim_end_matches('/'),
        issue_id,
        attachment_id
    );
    let download_status = if is_deleted {
        AttachmentDownloadStatus::DeletedOnServer
    } else {
        AttachmentDownloadStatus::Skipped
    };

    AttachmentMeta {
        attachment_id,
        issue_id,
        comment_number,
        filename: raw_filename,
        sanitized_filename,
        content_type,
        size_bytes,
        download_url,
        is_deleted,
        relative_path: None,
        local_path: None,
        download_status,
    }
}

fn parse_attachment_array(
    att_arr: &[Value],
    issue_id: i64,
    comment_number: Option<i32>,
    usercontent_url: &str,
) -> Option<AttachmentMeta> {
    let attachment_id = att_arr.first().and_then(as_i64)?;
    let content_type = att_arr
        .get(1)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .unwrap_or("application/octet-stream")
        .to_string();
    let size_bytes = att_arr.get(2).and_then(as_u64).unwrap_or(0);
    let raw_filename = att_arr
        .get(3)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .unwrap_or("attachment.bin")
        .to_string();

    // tag 6 (index 5): `entity_status` -> `[status_enum]` where `1 = ACTIVE`, `2 = DELETED`, `3 = PURGED`
    let status_code = att_arr
        .get(5)
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .and_then(as_i64)
        .unwrap_or(1);
    let is_deleted = status_code != 1;

    Some(build_attachment_meta(
        attachment_id,
        issue_id,
        comment_number,
        raw_filename,
        content_type,
        size_bytes,
        is_deleted,
        usercontent_url,
    ))
}

fn parse_field_updates_array(
    v: Option<&Value>,
    custom_field_defs: &CustomFieldDefMap,
    is_initial: bool,
) -> Vec<FieldDiff> {
    if is_initial {
        return Vec::new();
    }
    let Some(arr) = v.and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut diffs = Vec::new();
    for item in arr {
        let Some(f_arr) = item.as_array() else {
            continue;
        };
        let Some(field_name) = f_arr.first().and_then(Value::as_str) else {
            continue;
        };

        // tag 3 (index 2): single_value_update `[old_any, new_any]`
        if let Some(single) = f_arr.get(2).and_then(Value::as_array) {
            let old_val = format_proto_any(single.first(), field_name);
            let new_val = format_proto_any(single.get(1), field_name);
            let label = resolve_field_label(
                field_name,
                single.get(1).or(single.first()),
                custom_field_defs,
            );
            let summary = match (old_val, new_val) {
                (Some(o), Some(n)) => format!("{} -> {}", o, n),
                (None, Some(n)) => format!("set to {}", n),
                (Some(o), None) => format!("cleared (was {})", o),
                (None, None) => continue,
            };
            diffs.push(FieldDiff {
                field: label,
                summary,
            });
        }
        // tag 4 (index 3): collection_update `[added_list, removed_list]`
        else if let Some(coll) = f_arr.get(3).and_then(Value::as_array) {
            let added = coll
                .first()
                .and_then(Value::as_array)
                .map(|list| {
                    list.iter()
                        .filter_map(|a| format_proto_any(Some(a), field_name))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let removed = coll
                .get(1)
                .and_then(Value::as_array)
                .map(|list| {
                    list.iter()
                        .filter_map(|a| format_proto_any(Some(a), field_name))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            if let Some(diff) = build_collection_diff(field_name, &added, &removed) {
                diffs.push(diff);
            }
        }
    }
    diffs
}

pub(crate) fn build_collection_diff(
    field_name: &str,
    added: &[String],
    removed: &[String],
) -> Option<FieldDiff> {
    let mut parts = Vec::new();
    if !added.is_empty() {
        parts.push(format!("+{}", added.join(", +")));
    }
    if !removed.is_empty() {
        parts.push(format!("-{}", removed.join(", -")));
    }
    if parts.is_empty() {
        None
    } else {
        Some(FieldDiff {
            field: field_name.to_string(),
            summary: parts.join("; "),
        })
    }
}

fn resolve_field_label(
    field_name: &str,
    any_val: Option<&Value>,
    custom_field_defs: &CustomFieldDefMap,
) -> String {
    if field_name == "custom_fields" {
        if let Some(arr) = any_val.and_then(Value::as_array) {
            if let Some(payload) = arr.get(1).and_then(Value::as_array) {
                if let Some(cf_id) = payload.first().and_then(as_i64) {
                    if let Some((name, _)) = custom_field_defs.get(&cf_id) {
                        return name.clone();
                    }
                    return format!("custom_field_{}", cf_id);
                }
            }
        }
    }
    field_name.to_string()
}

pub(crate) fn map_field_int_code(field_name: &str, code: i64) -> String {
    match field_name {
        "status" => map_status(code),
        "priority" => map_priority(code),
        "severity" => map_severity(code),
        "type" => map_issue_type(code),
        _ => code.to_string(),
    }
}

fn format_proto_any(any_val: Option<&Value>, field_name: &str) -> Option<String> {
    let arr = any_val?.as_array()?;
    let type_url = arr.first()?.as_str()?;
    let payload = arr.get(1)?.as_array()?;

    if type_url.ends_with("CustomFieldValue") {
        let v = extract_custom_field_value(payload);
        return if v.is_empty() { None } else { Some(v) };
    }
    if type_url.ends_with("User") {
        return parse_user(Some(arr.get(1)?));
    }
    if type_url.ends_with("Int32Value") || type_url.ends_with("Int64Value") {
        let code = payload.first().and_then(as_i64)?;
        return Some(map_field_int_code(field_name, code));
    }
    if type_url.ends_with("StringValue") {
        return payload.first()?.as_str().map(ToOwned::to_owned);
    }
    if type_url.ends_with("BoolValue") {
        return payload.first()?.as_bool().map(|b| b.to_string());
    }
    if type_url.ends_with("GerritChange") {
        if let Some(gc) = parse_gerrit_change_array(payload) {
            return Some(format!("{} ({})", gc.url, gc.state));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_sparse_jspb() {
        let raw = ")]}'\n[[\"b.Test\",,1,[,\"a,b\"],]]";
        let val = parse_xssi_json(raw).unwrap();
        let arr = val.as_array().unwrap()[0].as_array().unwrap();
        assert_eq!(arr[0].as_str(), Some("b.Test"));
        assert!(arr[1].is_null());
        assert_eq!(arr[2].as_i64(), Some(1));
        assert!(arr[3].as_array().unwrap()[0].is_null());
        assert_eq!(arr[3].as_array().unwrap()[1].as_str(), Some("a,b"));
        assert!(arr[4].is_null());
    }
}
