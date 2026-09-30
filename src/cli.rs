use anyhow::{anyhow, Result};
use clap::{Parser, ValueEnum};
use std::path::PathBuf;

const LONG_ABOUT: &str = "\
crbugs — Fetch or search Chromium issues on https://issues.chromium.org/ in Markdown or JSON.

By default, output is printed directly to stdout. Pass -o / --output <PATH> to export to a file.

Features:
  • Fetch a single issue by numeric ID: title, description, status, priority, severity, type,
    reporter, assignee, CCs, component hierarchy, hotlists, blocking/blocked-by links, and
    Chromium custom fields.
  • Search issues by assignee (--assignee / -u), reporter (--reporter), CC (--cc), component
    (--component / -c), status (--status / -s: fixed, assigned, accepted, new, open, closed,
    verified, etc.), or raw query.
  • Reads the full comment thread in chronological order, coalescing attachment uploads
    with their associated comments.
  • Downloads binary attachments concurrently when exporting to a file (-o) or when
    --attachments-dir (-a) is specified.
  • Works out-of-the-box without authentication for public Chromium issues.";

const AFTER_LONG_HELP: &str = "\
QUERY SYNTAX (-Q / --query):
  The -Q / --query flag accepts the Google Issue Tracker search query language and can be
  used standalone or combined with -u/--assignee, --reporter, --cc, -c/--component, and -s/--status:

  • User filters:
      assignee:<email>          Issues assigned to user
      reporter:<email>          Issues reported by user
      cc:<email>                Issues where user is CC'd
      commentby:<email>         Issues with comments by user
      verifier:<email>          Issues verified by user

  • Date & time filters (YYYY-MM-DD, YYYY-MM-DD..YYYY-MM-DD, <, <=, >, >=, or Nd for last N days):
      modified:2026-07-01..2026-09-30   Modified within a date range (e.g. Q3)
      resolved:2026-07-01..2026-09-30   Resolved/fixed within a date range
      created:2026-07-01..2026-09-30    Created within a date range
      verified:2026-07-01..2026-09-30   Verified within a date range
      modified>=2026-07-01              Modified on or after a date
      modified:7d                       Modified in the last 7 days

  • Field & metadata filters:
      status:open | status:closed | status:(ASSIGNED|ACCEPTED|FIXED|VERIFIED|...)
      priority:(P0|P1|P2|P3|P4)
      severity:(S0|S1|S2|S3|S4)
      type:(BUG|FEATURE_REQUEST|VULNERABILITY|TASK|...)
      customfield1222907:\"<tag>\"  Filter by Chromium Component Tag (or use -c <tag>)
      componentid:<id>          Filter by numeric component ID (e.g. componentid:1456526)
      hotlistid:<id>            Filter by hotlist ID
      title:\"<phrase>\"          Match phrase in issue title

  • Boolean operators:
      Space is implicit AND; use OR, |, parentheses (...), and -<term> for negation.

EXAMPLES:
  1. Print an issue in Markdown to stdout (default):
     $ crbugs 563075803

  2. Export an issue to a Markdown file and download its attachments:
     $ crbugs 563075803 -o ./crbug_563075803/issue.md
     $ crbugs 563075803 -o ./bug.md -a ./bug_attachments

  3. Search for issues assigned to a user by email (prints to stdout by default):
     $ crbugs --assignee eugene@chromium.org
     $ crbugs eugene@chromium.org

  4. Search for issues assigned to a user filtered by status (fixed, assigned, accepted, open, etc.):
     $ crbugs --assignee eugene@chromium.org --status fixed
     $ crbugs -u eugene@chromium.org -s assigned,accepted --limit 20

  5. Filter issues by Chromium component tag or numeric component ID (-c / --component):
     $ crbugs -c \"Blink>Media>WebCodecs\" -s open
     $ crbugs \"Blink>Media>WebCodecs\" -s new,assigned
     $ crbugs -c 1456526 -s open

  6. Find all issues worked on or resolved in a quarter (e.g. Q3) using -Q:
     $ crbugs -u eugene@chromium.org -Q \"modified:2026-07-01..2026-09-30\" -l 100
     $ crbugs -u eugene@chromium.org -s fixed,verified -Q \"resolved:2026-07-01..2026-09-30\" -l 100
     $ crbugs -Q \"(assignee:eugene@chromium.org OR commentby:eugene@chromium.org) modified:2026-07-01..2026-09-30\" -l 200

  7. Emit structured JSON to stdout:
     $ crbugs 563075803 --format json
     $ crbugs -u eugene@chromium.org -s fixed --format json

  8. Limit output to the initial description plus the last 10 comments:
     $ crbugs 563075803 --max-comments 10

  9. Include field-change history (status, assignee, component, label diffs) in the timeline:
     $ crbugs 563075803 --include-field-updates

  10. Force corp authentication (via gcert) or public unauthenticated access:
      $ crbugs 556233928 --auth corp
      $ crbugs 563075803 --no-auth";

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum OutputFormat {
    /// Self-contained Markdown with YAML frontmatter, metadata table, and comment thread
    Markdown,
    /// Structured JSON representation of the issue bundle or search results
    Json,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum AuthMode {
    /// Automatically use corp gcert auth (sso-cred-helper + sso_client) if available, falling back to public
    Auto,
    /// Require corp authentication via issuetracker.corp.googleapis.com/v1
    Corp,
    /// Public unauthenticated access via issues.chromium.org
    None,
}

#[derive(Debug, Parser)]
#[command(
    name = "crbugs",
    version,
    about = "Fetch or search Chromium issues from issues.chromium.org (prints to stdout by default)",
    long_about = LONG_ABOUT,
    after_long_help = AFTER_LONG_HELP,
    arg_required_else_help = true
)]
pub struct Cli {
    /// Issue number (e.g. 563075803), user email (e.g. eugene@chromium.org),
    /// or Chromium component path (e.g. Blink>Media>WebCodecs)
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

    /// Filter search results by Chromium component tag (e.g. "Blink>Media>WebCodecs",
    /// "Internals>Media>Video") or numeric component ID (e.g. "1456526", "1456190+")
    #[arg(
        short = 'c',
        long = "component",
        value_delimiter = ',',
        value_name = "COMPONENT"
    )]
    pub component: Vec<String>,

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

    /// Issue Tracker search query (e.g. "modified:2026-07-01..2026-09-30", "resolved>=2026-07-01",
    /// "commentby:user@chromium.org", "priority:P0|P1"). Can be combined with -u, -c, and -s
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

    /// Export output to a file path instead of printing to stdout (use '-' for stdout)
    #[arg(short = 'o', long = "output", value_name = "PATH")]
    pub output: Option<PathBuf>,

    /// Print output to stdout (default when --output is not specified)
    #[arg(long = "stdout", hide_short_help = true)]
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
    /// [default: <output_dir>/attachments when -o is used]
    #[arg(short = 'a', long = "attachments-dir", value_name = "DIR")]
    pub attachments_dir: Option<PathBuf>,

    /// Do not download attachment binary files even when exporting to a file
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

    /// Authentication mode: auto (use corp gcert auth if available, else public), corp, or none
    #[arg(
        long = "auth",
        value_enum,
        default_value_t = AuthMode::Auto,
        value_name = "MODE"
    )]
    pub auth: AuthMode,

    /// Disable corp authentication and use public unauthenticated access (shorthand for --auth=none)
    #[arg(long = "no-auth")]
    pub no_auth: bool,

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
    /// Returns the effective `AuthMode` taking `--no-auth` into account.
    pub fn effective_auth_mode(&self) -> AuthMode {
        if self.no_auth {
            AuthMode::None
        } else {
            self.auth
        }
    }
    /// Returns `Some(query_string)` if the CLI was invoked in search mode, or `None` for single-issue mode.
    pub fn build_search_query(&self) -> Result<Option<String>> {
        let mut parts: Vec<String> = Vec::new();

        // Check if the positional argument is an email address, component path, or raw search query
        let mut positional_assignee: Option<&str> = None;
        let mut positional_component: Option<String> = None;
        let mut positional_query: Option<&str> = None;

        if let Some(ref raw_target) = self.issue {
            let trimmed = raw_target.trim();
            if parse_issue_id(trimmed).is_err() {
                if trimmed.contains('@') && !trimmed.contains(':') && !trimmed.contains(' ') {
                    positional_assignee = Some(trimmed);
                } else if trimmed.contains('>') && !trimmed.contains(':') && !trimmed.contains('@')
                {
                    positional_component = Some(trimmed.to_string());
                } else if trimmed.contains(':') {
                    positional_query = Some(trimmed);
                }
            }
        }

        let has_search_flags = self.assignee.is_some()
            || self.reporter.is_some()
            || self.cc.is_some()
            || !self.component.is_empty()
            || !self.status.is_empty()
            || self.query.is_some()
            || positional_assignee.is_some()
            || positional_component.is_some()
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

        let mut all_components = self.component.clone();
        if let Some(comp) = positional_component {
            all_components.push(comp);
        }
        if !all_components.is_empty() {
            parts.push(build_component_filter(&all_components)?);
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

    /// Resolves the target output file path (`None` when writing to stdout, which is the default).
    pub fn resolved_output_path(&self) -> Option<PathBuf> {
        if self.stdout {
            return None;
        }
        match self.output {
            Some(ref path) if path.as_os_str() != "-" => Some(path.clone()),
            _ => None,
        }
    }

    /// Returns true if attachments should be downloaded to disk.
    /// Attachments are downloaded when `--attachments-dir` (`-a`) or `--output` (`-o`) is explicitly provided,
    /// unless `--skip-attachments` is set.
    pub fn should_download_attachments(&self) -> bool {
        !self.skip_attachments
            && (self.attachments_dir.is_some() || self.resolved_output_path().is_some())
    }

    /// Resolves the directory where attachments should be stored on disk when downloading is enabled.
    pub fn resolved_attachments_dir(&self, issue_id: i64) -> PathBuf {
        if let Some(ref dir) = self.attachments_dir {
            return dir.clone();
        }
        if let Some(out_path) = self.resolved_output_path() {
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

/// Translates Chromium component tag paths (e.g. `Blink>Media>WebCodecs` -> `customfield1222907:"Blink>Media>WebCodecs"`)
/// or numeric component IDs (e.g. `1456526` -> `componentid:1456526`, `1456190+` -> `componentid:1456190+`)
/// into an Issue Tracker query clause.
pub fn build_component_filter(components: &[String]) -> Result<String> {
    let mut clauses: Vec<String> = Vec::new();

    for raw in components {
        let trimmed = raw.trim().trim_matches('"');
        if trimmed.is_empty() {
            continue;
        }

        let numeric_part = trimmed.strip_suffix('+').unwrap_or(trimmed);
        let clause = if !numeric_part.is_empty() && numeric_part.chars().all(|c| c.is_ascii_digit())
        {
            format!("componentid:{}", trimmed)
        } else {
            let normalized = trimmed
                .split('>')
                .map(str::trim)
                .filter(|seg| !seg.is_empty())
                .collect::<Vec<_>>()
                .join(">");
            if normalized.is_empty() {
                continue;
            }
            format!("customfield1222907:\"{}\"", normalized)
        };

        if !clauses.iter().any(|existing| existing == &clause) {
            clauses.push(clause);
        }
    }

    if clauses.is_empty() {
        return Err(anyhow!("Component filter cannot be empty"));
    }

    if clauses.len() == 1 {
        Ok(clauses.remove(0))
    } else {
        Ok(format!("({})", clauses.join(" OR ")))
    }
}

/// Maximum numeric issue ID used by legacy Monorail issues before migration to Google Issue Tracker.
/// `crbug.com` redirects all IDs `<= 9_999_999` through `bugs.chromium.org`.
pub const LEGACY_MONORAIL_MAX_ID: i64 = 9_999_999;

/// Returns `true` if `issue_id` falls in the legacy Monorail ID range (`1..=9_999_999`).
pub fn is_legacy_monorail_id(issue_id: i64) -> bool {
    (1..=LEGACY_MONORAIL_MAX_ID).contains(&issue_id)
}

/// Parsed representation of an issue identifier, optionally carrying a Monorail project prefix
/// (such as `"chromium"` or `"v8"`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueTarget {
    pub project: Option<String>,
    pub id: i64,
}

impl IssueTarget {
    /// Returns `true` if this target refers to a legacy Monorail issue that must be resolved
    /// to a Google Issue Tracker ID before fetching.
    pub fn is_legacy_monorail(&self) -> bool {
        self.project.is_some() || is_legacy_monorail_id(self.id)
    }
}

/// Parses an `IssueTarget` from a raw numeric issue ID string (e.g. `563075803` or `1275474`).
pub fn parse_issue_target(input: &str) -> Result<IssueTarget> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(anyhow!("Issue identifier cannot be empty"));
    }

    if let Ok(id) = trimmed.parse::<i64>() {
        if id > 0 {
            return Ok(IssueTarget { project: None, id });
        }
    }

    Err(anyhow!(
        "Could not parse a valid numeric issue ID from '{}'. Expected a raw numeric ID like '563075803'.",
        input
    ))
}

/// Parses an issue identifier from a raw numeric string (e.g. `563075803`).
pub fn parse_issue_id(input: &str) -> Result<i64> {
    parse_issue_target(input).map(|t| t.id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_issue_id() {
        assert_eq!(parse_issue_id("563075803").unwrap(), 563075803);
        assert!(parse_issue_id("b/563075803").is_err());
        assert!(parse_issue_id("crbug/563075803").is_err());
        assert!(parse_issue_id("https://crbug.com/563075803").is_err());
        assert!(parse_issue_id("https://issues.chromium.org/issues/563075803").is_err());
        assert!(parse_issue_id("not-an-id").is_err());

        let legacy = parse_issue_target("1275474").unwrap();
        assert_eq!(legacy, IssueTarget { project: None, id: 1275474 });
        assert!(legacy.is_legacy_monorail());
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

    #[test]
    fn test_build_component_filter() {
        assert_eq!(
            build_component_filter(&["Blink>Media>WebCodecs".to_string()]).unwrap(),
            "customfield1222907:\"Blink>Media>WebCodecs\""
        );
        assert_eq!(
            build_component_filter(&["Internals > Media > Video".to_string()]).unwrap(),
            "customfield1222907:\"Internals>Media>Video\""
        );
        assert_eq!(
            build_component_filter(&["1456526".to_string()]).unwrap(),
            "componentid:1456526"
        );
        assert_eq!(
            build_component_filter(&["1456190+".to_string()]).unwrap(),
            "componentid:1456190+"
        );
        assert_eq!(
            build_component_filter(&[
                "Blink>Media>WebCodecs".to_string(),
                "Internals>Media>Video".to_string()
            ])
            .unwrap(),
            "(customfield1222907:\"Blink>Media>WebCodecs\" OR customfield1222907:\"Internals>Media>Video\")"
        );

        let positional_cli = Cli::parse_from(["crbugs", "Blink>Media>WebCodecs", "-s", "open"]);
        assert_eq!(
            positional_cli.build_search_query().unwrap().as_deref(),
            Some("customfield1222907:\"Blink>Media>WebCodecs\" status:open")
        );
    }

    #[test]
    fn test_default_stdout_and_explicit_file_export() {
        let default_cli = Cli::parse_from(["crbugs", "563075803"]);
        assert!(default_cli.resolved_output_path().is_none());
        assert!(!default_cli.should_download_attachments());

        let file_cli = Cli::parse_from(["crbugs", "563075803", "-o", "out.md"]);
        assert_eq!(
            file_cli.resolved_output_path().unwrap(),
            PathBuf::from("out.md")
        );
        assert!(file_cli.should_download_attachments());

        let file_skip_att_cli =
            Cli::parse_from(["crbugs", "563075803", "-o", "out.md", "--skip-attachments"]);
        assert!(!file_skip_att_cli.should_download_attachments());

        let stdout_with_att_cli = Cli::parse_from(["crbugs", "563075803", "-a", "my_attachments"]);
        assert!(stdout_with_att_cli.resolved_output_path().is_none());
        assert!(stdout_with_att_cli.should_download_attachments());
    }
}
