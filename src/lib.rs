pub mod attachments;
pub mod cli;
pub mod client;
pub mod jspb;
pub mod markdown;
pub mod models;

use anyhow::{Context, Result};
use std::io::Write;

use crate::attachments::download_bundle_attachments;
use crate::cli::{ parse_issue_id, Cli, OutputFormat };
use crate::client::CrbugClient;
use crate::markdown::render_issue_markdown;
use crate::models::AttachmentDownloadStatus;

pub async fn run(cli: Cli) -> Result<()> {
    let issue_id = parse_issue_id(&cli.issue)?;

    let client = CrbugClient::new(
        cli.base_url.clone(),
        cli.usercontent_url.clone(),
        cli.cookie.clone(),
    )?;

    if !cli.quiet {
        eprintln!("Fetching issue {} from {}...", issue_id, cli.base_url);
    }

    let mut bundle = client
        .fetch_issue_bundle(issue_id, cli.include_field_updates, cli.max_comments)
        .await?;

    let output_path = cli.resolved_output_path(issue_id);
    let attachments_dir = cli.resolved_attachments_dir(issue_id);

    if !cli.skip_attachments && !bundle.attachments.is_empty() {
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
        )
        .await?;
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

    if let Some(ref path) = output_path {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                tokio::fs::create_dir_all(parent)
                    .await
                    .with_context(|| format!("Failed to create directory {}", parent.display()))?;
            }
        }
        tokio::fs::write(path, rendered.as_bytes())
            .await
            .with_context(|| format!("Failed to write output file {}", path.display()))?;

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
    } else {
        let mut stdout = std::io::stdout().lock();
        stdout
            .write_all(rendered.as_bytes())
            .context("Failed to write to stdout")?;
        stdout.flush()?;
    }

    Ok(())
}
