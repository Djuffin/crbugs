use anyhow::{anyhow, Result};
use clap::{Parser, ValueEnum};
use regex::Regex;
use std::path::PathBuf;

const LONG_ABOUT: &str = "\
crbugs — Fetch or search Chromium issues on https://issues.chromium.org/ and export to Markdown or JSON.

Features:
  • Fetch a single issue by ID or URL: title, description, status, priority, severity, type,
    reporter, assignee, CCs, component hierarchy, hotlists, blocking/blocked-by links, and
    Chromium custom fields.
  • Search issues by assignee (--assignee / -u), reporter (--reporter), CC (--cc), status
    (--status / -s: fixed, assigned, accepted, new, open, closed, verified, etc.), or raw query.
  • Reads the full comment thread in chronological order, coalescing attachment uploads
    with their associated comments.
  • Downloads binary attachments concurrently with safe filename sanitization.
  • Works out-of-the-box without authentication for public Chromium issues.";

const AFTER_LONG_HELP: &str = "\
EXAMPLES:
  1. Export an issue and all its attachments to ./crbug_563075803/:
     $ crbugs 563075803

  2. Print Markdown to stdout without downloading binary attachments:
     $ crbugs 563075803 --stdout --skip-attachments

  3. Fetch using a full URL or crbug.com shorthand:
     $ crbugs https://issues.chromium.org/issues/563075803
     $ crbugs https://crbug.com/563075803

  4. Search for issues assigned to a user by email:
     $ crbugs --assignee eugene@chromium.org
     $ crbugs eugene@chromium.org

  5. Search for issues assigned to a user filtered by status (fixed, assigned, accepted, open, etc.):
     $ crbugs --assignee eugene@chromium.org --status fixed
     $ crbugs -u eugene@chromium.org -s assigned,accepted --limit 20

  6. Emit structured JSON for an issue or search query:
     $ crbugs 563075803 --stdout --format json --skip-attachments
     $ crbugs -u eugene@chromium.org -s fixed --format json

  7. Limit output to the initial description plus the last 10 comments:
     $ crbugs 563075803 --stdout --max-comments 10

  8. Include field-change history (status, assignee, component, label diffs) in the timeline:
     $ crbugs 563075803 --include-field-updates

OUTPUT LAYOUT (DEFAULT SINGLE-ISSUE FILE MODE):
  ./crbug_<ISSUE_ID>/
  ├── issue_<ISSUE_ID>.md                  # YAML frontmatter + metadata table + description + comments
  └── attachments/
      └── <ATTACHMENT_ID>_<FILENAME>       # Downloaded attachments linked relatively from the .md file";

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum OutputFormat {
    /// Self-contained Markdown with YAML frontmatter, metadata table, and comment thread
    Markdown,
    /// Structured JSON representation of the issue bundle or search results
    Json,
}

#[derive(Debug, Parser)]
#[command(
    name = "crbugs",
    version,
    about = "Fetch or search Chromium issues from issues.chromium.org into Markdown or JSON",
    long_about = LONG_ABOUT,
    after_long_help = AFTER_LONG_HELP,
    arg_required_else_help = true
)]
pub struct Cli {
    /// Issue identifier (e.g. 563075803, https://crbug.com/563075803, b/563075803)
    /// or user email to search for assigned issues (e.g. eugene@chromium.org)
    #[arg(value_name = "ISSUE_OR_USER")]
    pub issue: Option<String>,

    /// Search for issues assigned to a user by email (e.g. eugene@chromium.org)
    #[arg(
        short = 'u',
        long = "assignee",
        visible_alias = "user",
        value_name = "EMAIL"
    )]
    pub assignee: Option<String>,

    /// Search for issues reported by a user by email
    #[arg(long = "reporter", value_name = "EMAIL")]
    pub reporter: Option<String>,

    /// Search for issues where a user is CC'd by email
    #[arg(long = "cc", value_name = "EMAIL")]
    pub cc: Option<String>,

    /// Filter search results by status (comma-separated or repeated):
    /// open, closed, new, assigned, accepted (or in_progress), fixed, verified,
    /// not_reproducible, intended_behavior, obsolete, infeasible, duplicate, wontfix
    #[arg(
        short = 's',
        long = "status",
        value_delimiter = ',',
        value_name = "STATUS"
    )]
    pub status: Vec<String>,

    /// Raw Issue Tracker search query string (can be combined with --assignee and --status)
    #[arg(short = 'Q', long = "query", value_name = "QUERY")]
    pub query: Option<String>,

    /// Maximum number of issues to return when searching
    #[arg(short = 'l', long = "limit", default_value_t = 25, value_name = "N")]
    pub limit: usize,

    /// Sort order for search results (e.g. "modified_time desc", "created_time desc", "priority asc")
    #[arg(
        long = "sort",
        default_value = "modified_time desc",
        value_name = "ORDER"
    )]
    pub sort: String,

    /// Output file path (use '-' for stdout).
    /// Single issue defaults to ./crbug_<id>/issue_<id>.md; search defaults to stdout unless -o is set
    #[arg(short = 'o', long = "output", value_name = "PATH")]
    pub output: Option<PathBuf>,

    /// Print the rendered Markdown or JSON directly to stdout instead of writing a file
    #[arg(long = "stdout")]
    pub stdout: bool,

    /// Output format: markdown or json
    #[arg(
        short = 'f',
        long = "format",
        value_enum,
        default_value_t = OutputFormat::Markdown,
        value_name = "FORMAT"
    )]
    pub format: OutputFormat,

    /// Directory to save downloaded attachments
    /// [default: <output_dir>/attachments or ./crbug_<id>/attachments]
    #[arg(short = 'a', long = "attachments-dir", value_name = "DIR")]
    pub attachments_dir: Option<PathBuf>,

    /// Do not download attachment binary files (attachment metadata & URLs are still included)
    #[arg(long = "skip-attachments", visible_alias = "no-attachments")]
    pub skip_attachments: bool,

    /// Skip downloading attachments larger than this size in bytes [default: 100 MiB]
    #[arg(
        long = "max-attachment-size",
        default_value_t = 100 * 1024 * 1024,
        value_name = "BYTES"
    )]
    pub max_attachment_size: u64,

    /// Limit output to the initial description plus the last N comments (0 = all comments)
    #[arg(long = "max-comments", value_name = "N")]
    pub max_comments: Option<usize>,

    /// Include field-change diffs (status, priority, component, custom fields) in the comment timeline
    #[arg(long = "include-field-updates")]
    pub include_field_updates: bool,

    /// Maximum concurrent attachment downloads
    #[arg(short = 'j', long = "concurrency", default_value_t = 4, value_name = "N")]
    pub concurrency: usize,

    /// Base URL for the Issue Tracker web frontend
    #[arg(
        long = "base-url",
        default_value = "https://issues.chromium.org",
        env = "CRBUGS_BASE_URL",
        hide_short_help = true
    )]
    pub base_url: String,

    /// Base URL for attachment binary downloads
    #[arg(
        long = "usercontent-url",
        default_value = "https://usercontent.issues.chromium.org",
        env = "CRBUGS_USERCONTENT_URL",
        hide_short_help = true
    )]
    pub usercontent_url: String,

    /// Optional HTTP Cookie header for restricted/embargoed issues
    #[arg(long = "cookie", env = "CRBUGS_COOKIE", value_name = "COOKIE", hide_env_values = true)]
    pub cookie: Option<String>,

    /// Suppress progress and summary messages on stderr
    #[arg(short = 'q', long = "quiet")]
    pub quiet: bool,
}

impl Cli {
    /// Returns `Some(query_string)` if the CLI was invoked in search mode, or `None` for single-issue mode.
    pub fn build_search_query(&self) -> Result<Option<String>> {
        let mut parts: Vec<String> = Vec::new();

        // Check if the positional argument is an email address or raw search query
        let mut positional_assignee: Option<&str> = None;
        let mut positional_query: Option<&str> = None;

        if let Some(ref raw_target) = self.issue {
            let trimmed = raw_target.trim();
            if parse_issue_id(trimmed).is_err() {
                if trimmed.contains('@') && !trimmed.contains(':') && !trimmed.contains(' ') {
                    positional_assignee = Some(trimmed);
                } else if trimmed.contains(':') {
                    positional_query = Some(trimmed);
                }
            }
        }

        let has_search_flags = self.assignee.is_some()
            || self.reporter.is_some()
            || self.cc.is_some()
            || !self.status.is_empty()
            || self.query.is_some()
            || positional_assignee.is_some()
            || positional_query.is_some();

        if !has_search_flags {
            return Ok(None);
        }

        if let Some(ref email) = self.assignee {
            let trimmed = email.trim();
            if !trimmed.is_empty() {
                parts.push(format!("assignee:{}", trimmed));
            }
        } else if let Some(email) = positional_assignee {
            parts.push(format!("assignee:{}", email));
        }

        if let Some(ref email) = self.reporter {
            let trimmed = email.trim();
            if !trimmed.is_empty() {
                parts.push(format!("reporter:{}", trimmed));
            }
        }

        if let Some(ref email) = self.cc {
            let trimmed = email.trim();
            if !trimmed.is_empty() {
                parts.push(format!("cc:{}", trimmed));
            }
        }

        if !self.status.is_empty() {
            parts.push(build_status_filter(&self.status)?);
        }

        if let Some(ref q) = self.query {
            let trimmed = q.trim();
            if !trimmed.is_empty() {
                parts.push(trimmed.to_string());
            }
        }

        if let Some(q) = positional_query {
            parts.push(q.to_string());
        }

        if parts.is_empty() {
            return Err(anyhow!("Search query cannot be empty"));
        }

        Ok(Some(parts.join(" ")))
    }

    /// Returns true if the primary document should be written to stdout.
    pub fn writes_to_stdout(&self) -> bool {
        self.stdout
            || self
                .output
                .as_ref()
                .map(|p| p.as_os_str() == "-")
                .unwrap_or(false)
    }

    /// Resolves the target output file path for a single issue (`None` when writing to stdout).
    pub fn resolved_output_path(&self, issue_id: i64) -> Option<PathBuf> {
        if self.writes_to_stdout() {
            return None;
        }
        if let Some(ref path) = self.output {
            return Some(path.clone());
        }
        let ext = match self.format {
            OutputFormat::Markdown => "md",
            OutputFormat::Json => "json",
        };
        Some(
            PathBuf::from(format!("crbug_{}", issue_id))
                .join(format!("issue_{}.{}", issue_id, ext)),
        )
    }

    /// Resolves the target output file path for search results (`None` when writing to stdout, which is default for search).
    pub fn resolved_search_output_path(&self) -> Option<PathBuf> {
        if self.writes_to_stdout() {
            return None;
        }
        self.output.clone()
    }

    /// Resolves the directory where attachments should be stored on disk.
    pub fn resolved_attachments_dir(&self, issue_id: i64) -> PathBuf {
        if let Some(ref dir) = self.attachments_dir {
            return dir.clone();
        }
        if let Some(out_path) = self.resolved_output_path(issue_id) {
            if let Some(parent) = out_path.parent() {
                if !parent.as_os_str().is_empty() {
                    return parent.join("attachments");
                }
            }
            return PathBuf::from("attachments");
        }
        PathBuf::from(format!("crbug_{}", issue_id)).join("attachments")
    }
}

/// Normalizes user-supplied status names into an Issue Tracker query clause.
pub fn build_status_filter(statuses: &[String]) -> Result<String> {
    let mut mapped: Vec<String> = Vec::new();
    let mut has_meta_status = false;

    for raw in statuses {
        let norm = raw.trim().to_ascii_lowercase().replace(['-', ' '], "_");
        if norm.is_empty() {
            continue;
        }
        let token = match norm.as_str() {
            "open" => {
                has_meta_status = true;
                "open"
            }
            "closed" => {
                has_meta_status = true;
                "closed"
            }
            "new" | "unconfirmed" | "untriaged" => "NEW",
            "assigned" => "ASSIGNED",
            "accepted" | "in_progress" | "inprogress" | "started" => "ACCEPTED",
            "fixed" => "FIXED",
            "verified" => "VERIFIED",
            "not_reproducible" | "notreproducible" | "unreproducible" => "NOT_REPRODUCIBLE",
            "intended_behavior" | "intendedbehavior" | "wai" | "works_as_intended" => {
                "INTENDED_BEHAVIOR"
            }
            "obsolete" => "OBSOLETE",
            "infeasible" | "not_feasible" | "notfeasible" => "INFEASIBLE",
            "duplicate" | "dup" => "DUPLICATE",
            "wontfix" | "wont_fix" | "will_not_fix" => {
                "INFEASIBLE|DUPLICATE|NOT_REPRODUCIBLE|OBSOLETE|INTENDED_BEHAVIOR"
            }
            _ => {
                return Err(anyhow!(
                    "Unknown status filter '{}'. Valid statuses: open, closed, new, assigned, accepted (in_progress), fixed, verified, not_reproducible, intended_behavior, obsolete, infeasible, duplicate, wontfix.",
                    raw
                ));
            }
        };
        if !mapped.iter().any(|existing| existing == token) {
            mapped.push(token.to_string());
        }
    }

    if mapped.is_empty() {
        return Err(anyhow!("Status filter cannot be empty"));
    }

    if mapped.len() == 1 {
        let single = &mapped[0];
        if single.contains('|') {
            Ok(format!("status:({})", single))
        } else {
            Ok(format!("status:{}", single))
        }
    } else if has_meta_status {
        let clauses: Vec<String> = mapped
            .iter()
            .map(|s| {
                if s.contains('|') {
                    format!("status:({})", s)
                } else {
                    format!("status:{}", s)
                }
            })
            .collect();
        Ok(format!("({})", clauses.join(" OR ")))
    } else {
        Ok(format!("status:({})", mapped.join("|")))
    }
}

/// Parses an issue identifier from a raw numeric string, `b/<id>`, `crbug.com/<id>`,
/// or `https://issues.chromium.org/issues/<id>` URL.
pub fn parse_issue_id(input: &str) -> Result<i64> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(anyhow!("Issue identifier cannot be empty"));
    }

    if let Ok(id) = trimmed.parse::<i64>() {
        if id > 0 {
            return Ok(id);
        }
    }

    let patterns = [
        r"(?:issues\.chromium\.org|issuetracker\.google\.com|b\.corp\.google\.com)/(?:u/\d+/)?issues/(\d+)",
        r"crbug\.com/(?:[a-zA-Z0-9_-]+/)?(\d+)",
        r"[?&]id=(\d+)",
        r"^(?:b/|b:|crbug:|issue:)(\d+)$",
    ];

    for pat in patterns {
        let re = Regex::new(pat).expect("valid regex");
        if let Some(caps) = re.captures(trimmed) {
            if let Some(m) = caps.get(1) {
                if let Ok(id) = m.as_str().parse::<i64>() {
                    if id > 0 {
                        return Ok(id);
                    }
                }
            }
        }
    }

    Err(anyhow!(
        "Could not parse a valid numeric issue ID from '{}'. Expected an ID like '563075803' or URL like 'https://issues.chromium.org/issues/563075803'.",
        input
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_issue_id() {
        assert_eq!(parse_issue_id("563075803").unwrap(), 563075803);
        assert_eq!(
            parse_issue_id("https://issues.chromium.org/issues/563075803").unwrap(),
            563075803
        );
        assert_eq!(
            parse_issue_id("https://issues.chromium.org/u/0/issues/563075803#comment2").unwrap(),
            563075803
        );
        assert_eq!(
            parse_issue_id("https://crbug.com/563075803").unwrap(),
            563075803
        );
        assert_eq!(
            parse_issue_id("crbug.com/chromium/563075803").unwrap(),
            563075803
        );
        assert_eq!(parse_issue_id("b/563075803").unwrap(), 563075803);
        assert!(parse_issue_id("not-an-id").is_err());
    }

    #[test]
    fn test_build_status_filter() {
        assert_eq!(
            build_status_filter(&["fixed".to_string()]).unwrap(),
            "status:FIXED"
        );
        assert_eq!(
            build_status_filter(&["assigned".to_string()]).unwrap(),
            "status:ASSIGNED"
        );
        assert_eq!(
            build_status_filter(&["in_progress".to_string()]).unwrap(),
            "status:ACCEPTED"
        );
        assert_eq!(
            build_status_filter(&["assigned".to_string(), "accepted".to_string()]).unwrap(),
            "status:(ASSIGNED|ACCEPTED)"
        );
        assert_eq!(
            build_status_filter(&["open".to_string()]).unwrap(),
            "status:open"
        );
        assert!(build_status_filter(&["invalid_status".to_string()]).is_err());
    }
}

