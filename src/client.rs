use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};
use std::fs::File;
use std::io::BufWriter;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use crate::cli::AuthMode;
use crate::jspb::{
    parse_component_response, parse_issue_fetch_response, parse_issue_search_response,
    parse_updates_response, parse_xssi_json,
};
use crate::models::{IssueBundle, SearchIssuesResult};
use crate::protojson::{
    parse_v1_component, parse_v1_issue, parse_v1_search_response, parse_v1_updates,
};

const CORP_API_BASE_URL: &str = "https://issuetracker.corp.googleapis.com/v1";
const SSO_CRED_HELPER_BIN: &str = "/usr/bin/sso-cred-helper";
const SSO_CLIENT_BIN: &str = "/usr/bin/sso_client";

#[derive(Clone)]
pub struct CrbugClient {
    agent: ureq::Agent,
    base_url: String,
    usercontent_url: String,
    cookie: Option<String>,
    corp_token: Option<String>,
}

impl CrbugClient {
    /// Creates a client using public unauthenticated access (`AuthMode::None`) unless a cookie is supplied.
    pub fn new(base_url: String, usercontent_url: String, cookie: Option<String>) -> Result<Self> {
        Self::new_with_auth(base_url, usercontent_url, cookie, AuthMode::None)
    }

    /// Creates a client configured with the specified `AuthMode` (`Auto`, `Corp`, or `None`).
    pub fn new_with_auth(
        base_url: String,
        usercontent_url: String,
        cookie: Option<String>,
        auth_mode: AuthMode,
    ) -> Result<Self> {
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(15))
            .timeout_read(Duration::from_secs(120))
            .timeout_write(Duration::from_secs(30))
            .user_agent(concat!(
                "crbugs-cli/",
                env!("CARGO_PKG_VERSION"),
                " (+https://issues.chromium.org)"
            ))
            .build();

        let cookie = cookie.and_then(|c| {
            let trimmed = c.trim().to_string();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed)
            }
        });

        let corp_token = resolve_corp_token(auth_mode)?;

        Ok(Self {
            agent,
            base_url: base_url.trim_end_matches('/').to_string(),
            usercontent_url: usercontent_url.trim_end_matches('/').to_string(),
            cookie,
            corp_token,
        })
    }

    /// Returns true if the client has active corp authentication (`sso-cred-helper` + `sso_client`).
    pub fn is_corp_authenticated(&self) -> bool {
        self.corp_token.is_some()
    }

    fn apply_common_headers(&self, req: ureq::Request) -> ureq::Request {
        let req = req.set("Accept", "application/json, */*");
        if let Some(ref c) = self.cookie {
            req.set("Cookie", c)
        } else {
            req
        }
    }

    /// Searches issues using `GET /v1/issues` (corp auth) or `POST /action/issues/list` (public).
    pub fn search_issues(
        &self,
        query: &str,
        sort_by: &str,
        limit: usize,
    ) -> Result<SearchIssuesResult> {
        if let Some(ref token) = self.corp_token {
            return self.search_issues_corp(token, query, sort_by, limit);
        }

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

    fn search_issues_corp(
        &self,
        token: &str,
        query: &str,
        sort_by: &str,
        limit: usize,
    ) -> Result<SearchIssuesResult> {
        let target_limit = limit.max(1);
        let mut all_issues = Vec::new();
        let mut page_token: Option<String> = None;
        let mut total_size = 0usize;
        let mut total_size_accurate = true;

        loop {
            let remaining = target_limit.saturating_sub(all_issues.len());
            if remaining == 0 {
                break;
            }
            let page_size = remaining.clamp(1, 250);

            let mut url = format!(
                "{}/issues?query={}&orderBy={}&pageSize={}&view=FULL",
                CORP_API_BASE_URL,
                url_encode_param(query),
                url_encode_param(sort_by),
                page_size
            );
            if let Some(ref tok) = page_token {
                if !tok.is_empty() {
                    url.push_str("&pageToken=");
                    url.push_str(&url_encode_param(tok));
                }
            }

            let val = self
                .sso_get_json(&url, token)
                .with_context(|| format!("Failed corp issue search for query '{}'", query))?;

            let page = parse_v1_search_response(&val, query, sort_by, &self.base_url)?;
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
        if let Some(ref token) = self.corp_token {
            return self.fetch_issue_bundle_corp(
                token,
                issue_id,
                include_field_updates,
                max_comments,
            );
        }

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

    fn fetch_issue_bundle_corp(
        &self,
        token: &str,
        issue_id: i64,
        include_field_updates: bool,
        max_comments: Option<usize>,
    ) -> Result<IssueBundle> {
        let (issue_res, updates_res) = std::thread::scope(|s| {
            let issue_handle = s.spawn(|| self.fetch_issue_corp(token, issue_id));
            let updates_handle = s.spawn(|| self.fetch_all_updates_corp(token, issue_id));
            (
                issue_handle
                    .join()
                    .expect("corp issue fetch thread panicked"),
                updates_handle
                    .join()
                    .expect("corp updates fetch thread panicked"),
            )
        });

        let issue_json = issue_res?;
        let updates_list = updates_res?;

        let (mut bundle, custom_field_defs) = parse_v1_issue(&issue_json, &self.base_url)?;

        if bundle.component_id > 0 {
            if let Ok(comp_json) = self.fetch_component_corp(token, bundle.component_id) {
                bundle.component_path = parse_v1_component(&comp_json);
            }
        }

        let (desc_from_updates, mut comments, attachments) = parse_v1_updates(
            &updates_list,
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

    fn fetch_issue_corp(&self, token: &str, issue_id: i64) -> Result<Value> {
        let url = format!("{}/issues/{}?view=FULL", CORP_API_BASE_URL, issue_id);
        self.sso_get_json(&url, token)
            .with_context(|| format!("Failed to fetch issue {} via corp API", issue_id))
    }

    fn fetch_all_updates_corp(&self, token: &str, issue_id: i64) -> Result<Vec<Value>> {
        let mut all_updates = Vec::new();
        let mut page_token: Option<String> = None;

        loop {
            let mut url = format!(
                "{}/issues/{}/issueUpdates?sortBy=ASC&pageSize=500",
                CORP_API_BASE_URL, issue_id
            );
            if let Some(ref tok) = page_token {
                if !tok.is_empty() {
                    url.push_str("&pageToken=");
                    url.push_str(&url_encode_param(tok));
                }
            }

            let val = self.sso_get_json(&url, token).with_context(|| {
                format!("Failed to fetch updates for issue {} via corp API", issue_id)
            })?;

            let count = if let Some(arr) = val.get("issueUpdates").and_then(Value::as_array) {
                all_updates.extend(arr.iter().cloned());
                arr.len()
            } else {
                0
            };

            page_token = val
                .get("nextPageToken")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string);

            if count == 0 || page_token.is_none() {
                break;
            }
        }

        Ok(all_updates)
    }

    fn fetch_component_corp(&self, token: &str, component_id: i64) -> Result<Value> {
        let url = format!("{}/components/{}", CORP_API_BASE_URL, component_id);
        self.sso_get_json(&url, token)
    }

    fn sso_get_json(&self, url: &str, token: &str) -> Result<Value> {
        match self.sso_get_json_once(url, token) {
            Ok(val) => Ok(val),
            Err(err) if err.to_string().contains("HTTP 401") => {
                // Self-heal if a cached token was revoked or expired early
                invalidate_cached_token();
                if let Ok(Some(fresh_token)) = mint_corp_token(AuthMode::Corp) {
                    write_cached_token(&fresh_token);
                    return self.sso_get_json_once(url, &fresh_token);
                }
                Err(err)
            }
            Err(err) => Err(err),
        }
    }

    fn sso_get_json_once(&self, url: &str, token: &str) -> Result<Value> {
        let auth_header = format!("Authorization: Bearer {}", token);
        let output = Command::new(SSO_CLIENT_BIN)
            .arg(format!("--url={}", url))
            .arg(format!("--headers={}", auth_header))
            .arg("--location")
            .arg("--connect_timeout=15")
            .arg("--request_timeout=60")
            .output()
            .with_context(|| format!("Failed to execute {}", SSO_CLIENT_BIN))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            return Err(anyhow!(
                "sso_client failed for {} ({}): {} {}",
                url,
                output.status,
                stderr.trim(),
                stdout.chars().take(200).collect::<String>()
            ));
        }

        let text = String::from_utf8_lossy(&output.stdout);
        let val = parse_xssi_json(&text).map_err(|e| {
            anyhow!(
                "Invalid JSON from corp API {}: {} ({})",
                url,
                e,
                text.chars().take(300).collect::<String>()
            )
        })?;

        if let Some(err_obj) = val.get("error") {
            let code = err_obj.get("code").and_then(Value::as_i64).unwrap_or(0);
            let status = err_obj
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("UNKNOWN");
            let message = err_obj
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("Unknown API error");
            return Err(anyhow!(
                "Corp API error (HTTP {} {}): {}",
                code,
                status,
                message
            ));
        }

        Ok(val)
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
        if let Some(ref token) = self.corp_token {
            return self.download_attachment_corp(token, issue_id, attachment_id, dest_path);
        }

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

    fn download_attachment_corp(
        &self,
        token: &str,
        issue_id: i64,
        attachment_id: i64,
        dest_path: &Path,
    ) -> Result<u64> {
        let media_url = format!(
            "{}/media/attachment:{}:{}?alt=media",
            CORP_API_BASE_URL, issue_id, attachment_id
        );
        let auth_header = format!("Authorization: Bearer {}", token);

        let tmp_path = dest_path.with_extension("part");
        let file = File::create(&tmp_path)
            .with_context(|| format!("Failed to create temporary file {}", tmp_path.display()))?;
        let mut writer = BufWriter::new(file);

        let mut child = Command::new(SSO_CLIENT_BIN)
            .arg(format!("--url={}", media_url))
            .arg(format!("--headers={}", auth_header))
            .arg("--location")
            .arg("--connect_timeout=15")
            .arg("--request_timeout=120")
            .arg("--expect_http_code=200")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("Failed to spawn {} for attachment download", SSO_CLIENT_BIN))?;

        let mut child_stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("Failed to capture stdout of sso_client"))?;

        let written = std::io::copy(&mut child_stdout, &mut writer)
            .context("Error streaming attachment bytes from sso_client")?;
        std::io::Write::flush(&mut writer)?;
        drop(writer);

        let output = child
            .wait_with_output()
            .context("Failed waiting for sso_client attachment download")?;

        if !output.status.success() {
            let _ = std::fs::remove_file(&tmp_path);
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!(
                "Failed to download attachment {} for issue {} via corp API: {}",
                attachment_id,
                issue_id,
                stderr.trim()
            ));
        }

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

const TOKEN_CACHE_TTL_SECS: u64 = 55 * 60;

fn current_unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn token_cache_path() -> std::path::PathBuf {
    let raw_user = std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .unwrap_or_else(|_| "default".to_string());
    let safe_user: String = raw_user
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    std::env::temp_dir().join(format!("crbugs_sso_token_{}.json", safe_user))
}

fn read_cached_token() -> Option<String> {
    let path = token_cache_path();
    let meta = std::fs::metadata(&path).ok()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o077 != 0 {
            return None;
        }
    }
    let data = std::fs::read(&path).ok()?;
    let val = serde_json::from_slice::<Value>(&data).ok()?;
    let expires_at = val.get("expires_at_unix").and_then(Value::as_u64)?;
    if current_unix_secs() >= expires_at {
        return None;
    }
    val.get("token")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn write_cached_token(token: &str) {
    let path = token_cache_path();
    let tmp_path = path.with_extension(format!("part.{}", std::process::id()));
    let payload = json!({
        "token": token,
        "expires_at_unix": current_unix_secs() + TOKEN_CACHE_TTL_SECS,
    });
    let Ok(bytes) = serde_json::to_vec(&payload) else {
        return;
    };

    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }

    if let Ok(mut file) = opts.open(&tmp_path) {
        if std::io::Write::write_all(&mut file, &bytes).is_ok()
            && std::io::Write::flush(&mut file).is_ok()
        {
            drop(file);
            let _ = std::fs::rename(&tmp_path, &path);
            return;
        }
    }
    let _ = std::fs::remove_file(&tmp_path);
}

fn invalidate_cached_token() {
    let _ = std::fs::remove_file(token_cache_path());
}

fn resolve_corp_token(auth_mode: AuthMode) -> Result<Option<String>> {
    if auth_mode == AuthMode::None {
        return Ok(None);
    }

    if !Path::new(SSO_CRED_HELPER_BIN).exists() || !Path::new(SSO_CLIENT_BIN).exists() {
        return match auth_mode {
            AuthMode::Corp => Err(anyhow!(
                "Corp authentication requested (--auth=corp), but {} or {} was not found",
                SSO_CRED_HELPER_BIN,
                SSO_CLIENT_BIN
            )),
            AuthMode::Auto | AuthMode::None => Ok(None),
        };
    }

    if let Some(cached) = read_cached_token() {
        return Ok(Some(cached));
    }

    let minted = mint_corp_token(auth_mode)?;
    if let Some(ref tok) = minted {
        write_cached_token(tok);
    }
    Ok(minted)
}

fn mint_corp_token(auth_mode: AuthMode) -> Result<Option<String>> {
    let output = Command::new(SSO_CRED_HELPER_BIN)
        .arg("-force")
        .arg("-scopes=https://www.googleapis.com/auth/buganizer")
        .output();

    match output {
        Ok(out) if out.status.success() => {
            if let Ok(val) = serde_json::from_slice::<Value>(&out.stdout) {
                if let Some(tok) = val
                    .get("token")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                {
                    return Ok(Some(tok.to_string()));
                }
            }
            match auth_mode {
                AuthMode::Corp => Err(anyhow!(
                    "Corp authentication failed: invalid token JSON from {}",
                    SSO_CRED_HELPER_BIN
                )),
                AuthMode::Auto | AuthMode::None => Ok(None),
            }
        }
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            match auth_mode {
                AuthMode::Corp => Err(anyhow!(
                    "Corp authentication failed (try running `gcert`): {}",
                    stderr.trim()
                )),
                AuthMode::Auto | AuthMode::None => Ok(None),
            }
        }
        Err(e) => match auth_mode {
            AuthMode::Corp => Err(anyhow!(
                "Failed to execute {} (try running `gcert`): {}",
                SSO_CRED_HELPER_BIN,
                e
            )),
            AuthMode::Auto | AuthMode::None => Ok(None),
        },
    }
}

fn url_encode_param(input: &str) -> String {
    let mut encoded = String::with_capacity(input.len() * 2);
    for b in input.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(b as char);
            }
            _ => {
                encoded.push_str(&format!("%{:02X}", b));
            }
        }
    }
    encoded
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
