use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};
use std::fs::File;
use std::io::BufWriter;
use std::path::Path;
use std::time::Duration;

use crate::jspb::{
    parse_component_response, parse_issue_fetch_response, parse_issue_search_response,
    parse_updates_response, parse_xssi_json,
};
use crate::models::{IssueBundle, SearchIssuesResult};

#[derive(Clone)]
pub struct CrbugClient {
    agent: ureq::Agent,
    base_url: String,
    usercontent_url: String,
    cookie: Option<String>,
}

impl CrbugClient {
    pub fn new(base_url: String, usercontent_url: String, cookie: Option<String>) -> Result<Self> {
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(15))
            .timeout_read(Duration::from_secs(120))
            .timeout_write(Duration::from_secs(30))
            .user_agent("crbugs-cli/0.1 (+https://issues.chromium.org)")
            .build();

        let cookie = cookie.and_then(|c| {
            let trimmed = c.trim().to_string();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed)
            }
        });

        Ok(Self {
            agent,
            base_url: base_url.trim_end_matches('/').to_string(),
            usercontent_url: usercontent_url.trim_end_matches('/').to_string(),
            cookie,
        })
    }

    fn apply_common_headers(&self, req: ureq::Request) -> ureq::Request {
        let req = req.set("Accept", "application/json, */*");
        if let Some(ref c) = self.cookie {
            req.set("Cookie", c)
        } else {
            req
        }
    }

    /// Searches issues using `POST /action/issues/list` (`b.IssueSearchResponse`).
    pub fn search_issues(
        &self,
        query: &str,
        sort_by: &str,
        limit: usize,
    ) -> Result<SearchIssuesResult> {
        let url = format!("{}/action/issues/list", self.base_url);
        let target_limit = limit.max(1);
        let mut all_issues = Vec::new();
        let mut page_token: Option<String> = None;
        let mut total_size = 0usize;
        let mut total_size_accurate = false;

        loop {
            let remaining = target_limit.saturating_sub(all_issues.len());
            if remaining == 0 {
                break;
            }
            let page_size = remaining.clamp(1, 500) as i32;

            // JSPB serialization of IssueListRequest:
            // [null, null, null, null, null, tracker_ids (6), ListIssuesRequest (7)]
            // ListIssuesRequest: [query (1), order_by (2), page_size (3), page_token (4)]
            let it_req = match page_token.as_deref() {
                Some(tok) if !tok.is_empty() => json!([query, sort_by, page_size, tok]),
                _ => json!([query, sort_by, page_size]),
            };
            let body = json!([null, null, null, null, null, null, it_req]).to_string();

            let req = self
                .apply_common_headers(self.agent.post(&url))
                .set("Content-Type", "application/json");
            let (status, text) = send_string_and_read_text(req, &body, &url)?;

            let val = parse_xssi_json(&text).map_err(|e| {
                anyhow!(
                    "Failed to search issues for query '{}' (HTTP {}): {} ({})",
                    query,
                    status,
                    e,
                    text.chars().take(300).collect::<String>()
                )
            })?;

            let page = parse_issue_search_response(&val, query, sort_by, &self.base_url)?;
            total_size = page.total_size;
            total_size_accurate = page.total_size_accurate;
            let fetched_count = page.issues.len();
            all_issues.extend(page.issues);
            page_token = page.next_page_token;

            if fetched_count == 0 || page_token.is_none() || all_issues.len() >= target_limit {
                break;
            }
        }

        all_issues.truncate(target_limit);
        if total_size < all_issues.len() {
            total_size = all_issues.len();
        }

        Ok(SearchIssuesResult {
            query: query.to_string(),
            sort_by: sort_by.to_string(),
            total_size,
            total_size_accurate,
            next_page_token: page_token,
            issues: all_issues,
        })
    }

    /// Fetches the complete `IssueBundle` (issue state, custom fields, component hierarchy,
    /// description, comments, and attachment metadata) for a single issue ID.
    pub fn fetch_issue_bundle(
        &self,
        issue_id: i64,
        include_field_updates: bool,
        max_comments: Option<usize>,
    ) -> Result<IssueBundle> {
        // Fetch issue state and updates concurrently using scoped OS threads
        let (issue_res, updates_res) = std::thread::scope(|s| {
            let issue_handle = s.spawn(|| self.fetch_issue_raw(issue_id));
            let updates_handle = s.spawn(|| self.fetch_updates_raw(issue_id));
            (
                issue_handle.join().expect("issue fetch thread panicked"),
                updates_handle
                    .join()
                    .expect("updates fetch thread panicked"),
            )
        });

        let issue_json = issue_res?;
        let updates_json = updates_res?;

        let (mut bundle, custom_field_defs) =
            parse_issue_fetch_response(&issue_json, &self.base_url)?;

        if bundle.component_id > 0 {
            if let Ok(comp_json) = self.fetch_component_raw(bundle.component_id) {
                bundle.component_path = parse_component_response(&comp_json);
            }
        }

        let (desc_from_updates, mut comments, attachments) = parse_updates_response(
            &updates_json,
            issue_id,
            &self.usercontent_url,
            &custom_field_defs,
            include_field_updates,
        )?;

        if desc_from_updates.is_some() {
            bundle.description = desc_from_updates;
        }

        if let Some(limit) = max_comments {
            if limit > 0 && comments.len() > limit {
                let start = comments.len() - limit;
                comments = comments.split_off(start);
            }
        }

        bundle.comments = comments;
        bundle.attachments = attachments;

        Ok(bundle)
    }

    pub fn fetch_issue_raw(&self, issue_id: i64) -> Result<Value> {
        let url = format!("{}/action/issues/{}", self.base_url, issue_id);
        let req = self.apply_common_headers(self.agent.get(&url));
        let (status, text) = send_and_read_text(req, &url)?;

        if let Ok(val) = parse_xssi_json(&text) {
            return Ok(val);
        }
        Err(anyhow!(
            "Failed to fetch issue {} (HTTP {}): {}",
            issue_id,
            status,
            text.chars().take(300).collect::<String>()
        ))
    }

    pub fn fetch_updates_raw(&self, issue_id: i64) -> Result<Value> {
        let url = format!("{}/action/issues/{}/updates", self.base_url, issue_id);
        // JSPB serialization of ListIssueUpdatesRequest:
        // [issue_id (1), sort_by (2), page_size (3), page_token (4), null (5), issue_comment_view=FULL (6)]
        let body = json!([issue_id.to_string(), "ASC", 0, "", null, 1]).to_string();

        let req = self
            .apply_common_headers(self.agent.post(&url))
            .set("Content-Type", "application/json");
        let (status, text) = send_string_and_read_text(req, &body, &url)?;

        if let Ok(val) = parse_xssi_json(&text) {
            return Ok(val);
        }
        Err(anyhow!(
            "Failed to fetch updates for issue {} (HTTP {}): {}",
            issue_id,
            status,
            text.chars().take(300).collect::<String>()
        ))
    }

    pub fn fetch_component_raw(&self, component_id: i64) -> Result<Value> {
        let url = format!("{}/action/components/{}", self.base_url, component_id);
        let req = self.apply_common_headers(self.agent.get(&url));
        let (_status, text) = send_and_read_text(req, &url)?;
        parse_xssi_json(&text)
    }

    /// Streams an attachment to `dest_path` using an atomic `.part` temporary file.
    pub fn download_attachment_to_path(
        &self,
        issue_id: i64,
        attachment_id: i64,
        dest_path: &Path,
    ) -> Result<u64> {
        let primary_url = format!(
            "{}/download/attachment/{}/{}?download=true",
            self.usercontent_url, issue_id, attachment_id
        );

        let req = self
            .apply_common_headers(self.agent.get(&primary_url))
            .set("Sec-Fetch-Mode", "cors")
            .set("Origin", &self.base_url);

        let resp = req.call().map_err(|e| match e {
            ureq::Error::Status(code, _) => anyhow!(
                "HTTP {} when downloading attachment {} for issue {}",
                code,
                attachment_id,
                issue_id
            ),
            ureq::Error::Transport(t) => {
                anyhow!("Transport error downloading {}: {}", primary_url, t)
            }
        })?;

        let tmp_path = dest_path.with_extension("part");
        let file = File::create(&tmp_path)
            .with_context(|| format!("Failed to create temporary file {}", tmp_path.display()))?;
        let mut writer = BufWriter::new(file);
        let mut reader = resp.into_reader();

        let written = std::io::copy(&mut reader, &mut writer)
            .context("Error reading attachment byte stream")?;
        std::io::Write::flush(&mut writer)?;
        drop(writer);

        std::fs::rename(&tmp_path, dest_path).with_context(|| {
            format!(
                "Failed to rename {} to {}",
                tmp_path.display(),
                dest_path.display()
            )
        })?;

        Ok(written)
    }
}

fn send_and_read_text(req: ureq::Request, url: &str) -> Result<(u16, String)> {
    match req.call() {
        Ok(resp) => {
            let status = resp.status();
            let text = resp
                .into_string()
                .with_context(|| format!("Failed reading body from {}", url))?;
            Ok((status, text))
        }
        Err(ureq::Error::Status(code, resp)) => {
            let text = resp.into_string().unwrap_or_default();
            Ok((code, text))
        }
        Err(ureq::Error::Transport(t)) => Err(anyhow!("HTTP request to {} failed: {}", url, t)),
    }
}

fn send_string_and_read_text(req: ureq::Request, body: &str, url: &str) -> Result<(u16, String)> {
    match req.send_string(body) {
        Ok(resp) => {
            let status = resp.status();
            let text = resp
                .into_string()
                .with_context(|| format!("Failed reading body from {}", url))?;
            Ok((status, text))
        }
        Err(ureq::Error::Status(code, resp)) => {
            let text = resp.into_string().unwrap_or_default();
            Ok((code, text))
        }
        Err(ureq::Error::Transport(t)) => Err(anyhow!("HTTP POST to {} failed: {}", url, t)),
    }
}
