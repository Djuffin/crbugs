use anyhow::{anyhow, Result};
use clap::{Parser, ValueEnum};
use regex::Regex;
use std::path::PathBuf;

const LONG_ABOUT: &str = "\
crbugs — Fetch a Chromium issue from https://issues.chromium.org/ and export it to Markdown or JSON.

Features:
  • Reads issue title, description, status, priority, severity, type, reporter, assignee, CCs,
    component hierarchy, hotlists, blocking/blocked-by links, and Chromium custom fields.
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

  4. Emit structured JSON to stdout:
     $ crbugs 563075803 --stdout --format json --skip-attachments

  5. Export to a specific Markdown file and custom attachments directory:
     $ crbugs 563075803 -o ./bug.md -a ./bug_attachments

  6. Limit output to the initial description plus the last 10 comments:
     $ crbugs 563075803 --stdout --max-comments 10

  7. Include field-change history (status, assignee, component, label diffs) in the timeline:
     $ crbugs 563075803 --include-field-updates

OUTPUT LAYOUT (DEFAULT FILE MODE):
  ./crbug_<ISSUE_ID>/
  ├── issue_<ISSUE_ID>.md                  # YAML frontmatter + metadata table + description + comments
  └── attachments/
      └── <ATTACHMENT_ID>_<FILENAME>       # Downloaded attachments linked relatively from the .md file";

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum OutputFormat {
    /// Self-contained Markdown with YAML frontmatter, metadata table, and comment thread
    Markdown,
    /// Structured JSON representation of the issue bundle
    Json,
}

#[derive(Debug, Parser)]
#[command(
    name = "crbugs",
    version,
    about = "Fetch a Chromium issue and its attachments from issues.chromium.org into Markdown or JSON",
    long_about = LONG_ABOUT,
    after_long_help = AFTER_LONG_HELP,
    arg_required_else_help = true
)]
pub struct Cli {
    /// Issue identifier: numeric ID (e.g. 563075803) or URL
    /// (e.g. https://issues.chromium.org/issues/563075803, https://crbug.com/563075803, b/563075803)
    #[arg(value_name = "ISSUE")]
    pub issue: String,

    /// Output file path (use '-' for stdout).
    /// Defaults to ./crbug_<id>/issue_<id>.md (or .json when --format=json)
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
    /// Returns true if the primary document should be written to stdout.
    pub fn writes_to_stdout(&self) -> bool {
        self.stdout
            || self
                .output
                .as_ref()
                .map(|p| p.as_os_str() == "-")
                .unwrap_or(false)
    }

    /// Resolves the target output file path (`None` when writing to stdout).
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
}
