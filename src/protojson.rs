use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use serde_json::Value;
use std::collections::HashMap;

use crate::jspb::{
    as_i64, as_u64, build_attachment_meta, build_collection_diff, coalesce_issue_updates,
    format_gerrit_url, map_field_int_code, parse_i64_list, parse_string_list,
    resolve_custom_fields, RawIssueUpdate,
};
use crate::models::{
    AttachmentMeta, CodeChange, CommentEntry, CustomFieldDefMap, FieldDiff, FormattingMode,
    IssueBundle, SearchIssuesResult,
};

fn parse_rfc3339(v: Option<&Value>) -> Option<DateTime<Utc>> {
    let s = v?.as_str()?;
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

fn parse_user_obj(v: Option<&Value>) -> Option<String> {
    let obj = v?.as_object()?;
    if let Some(email) = obj.get("emailAddress").and_then(Value::as_str) {
        if !email.trim().is_empty() {
            return Some(email.trim().to_string());
        }
    }
    if let Some(obf) = obj.get("obfuscatedEmailAddress").and_then(Value::as_str) {
        if !obf.trim().is_empty() {
            return Some(obf.trim().to_string());
        }
    }
    None
}

fn parse_user_obj_list(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array)
        .map(|arr| arr.iter().filter_map(|u| parse_user_obj(Some(u))).collect())
        .unwrap_or_default()
}

fn parse_formatting_mode_str(v: Option<&Value>) -> FormattingMode {
    match v.and_then(Value::as_str).unwrap_or("PLAIN") {
        "MARKDOWN" => FormattingMode::Markdown,
        "LITERAL" => FormattingMode::Literal,
        _ => FormattingMode::Plain,
    }
}

fn extract_v1_custom_field_value(val_obj: &serde_json::Map<String, Value>) -> String {
    if let Some(display) = val_obj.get("displayString").and_then(Value::as_str) {
        if !display.is_empty() {
            return display.to_string();
        }
    }
    for key in ["textValue", "enumValue"] {
        if let Some(s) = val_obj.get(key).and_then(Value::as_str) {
            if !s.is_empty() {
                return s.to_string();
            }
        }
    }
    if let Some(n) = val_obj.get("numericValue") {
        if let Some(i) = as_i64(n) {
            return i.to_string();
        }
        if let Some(f) = n.as_f64() {
            return f.to_string();
        }
    }
    for key in [
        "repeatedTextValue",
        "repeatedEnumValue",
        "repeatedNumericValue",
    ] {
        if let Some(vals) = val_obj
            .get(key)
            .and_then(Value::as_object)
            .and_then(|o| o.get("values"))
            .and_then(Value::as_array)
        {
            let items: Vec<String> = vals
                .iter()
                .filter_map(|v| {
                    v.as_str()
                        .map(ToOwned::to_owned)
                        .or_else(|| as_i64(v).map(|n| n.to_string()))
                })
                .collect();
            if !items.is_empty() {
                return items.join(", ");
            }
        }
    }
    String::new()
}

/// Checks if a v1 REST JSON response contains a Google RPC `{"error": ...}` object.
pub fn check_v1_error(root: &Value) -> Result<()> {
    if let Some(err_obj) = root.get("error").and_then(Value::as_object) {
        let code = err_obj.get("code").and_then(as_i64).unwrap_or(0);
        let status = err_obj
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("UNKNOWN");
        let message = err_obj
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("Unknown Issue Tracker v1 API error");
        return Err(anyhow!(
            "Corp API error (HTTP {} {}): {}",
            code,
            status,
            message
        ));
    }
    Ok(())
}

/// Parses a `google.devtools.issuetracker.v1.Issue` ProtoJSON object from `/v1/issues/{id}` or `/v1/issues`.
pub fn parse_v1_issue(
    root: &Value,
    web_base_url: &str,
) -> Result<(IssueBundle, CustomFieldDefMap)> {
    check_v1_error(root)?;

    let issue_id = root
        .get("issueId")
        .and_then(as_i64)
        .ok_or_else(|| anyhow!("Missing issueId in v1 Issue response"))?;

    let state = root
        .get("issueState")
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow!("Missing issueState in v1 Issue response"))?;

    let component_id = state.get("componentId").and_then(as_i64).unwrap_or(0);
    let issue_type = state
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("UNSPECIFIED")
        .to_string();
    let status = state
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("UNSPECIFIED")
        .to_string();
    let priority = state
        .get("priority")
        .and_then(Value::as_str)
        .unwrap_or("UNSPECIFIED")
        .to_string();
    let severity = state
        .get("severity")
        .and_then(Value::as_str)
        .unwrap_or("UNSPECIFIED")
        .to_string();
    let title = state
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    let reporter = parse_user_obj(state.get("reporter"));
    let assignee = parse_user_obj(state.get("assignee"));
    let verifier = parse_user_obj(state.get("verifier"));
    let ccs = parse_user_obj_list(state.get("ccs"));
    let collaborators = parse_user_obj_list(state.get("collaborators"));
    let canonical_issue_id = state.get("canonicalIssueId").and_then(as_i64);
    let blocked_by_ids = parse_i64_list(state.get("blockedByIssueIds"));
    let blocking_ids = parse_i64_list(state.get("blockingIssueIds"));
    let hotlist_ids = parse_i64_list(state.get("hotlistIds"));
    let duplicate_issue_ids = parse_i64_list(state.get("duplicateIssueIds"));
    let found_in_versions = parse_string_list(state.get("foundInVersions"));
    let targeted_to_versions = parse_string_list(state.get("targetedToVersions"));
    let verified_in_versions = parse_string_list(state.get("verifiedInVersions"));
    let in_prod = state
        .get("inProd")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let access_level = state
        .get("accessLimit")
        .or_else(|| root.get("accessLimit"))
        .and_then(|v| v.get("accessLevel"))
        .and_then(Value::as_str)
        .unwrap_or("LIMIT_NONE")
        .to_string();

    let created_time = parse_rfc3339(root.get("createdTime"));
    let modified_time = parse_rfc3339(root.get("modifiedTime"));
    let resolved_time = parse_rfc3339(root.get("resolvedTime"));
    let verified_time = parse_rfc3339(root.get("verifiedTime"));
    let vote_count = root.get("voteCount").and_then(as_i64).unwrap_or(0);
    let version = root.get("version").and_then(as_i64);
    let parent_issue_ids = parse_i64_list(root.get("parentIssueIds"));
    let is_archived = root
        .get("isArchived")
        .and_then(Value::as_bool)
        .or_else(|| state.get("isArchived").and_then(Value::as_bool))
        .unwrap_or(false);

    let mut custom_field_defs: CustomFieldDefMap = HashMap::new();
    if let Some(defs) = root.get("customFields").and_then(Value::as_array) {
        for def in defs {
            if let Some(def_obj) = def.as_object() {
                if let Some(cf_id) = def_obj.get("customFieldId").and_then(as_i64) {
                    let cf_type = def_obj
                        .get("type")
                        .and_then(Value::as_str)
                        .unwrap_or("UNKNOWN")
                        .to_string();
                    let cf_name = def_obj
                        .get("name")
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

    let mut raw_cf_values = Vec::new();
    if let Some(vals) = state.get("customFields").and_then(Value::as_array) {
        for val in vals {
            if let Some(val_obj) = val.as_object() {
                if let Some(cf_id) = val_obj.get("customFieldId").and_then(as_i64) {
                    let value_str = extract_v1_custom_field_value(val_obj);
                    raw_cf_values.push((cf_id, value_str));
                }
            }
        }
    }
    let custom_fields = resolve_custom_fields(raw_cf_values, &custom_field_defs);

    let fallback_description = root
        .get("description")
        .or_else(|| root.get("issueComment"))
        .and_then(Value::as_object)
        .and_then(|desc_obj| {
            let body = desc_obj
                .get("comment")
                .and_then(Value::as_str)
                .unwrap_or("");
            if body.is_empty() {
                return None;
            }
            Some(parse_v1_comment_obj(
                desc_obj,
                Some(1),
                reporter.clone(),
                created_time,
                None,
            ))
        });

    let (pending_code_changes, code_changes) =
        parse_v1_gerrit_changes_list(state.get("gerritChanges"));

    let url = format!("{}/issues/{}", web_base_url.trim_end_matches('/'), issue_id);

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

fn parse_v1_gerrit_change(obj: &serde_json::Map<String, Value>) -> Option<CodeChange> {
    let host = obj.get("host").and_then(Value::as_str)?.trim().to_string();
    let repo = obj
        .get("repo")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    let change_number = obj.get("changeNumber").and_then(as_i64)?;
    let state = match obj.get("state") {
        Some(Value::String(s)) => s.trim().to_string(),
        Some(v) => match as_i64(v) {
            Some(1) => "PENDING".to_string(),
            Some(2) => "MERGED".to_string(),
            _ => "STATE_UNSPECIFIED".to_string(),
        },
        None => "STATE_UNSPECIFIED".to_string(),
    };
    let branch = obj
        .get("branch")
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

fn parse_v1_gerrit_changes_list(v: Option<&Value>) -> (Vec<CodeChange>, Vec<CodeChange>) {
    let mut pending = Vec::new();
    let mut merged = Vec::new();
    if let Some(arr) = v.and_then(Value::as_array) {
        for item in arr {
            if let Some(obj) = item.as_object() {
                if let Some(gc) = parse_v1_gerrit_change(obj) {
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

fn parse_v1_comment_obj(
    comment_obj: &serde_json::Map<String, Value>,
    comment_number: Option<i32>,
    fallback_author: Option<String>,
    fallback_time: Option<DateTime<Utc>>,
    fallback_version: Option<i64>,
) -> CommentEntry {
    let body = comment_obj
        .get("comment")
        .and_then(Value::as_str)
        .unwrap_or("")
        .replace("\r\n", "\n");
    let last_editor = parse_user_obj(comment_obj.get("lastEditor"));
    let author = fallback_author
        .or(last_editor)
        .unwrap_or_else(|| "unknown".to_string());
    let modified_time = parse_rfc3339(comment_obj.get("modifiedTime"));
    let timestamp = fallback_time.or(modified_time);
    let formatting_mode = parse_formatting_mode_str(comment_obj.get("formattingMode"));
    let version = comment_obj
        .get("version")
        .and_then(as_i64)
        .or(fallback_version);

    CommentEntry {
        comment_number,
        author,
        timestamp,
        modified_time,
        body,
        formatting_mode,
        redacted: false,
        version,
        attachments: Vec::new(),
        field_updates: Vec::new(),
    }
}

/// Parses `Component` ProtoJSON from `GET /v1/components/{componentId}`.
pub fn parse_v1_component(root: &Value) -> Vec<String> {
    root.get("componentPathInfo")
        .and_then(|info| info.get("componentPathNames"))
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(ToOwned::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// Parses `ListIssuesResponse` ProtoJSON from `GET /v1/issues`.
pub fn parse_v1_search_response(
    root: &Value,
    query: &str,
    sort_by: &str,
    web_base_url: &str,
) -> Result<SearchIssuesResult> {
    check_v1_error(root)?;

    let mut issues = Vec::new();
    if let Some(arr) = root.get("issues").and_then(Value::as_array) {
        for item in arr {
            if let Ok((bundle, _)) = parse_v1_issue(item, web_base_url) {
                issues.push(bundle);
            }
        }
    }

    let next_page_token = root
        .get("nextPageToken")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned);
    let total_size = root
        .get("totalSize")
        .and_then(as_u64)
        .map(|n| n as usize)
        .unwrap_or(issues.len());
    let total_size_accurate = root
        .get("totalSizeAccurate")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    Ok(SearchIssuesResult {
        query: query.to_string(),
        sort_by: sort_by.to_string(),
        total_size,
        total_size_accurate,
        next_page_token,
        issues,
    })
}

/// Parses `ListIssueUpdatesResponse` ProtoJSON from `GET /v1/issues/{id}/issueUpdates`,
/// sharing `coalesce_issue_updates` with the JSPB parser.
pub fn parse_v1_updates(
    raw_updates: &[Value],
    issue_id: i64,
    usercontent_url: &str,
    custom_field_defs: &CustomFieldDefMap,
    include_field_updates: bool,
) -> Result<(Option<CommentEntry>, Vec<CommentEntry>, Vec<AttachmentMeta>)> {
    let mut parsed_updates: Vec<RawIssueUpdate> = Vec::with_capacity(raw_updates.len());

    for (idx, upd) in raw_updates.iter().enumerate() {
        let Some(upd_obj) = upd.as_object() else {
            continue;
        };

        let author = parse_user_obj(upd_obj.get("author")).unwrap_or_else(|| "unknown".to_string());
        let timestamp = parse_rfc3339(upd_obj.get("timestamp"));
        let comment_number = upd_obj
            .get("commentNumber")
            .and_then(as_i64)
            .map(|n| n as i32);
        let version = upd_obj.get("version").and_then(as_i64);
        let is_initial = idx == 0 || comment_number == Some(1) || version == Some(0);

        let comment = upd_obj
            .get("issueComment")
            .and_then(Value::as_object)
            .map(|comm_obj| {
                parse_v1_comment_obj(
                    comm_obj,
                    comment_number,
                    Some(author.clone()),
                    timestamp,
                    version,
                )
            });

        let mut attachments: Vec<AttachmentMeta> = Vec::new();
        if let Some(att_list) = upd_obj.get("attachments").and_then(Value::as_array) {
            for att_val in att_list {
                if let Some(att_obj) = att_val.as_object() {
                    if let Some(att) =
                        parse_v1_attachment(att_obj, issue_id, comment_number, usercontent_url)
                    {
                        attachments.push(att);
                    }
                }
            }
        }

        let mut field_diffs: Vec<FieldDiff> = Vec::new();
        if !is_initial {
            if let Some(f_list) = upd_obj.get("fieldUpdates").and_then(Value::as_array) {
                for f_val in f_list {
                    if let Some(f_obj) = f_val.as_object() {
                        if let Some(diff) = parse_v1_field_update(f_obj, custom_field_defs) {
                            field_diffs.push(diff);
                        }
                    }
                }
            }
        }

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

fn parse_v1_attachment(
    att_obj: &serde_json::Map<String, Value>,
    issue_id: i64,
    comment_number: Option<i32>,
    usercontent_url: &str,
) -> Option<AttachmentMeta> {
    let attachment_id = att_obj.get("attachmentId").and_then(as_i64)?;
    let content_type = att_obj
        .get("contentType")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .unwrap_or("application/octet-stream")
        .to_string();
    let size_bytes = att_obj.get("length").and_then(as_u64).unwrap_or(0);
    let raw_filename = att_obj
        .get("filename")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .unwrap_or("attachment.bin")
        .to_string();
    let is_deleted = att_obj
        .get("entityStatus")
        .and_then(|s| s.get("status"))
        .and_then(Value::as_str)
        .map(|s| s != "ACTIVE")
        .unwrap_or(false)
        || att_obj.get("attachmentDataRef").is_none();

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

fn parse_v1_field_update(
    f_obj: &serde_json::Map<String, Value>,
    custom_field_defs: &HashMap<i64, (String, String)>,
) -> Option<FieldDiff> {
    let field_name = f_obj.get("field").and_then(Value::as_str)?;
    if let Some(single) = f_obj.get("singleValueUpdate").and_then(Value::as_object) {
        let old_val = single.get("oldValue");
        let new_val = single.get("newValue");
        if field_name == "custom_fields" {
            let cf_obj = new_val.or(old_val).and_then(Value::as_object)?;
            let cf_id = cf_obj.get("customFieldId").and_then(as_i64)?;
            let label = custom_field_defs
                .get(&cf_id)
                .map(|(n, _)| n.clone())
                .unwrap_or_else(|| format!("custom_field_{}", cf_id));
            let old_s = old_val
                .and_then(Value::as_object)
                .map(extract_v1_custom_field_value)
                .unwrap_or_default();
            let new_s = new_val
                .and_then(Value::as_object)
                .map(extract_v1_custom_field_value)
                .unwrap_or_default();
            let summary = match (old_s.is_empty(), new_s.is_empty()) {
                (true, false) => format!("set to {}", new_s),
                (false, true) => format!("cleared (was {})", old_s),
                (false, false) => format!("{} -> {}", old_s, new_s),
                (true, true) => return None,
            };
            return Some(FieldDiff {
                field: label,
                summary,
            });
        }
        let old_s = format_v1_any(old_val, field_name);
        let new_s = format_v1_any(new_val, field_name);
        let summary = match (old_s, new_s) {
            (None, Some(n)) => format!("set to {}", n),
            (Some(o), None) => format!("cleared (was {})", o),
            (Some(o), Some(n)) => format!("{} -> {}", o, n),
            (None, None) => return None,
        };
        return Some(FieldDiff {
            field: field_name.to_string(),
            summary,
        });
    } else if let Some(coll) = f_obj.get("collectionUpdate").and_then(Value::as_object) {
        let added = coll
            .get("addedValues")
            .and_then(Value::as_array)
            .map(|list| {
                list.iter()
                    .filter_map(|v| format_v1_any(Some(v), field_name))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let removed = coll
            .get("removedValues")
            .and_then(Value::as_array)
            .map(|list| {
                list.iter()
                    .filter_map(|v| format_v1_any(Some(v), field_name))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        return build_collection_diff(field_name, &added, &removed);
    }
    None
}

fn format_v1_any(v: Option<&Value>, field_name: &str) -> Option<String> {
    let obj = v?.as_object()?;
    if let Some(email) = parse_user_obj(v) {
        return Some(email);
    }
    if let Some(val) = obj.get("value") {
        if let Some(s) = val.as_str() {
            if let Ok(code) = s.parse::<i64>() {
                return Some(map_field_int_code(field_name, code));
            }
            return Some(s.to_string());
        }
        if let Some(code) = as_i64(val) {
            return Some(map_field_int_code(field_name, code));
        }
        if let Some(b) = val.as_bool() {
            return Some(b.to_string());
        }
    }
    if obj.contains_key("changeNumber") {
        if let Some(gc) = parse_v1_gerrit_change(obj) {
            return Some(format!("{} ({})", gc.url, gc.state));
        }
    }
    None
}
