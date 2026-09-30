use crbugs::client::CrbugClient;
use crbugs::markdown::render_issue_markdown;

#[test]
fn test_code_changes_and_pending_code_changes_327625558() {
    let client = CrbugClient::new(
        "https://issues.chromium.org".to_string(),
        "https://usercontent.issues.chromium.org".to_string(),
        None,
    )
    .expect("Failed to create CrbugClient");

    let bundle = client
        .fetch_issue_bundle(327625558, false, Some(1))
        .expect("Failed to fetch issue 327625558");

    // 1. Verify pending code changes
    assert!(
        !bundle.pending_code_changes.is_empty(),
        "Expected pending code changes on issue 327625558"
    );
    assert_eq!(bundle.pending_code_changes.len(), 3);
    for pending in &bundle.pending_code_changes {
        assert_eq!(pending.state, "PENDING");
        assert_eq!(pending.host, "chromium");
        assert_eq!(pending.repo, "chromium/src");
        assert!(
            pending
                .url
                .starts_with("https://chromium-review.googlesource.com/c/chromium/src/+/")
        );
    }
    let pending_numbers: Vec<i64> = bundle
        .pending_code_changes
        .iter()
        .map(|c| c.change_number)
        .collect();
    assert!(pending_numbers.contains(&5855319));
    assert!(pending_numbers.contains(&6086499));
    assert!(pending_numbers.contains(&8487638));

    // 2. Verify merged code changes
    assert!(
        bundle.code_changes.len() >= 17,
        "Expected at least 17 merged code changes on issue 327625558, got {}",
        bundle.code_changes.len()
    );
    for merged in &bundle.code_changes {
        assert_eq!(merged.state, "MERGED");
        assert_eq!(merged.host, "chromium");
        assert_eq!(merged.repo, "chromium/src");
    }

    // 3. Verify Markdown rendering
    let md = render_issue_markdown(&bundle);
    assert!(!md.starts_with("---"));
    assert!(
        md.contains("| **Pending Code Changes** |"),
        "Expected Pending Code Changes row in metadata table"
    );
    assert!(
        md.contains("| **Code Changes** |"),
        "Expected Code Changes row in metadata table"
    );
    assert!(md.contains("[5855319](https://chromium-review.googlesource.com/c/chromium/src/+/5855319)"));
    assert!(md.contains("[5742025](https://chromium-review.googlesource.com/c/chromium/src/+/5742025)"));
}
