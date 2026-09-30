pub mod attachments;
pub mod cli;
pub mod client;
pub mod jspb;
pub mod markdown;
pub mod models;
pub mod protojson;

use anyhow::{anyhow, Context, Result};
use std::io::Write;
use std::path::Path;

use crate::attachments::download_bundle_attachments;
use crate::cli::{parse_issue_id, Cli, OutputFormat};
use crate::client::CrbugClient;
use crate::markdown::{render_issue_markdown, render_search_markdown};
use crate::models::AttachmentDownloadStatus;

pub fn run(cli: Cli) -> Result<()> {
    let client = CrbugClient::new_with_auth(
        cli.base_url.clone(),
        cli.usercontent_url.clone(),
        cli.cookie.clone(),
        cli.effective_auth_mode(),
    )?;
    let auth_label = if client.is_corp_authenticated() {
        "corp auth"
    } else {
        "public"
    };

    let output_path = cli.resolved_output_path();

    if let Some(search_query) = cli.build_search_query()? {
        if !cli.quiet {
            eprintln!(
                "Searching issues ({}) for `{}` (limit {}, sort `{}`)...",
                auth_label, search_query, cli.limit, cli.sort
            );
        }

        let search_result = client.search_issues(&search_query, &cli.sort, cli.limit)?;

        let rendered = match cli.format {
            OutputFormat::Markdown => render_search_markdown(&search_result),
            OutputFormat::Json => {
                let mut s = serde_json::to_string_pretty(&search_result)
                    .context("Failed to serialize SearchIssuesResult to JSON")?;
                s.push('\n');
                s
            }
        };

        write_output(&rendered, output_path.as_deref())?;

        if let Some(ref path) = output_path {
            if !cli.quiet {
                eprintln!(
                    "Exported {} search result(s) (of {} total) to {}",
                    search_result.issues.len(),
                    search_result.total_size,
                    path.display()
                );
            }
        }

        return Ok(());
    }

    let raw_issue = cli.issue.as_deref().ok_or_else(|| {
        anyhow!("Missing issue identifier or search filter (try --help for usage)")
    })?;
    let issue_id = parse_issue_id(raw_issue)?;

    if !cli.quiet {
        eprintln!("Fetching issue {} ({})...", issue_id, auth_label);
    }

    let mut bundle =
        client.fetch_issue_bundle(issue_id, cli.include_field_updates, cli.max_comments)?;

    if cli.should_download_attachments() && !bundle.attachments.is_empty() {
        let attachments_dir = cli.resolved_attachments_dir(issue_id);
        if !cli.quiet {
            eprintln!(
                "Downloading {} attachment(s) to {}...",
                bundle.attachments.len(),
                attachments_dir.display()
            );
        }
        download_bundle_attachments(
            &client,
            &mut bundle,
            &attachments_dir,
            output_path.as_deref(),
            cli.max_attachment_size,
            cli.concurrency,
            cli.quiet,
        )?;
    }

    let rendered = match cli.format {
        OutputFormat::Markdown => render_issue_markdown(&bundle),
        OutputFormat::Json => {
            let mut s = serde_json::to_string_pretty(&bundle)
                .context("Failed to serialize IssueBundle to JSON")?;
            s.push('\n');
            s
        }
    };

    write_output(&rendered, output_path.as_deref())?;

    if let Some(ref path) = output_path {
        if !cli.quiet {
            let downloaded_count = bundle
                .attachments
                .iter()
                .filter(|a| a.download_status == AttachmentDownloadStatus::Downloaded)
                .count();
            eprintln!(
                "Exported issue {} ({}) to {}",
                bundle.issue_id,
                bundle.title,
                path.display()
            );
            if downloaded_count > 0 {
                for att in &bundle.attachments {
                    if let Some(ref lp) = att.local_path {
                        eprintln!("  attachment: {} ({})", lp.display(), att.content_type);
                    }
                }
            }
        }
    }

    Ok(())
}

fn write_output(rendered: &str, output_path: Option<&Path>) -> Result<()> {
    if let Some(path) = output_path {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("Failed to create directory {}", parent.display()))?;
            }
        }
        std::fs::write(path, rendered.as_bytes())
            .with_context(|| format!("Failed to write output file {}", path.display()))?;
    } else {
        let mut stdout = std::io::stdout().lock();
        stdout
            .write_all(rendered.as_bytes())
            .context("Failed to write to stdout")?;
        stdout.flush()?;
    }
    Ok(())
}
