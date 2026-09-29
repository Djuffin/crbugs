use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

use crate::client::CrbugClient;
use crate::models::{AttachmentDownloadStatus, AttachmentMeta, IssueBundle};

/// Downloads all eligible attachments for `bundle` concurrently into `attachments_dir`
/// using a bounded pool of OS worker threads (`std::thread::scope`), updating
/// `bundle.attachments`, `bundle.description`, and `bundle.comments` with local and relative paths.
pub fn download_bundle_attachments(
    client: &CrbugClient,
    bundle: &mut IssueBundle,
    attachments_dir: &Path,
    markdown_file_path: Option<&Path>,
    max_attachment_size: u64,
    concurrency: usize,
    quiet: bool,
) -> Result<()> {
    if bundle.attachments.is_empty() {
        return Ok(());
    }

    let has_downloadable = bundle
        .attachments
        .iter()
        .any(|a| !a.is_deleted && a.size_bytes <= max_attachment_size);

    if has_downloadable {
        std::fs::create_dir_all(attachments_dir).with_context(|| {
            format!(
                "Failed to create attachments directory {}",
                attachments_dir.display()
            )
        })?;
    }

    let worker_count = concurrency.max(1).min(bundle.attachments.len());
    let issue_id = bundle.issue_id;
    let md_parent = markdown_file_path
        .and_then(|p| p.parent())
        .filter(|p| !p.as_os_str().is_empty());

    let queue = Mutex::new(bundle.attachments.clone().into_iter());
    let results: Mutex<HashMap<i64, AttachmentMeta>> =
        Mutex::new(HashMap::with_capacity(bundle.attachments.len()));

    std::thread::scope(|s| {
        for _ in 0..worker_count {
            s.spawn(|| loop {
                let next_att = {
                    let mut guard = queue.lock().expect("queue lock poisoned");
                    guard.next()
                };
                let Some(mut att) = next_att else {
                    break;
                };

                if att.is_deleted {
                    att.download_status = AttachmentDownloadStatus::DeletedOnServer;
                } else if att.size_bytes > max_attachment_size {
                    att.download_status = AttachmentDownloadStatus::SkippedTooLarge;
                } else {
                    let dest_path = attachments_dir.join(&att.sanitized_filename);
                    match client.download_attachment_to_path(issue_id, att.attachment_id, &dest_path)
                    {
                        Ok(bytes_written) => {
                            if bytes_written > 0 && att.size_bytes == 0 {
                                att.size_bytes = bytes_written;
                            }
                            let rel = compute_relative_link(md_parent, &dest_path);
                            att.relative_path = Some(rel);
                            att.local_path = Some(dest_path);
                            att.download_status = AttachmentDownloadStatus::Downloaded;
                        }
                        Err(err) => {
                            if !quiet {
                                eprintln!(
                                    "warning: failed to download attachment {} ({}): {}",
                                    att.attachment_id, att.filename, err
                                );
                            }
                            att.download_status = AttachmentDownloadStatus::Failed(err.to_string());
                        }
                    }
                }

                results
                    .lock()
                    .expect("results lock poisoned")
                    .insert(att.attachment_id, att);
            });
        }
    });

    let by_id = results.into_inner().expect("results lock poisoned");

    for att in &mut bundle.attachments {
        if let Some(updated) = by_id.get(&att.attachment_id) {
            *att = updated.clone();
        }
    }

    if let Some(ref mut desc) = bundle.description {
        for att in &mut desc.attachments {
            if let Some(updated) = by_id.get(&att.attachment_id) {
                *att = updated.clone();
            }
        }
    }

    for comment in &mut bundle.comments {
        for att in &mut comment.attachments {
            if let Some(updated) = by_id.get(&att.attachment_id) {
                *att = updated.clone();
            }
        }
    }

    Ok(())
}

fn compute_relative_link(md_parent: Option<&Path>, dest_path: &Path) -> String {
    if let Some(parent) = md_parent {
        if let Ok(stripped) = dest_path.strip_prefix(parent) {
            return format!("./{}", stripped.to_string_lossy().replace('\\', "/"));
        }
    }
    let normalized = dest_path.to_string_lossy().replace('\\', "/");
    if normalized.starts_with("./") || normalized.starts_with('/') {
        normalized
    } else {
        format!("./{}", normalized)
    }
}

pub fn format_byte_size(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = 1024.0 * 1024.0;
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;

    let b = bytes as f64;
    if b >= GIB {
        format!("{:.2} GiB", b / GIB)
    } else if b >= MIB {
        format!("{:.2} MiB", b / MIB)
    } else if b >= KIB {
        format!("{:.2} KiB", b / KIB)
    } else {
        format!("{} B", bytes)
    }
}
