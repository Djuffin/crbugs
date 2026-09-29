use anyhow::{anyhow, Context, Result};
use futures::StreamExt;
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, CONTENT_TYPE, COOKIE, ORIGIN, USER_AGENT};
use serde_json::{json, Value};
use std::path::Path;
use std::time::Duration;
use tokio::io::AsyncWriteExt;

use crate::jspb::{
    parse_component_response, parse_issue_fetch_response, parse_updates_response, parse_xssi_json,
};
use crate::models::IssueBundle;

#[derive(Clone)]
pub struct CrbugClient {
    http: reqwest::Client,
    base_url: String,
    usercontent_url: String,
}

impl CrbugClient {
    pub fn new(base_url: String, usercontent_url: String, cookie: Option<String>) -> Result<Self> {
        let mut default_headers = HeaderMap::new();
        default_headers.insert(
            USER_AGENT,
            HeaderValue::from_static("crbugs-cli/0.1 (+https://issues.chromium.org)"),
        );
        default_headers.insert(ACCEPT, HeaderValue::from_static("application/json, */*"));

        if let Some(ref c) = cookie {
            if !c.trim().is_empty() {
                let val = HeaderValue::from_str(c.trim())
                    .context("Invalid characters in --cookie header value")?;
                default_headers.insert(COOKIE, val);
            }
        }

        let http = reqwest::Client::builder()
            .default_headers(default_headers)
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(120))
            .build()
            .context("Failed to build HTTP client")?;

        Ok(Self {
            http,
            base_url: base_url.trim_end_matches('/').to_string(),
            usercontent_url: usercontent_url.trim_end_matches('/').to_string(),
        })
    }

    /// Fetches the complete `IssueBundle` (issue state, custom fields, component hierarchy,
    /// description, comments, and attachment metadata) for a single issue ID.
    pub async fn fetch_issue_bundle(
        &self,
        issue_id: i64,
        include_field_updates: bool,
        max_comments: Option<usize>,
    ) -> Result<IssueBundle> {
        let (issue_json, updates_json) = tokio::try_join!(
            self.fetch_issue_raw(issue_id),
            self.fetch_updates_raw(issue_id)
        )?;

        let (mut bundle, custom_field_defs) =
            parse_issue_fetch_response(&issue_json, &self.base_url)?;

        if bundle.component_id > 0 {
            if let Ok(comp_json) = self.fetch_component_raw(bundle.component_id).await {
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

    pub async fn fetch_issue_raw(&self, issue_id: i64) -> Result<Value> {
        let url = format!("{}/action/issues/{}", self.base_url, issue_id);
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .with_context(|| format!("GET {} failed", url))?;
        let status = resp.status();
        let text = resp
            .text()
            .await
            .with_context(|| format!("Failed reading body from {}", url))?;

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

    pub async fn fetch_updates_raw(&self, issue_id: i64) -> Result<Value> {
        let url = format!("{}/action/issues/{}/updates", self.base_url, issue_id);
        // JSPB serialization of ListIssueUpdatesRequest:
        // [issue_id (1), sort_by (2), page_size (3), page_token (4), null (5), issue_comment_view=FULL (6)]
        let body = json!([issue_id.to_string(), "ASC", 0, "", null, 1]);

        let resp = self
            .http
            .post(&url)
            .header(CONTENT_TYPE, "application/json")
            .json(&body)
            .send()
            .await
            .with_context(|| format!("POST {} failed", url))?;
        let status = resp.status();
        let text = resp
            .text()
            .await
            .with_context(|| format!("Failed reading body from {}", url))?;

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

    pub async fn fetch_component_raw(&self, component_id: i64) -> Result<Value> {
        let url = format!("{}/action/components/{}", self.base_url, component_id);
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .with_context(|| format!("GET {} failed", url))?;
        let text = resp.text().await?;
        parse_xssi_json(&text)
    }

    /// Streams an attachment to `dest_path` using an atomic `.part` temporary file.
    pub async fn download_attachment_to_path(
        &self,
        issue_id: i64,
        attachment_id: i64,
        dest_path: &Path,
    ) -> Result<u64> {
        let primary_url = format!(
            "{}/download/attachment/{}/{}?download=true",
            self.usercontent_url, issue_id, attachment_id
        );

        let resp = self
            .http
            .get(&primary_url)
            .header("Sec-Fetch-Mode", "cors")
            .header(ORIGIN, &self.base_url)
            .send()
            .await
            .with_context(|| format!("GET {} failed", primary_url))?;

        if !resp.status().is_success() {
            return Err(anyhow!(
                "HTTP {} when downloading attachment {} for issue {}",
                resp.status(),
                attachment_id,
                issue_id
            ));
        }

        let tmp_path = dest_path.with_extension("part");
        let mut file = tokio::fs::File::create(&tmp_path)
            .await
            .with_context(|| format!("Failed to create temporary file {}", tmp_path.display()))?;

        let mut stream = resp.bytes_stream();
        let mut written: u64 = 0;

        while let Some(chunk_res) = stream.next().await {
            let chunk = chunk_res.context("Error reading attachment byte stream")?;
            file.write_all(&chunk).await?;
            written += chunk.len() as u64;
        }
        file.flush().await?;
        drop(file);

        tokio::fs::rename(&tmp_path, dest_path)
            .await
            .with_context(|| {
                format!(
                    "Failed to rename {} to {}",
                    tmp_path.display(),
                    dest_path.display()
                )
            })?;

        Ok(written)
    }
}
