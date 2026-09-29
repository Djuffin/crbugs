use clap::Parser;
use crbugs::attachments::download_bundle_attachments;
use crbugs::cli::Cli;
use crbugs::client::CrbugClient;
use crbugs::markdown::render_issue_markdown;
use crbugs::models::AttachmentDownloadStatus;
use tempfile::tempdir;

const TEST_ISSUE_ID: i64 = 563075803;

#[test]
fn test_fetch_and_export_issue_563075803() {
    let client = CrbugClient::new(
        "https://issues.chromium.org".to_string(),
        "https://usercontent.issues.chromium.org".to_string(),
        None,
    )
    .expect("Failed to create CrbugClient");

    let mut bundle = client
        .fetch_issue_bundle(TEST_ISSUE_ID, true, None)
        .expect("Failed to fetch issue 563075803");

    // 1. Verify Title & Description
    assert_eq!(bundle.issue_id, TEST_ISSUE_ID);
    assert!(
        bundle.title.contains("HEVC hardware decode"),
        "Unexpected title: {}",
        bundle.title
    );
    let desc = bundle
        .description
        .as_ref()
        .expect("Expected description (comment #1) to be present");
    assert_eq!(desc.comment_number, Some(1));
    assert!(!desc.body.is_empty());

    // 2. Verify Fields (Type, Reporter, Assignee, Status, Priority, Severity, Component, Custom Fields)
    assert_eq!(bundle.issue_type, "BUG");
    assert_eq!(bundle.component_id, 1456526);
    assert!(!bundle.component_path.is_empty());
    assert!(bundle.reporter.is_some());
    assert!(bundle.assignee.is_some());
    assert!(!bundle.custom_fields.is_empty());

    // 3. Verify Comments (Comments #2..#9)
    assert!(
        bundle.comments.len() >= 8,
        "Expected at least 8 comments on issue 563075803, got {}",
        bundle.comments.len()
    );
    let comment_3 = bundle
        .comments
        .iter()
        .find(|c| c.comment_number == Some(3))
        .expect("Expected Comment #3");
    assert!(
        comment_3.attachments.len() >= 3,
        "Expected 3 coalesced attachments on Comment #3, got {}",
        comment_3.attachments.len()
    );

    // 4. Verify Attachments Download
    assert!(
        bundle.attachments.len() >= 3,
        "Expected at least 3 attachments on issue 563075803"
    );

    let tmp = tempdir().expect("Failed to create temp dir");
    let md_path = tmp.path().join("issue_563075803.md");
    let att_dir = tmp.path().join("attachments");

    download_bundle_attachments(
        &client,
        &mut bundle,
        &att_dir,
        Some(&md_path),
        10 * 1024 * 1024,
        4,
        true,
    )
    .expect("Failed to download attachments for 563075803");

    for expected_name in ["analyze.py", "seek_test.html", "seq_server.py"] {
        let att = bundle
            .attachments
            .iter()
            .find(|a| a.filename == expected_name)
            .unwrap_or_else(|| panic!("Missing attachment {}", expected_name));
        assert_eq!(att.download_status, AttachmentDownloadStatus::Downloaded);
        let local_path = att.local_path.as_ref().expect("Missing local_path");
        assert!(
            local_path.exists(),
            "Attachment file does not exist on disk"
        );
        let metadata = std::fs::metadata(local_path).unwrap();
        assert!(metadata.len() > 0, "Downloaded attachment is empty");
    }

    // 5. Verify Markdown Export
    let md = render_issue_markdown(&bundle);
    std::fs::write(&md_path, &md).expect("Failed to write markdown");
    assert!(md.contains("issue_id: 563075803"));
    assert!(md.contains("HEVC hardware decode"));
    assert!(md.contains("## Metadata"));
    assert!(md.contains("## Attachments"));
    assert!(md.contains("## Description"));
    assert!(md.contains("## Comments"));
    assert!(md.contains("### Comment #2"));
    assert!(md.contains("./attachments/82130204_analyze.py"));
    assert!(md.contains("./attachments/82116102_seek_test.html"));
    assert!(md.contains("./attachments/82130205_seq_server.py"));
}

#[test]
fn test_cli_end_to_end_563075803() {
    let tmp = tempdir().expect("Failed to create temp dir");
    let out_file = tmp.path().join("exported.md");
    let att_dir = tmp.path().join("files");

    let cli = Cli::parse_from([
        "crbugs",
        "https://issues.chromium.org/issues/563075803",
        "-o",
        out_file.to_str().unwrap(),
        "-a",
        att_dir.to_str().unwrap(),
        "--quiet",
    ]);

    crbugs::run(cli).expect("CLI run failed");

    assert!(out_file.exists());
    let content = std::fs::read_to_string(&out_file).unwrap();
    assert!(content.contains("# [Issue 563075803]"));
    assert!(att_dir.join("82130204_analyze.py").exists());
}
