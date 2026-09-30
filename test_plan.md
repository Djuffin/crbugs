  ## Test Cases
  ### 1. CLI Smoke & Identifier Parsing
  Verify basic flags, exit codes, and issue identifier formats:

  - crbugs --version and crbugs --help exit 0.
  - Numeric ID (563075803) resolves to issue_id: 563075803.
  - Legacy Monorail ID (1275474) resolves to migrated issue_id: 40207080 and strips `[Empty comment from Monorail migration]` placeholders.
  - Running crbugs with no arguments, a non-numeric prefix/URL (b/563075803, https://crbug.com/563075803), or a nonexistent issue (999999999999) exits non-zero with a clear error message.

  ### 2. Single Issue Fetch (Markdown, JSON, Timeline Flags)
  Using public issue 563075803:
  - Markdown (default): Clean Markdown starting with # [Issue 563075803], followed by ## Metadata, ## Description, ## Attachments, and ## Comments.
  - JSON (--format json): Parses cleanly with jq and has expected fields (issue_id == 563075803, .comments | length > 0, .attachments | length > 0).
  - Comment limit (--max-comments 2 --format json): .comments | length == 2.
  - Field updates (--include-field-updates): Output includes **Field Updates:** entries in the comment timeline.
  - Quiet mode (-q): Stderr is empty (0 bytes) on success.

  ### 3. Issue Search & Filters
  Verify query construction and search results in both Markdown and JSON:
  - By assignee & status: crbugs -u eugene@chromium.org -s fixed -l 3 --no-auth returns a Markdown table with 3 rows and Query: assignee:eugene@chromium.org status:FIXED.
  - By component: crbugs -c "Blink>Media>WebCodecs" -s open -l 2 --no-auth and crbugs -c 1456526 -l 2 --no-auth return matching issues.
  - By raw query (-Q) + JSON: crbugs -u eugene@chromium.org -Q "modified>=2026-01-01" -l 5 --no-auth --format json parses with jq and returns .issues | length == 5.

  ### 4. File Export & Attachment Downloads
  Using a temporary directory $TMPDIR:

  - Full export with attachments (-o): `crbugs 563075803 --no-auth -o "$TMPDIR/full/issue.md"` creates `$TMPDIR/full/issue.md`, downloads all 3 files (`82130204_analyze.py`, `82116102_seek_test.html`, `82130205_seq_server.py`) into `$TMPDIR/full/attachments/`, verifies each file is non-empty, and rewrites attachment links in `issue.md` to `./attachments/...`.
  - Custom attachments dir with JSON output (-a): `crbugs 563075803 --no-auth -a "$TMPDIR/custom_att" --format json` downloads attachments into `$TMPDIR/custom_att/` while emitting JSON with `download_status.state == "downloaded"`.
  - Skip attachments (--skip-attachments): `crbugs 563075803 --no-auth -o "$TMPDIR/no_att/issue.md" --skip-attachments` creates `issue.md` with `Remote URL` status and without creating `$TMPDIR/no_att/attachments/`.
  - Max attachment size (--max-attachment-size 3000): `crbugs 563075803 --no-auth -o "$TMPDIR/sized/issue.md" --max-attachment-size 3000` downloads the 2 attachments `< 3000` bytes and marks the larger one as `Skipped (exceeds --max-attachment-size)`.

  ### 5. Corp Auth (--auth corp) Smoke Check

  Verify authenticated corp access (requires active gcert and `/usr/bin/sso-cred-helper`):

  - Restricted issue fetch: crbugs 556233928 --auth corp --format json succeeds and returns "issue_id": 556233928 with unredacted email addresses.
  - Corp search: crbugs -u eugene@chromium.org -l 3 --auth corp --format json succeeds and returns 3 issues.
  
  --------

  ## Copy-Pasteable Execution Script

    set -euo pipefail
    CRBUGS="${CRBUGS:-./target/debug/crbugs}"
    TMPDIR="$(mktemp -d)"
    trap 'rm -rf "$TMPDIR"' EXIT

    echo "=== 1. Smoke & ID Parsing ==="
    "$CRBUGS" --version
    "$CRBUGS" --help >/dev/null
    "$CRBUGS" "563075803" --no-auth -q | grep -m1 -q "^# \[Issue 563075803\]"
    "$CRBUGS" "1275474" --no-auth -q --max-comments 3 | grep -m1 -q "^# \[Issue 40207080\]"
    ! "$CRBUGS" "b/563075803" --no-auth >/dev/null 2>&1
    ! "$CRBUGS" "https://crbug.com/563075803" --no-auth >/dev/null 2>&1
    ! "$CRBUGS" 1275474 --no-auth -q | grep -q "Empty comment from Monorail migration"
    ! "$CRBUGS" --no-auth >/dev/null 2>&1

    echo "=== 2. Single Issue Fetch & Flags ==="
    "$CRBUGS" 563075803 --no-auth -q --include-field-updates | grep -q "Field Updates:"
    "$CRBUGS" 563075803 --no-auth -q --format json --max-comments 2 \
      | jq -e '.issue_id == 563075803 and (.comments | length == 2) and (.attachments | length == 3)' >/dev/null
    test -z "$("$CRBUGS" 563075803 --no-auth -q 2>&1 >/dev/null)"

    echo "=== 2b. Pending Code Changes & Code Changes ==="
    "$CRBUGS" 327625558 --no-auth -q --format json --max-comments 1 \
      | jq -e '.issue_id == 327625558 and (.pending_code_changes | length == 3) and (.code_changes | length >= 17)' >/dev/null
    "$CRBUGS" 327625558 --no-auth -q --max-comments 1 | grep -q "Pending Code Changes"
    "$CRBUGS" 327625558 --no-auth -q --max-comments 1 | grep -q "https://chromium-review.googlesource.com/c/chromium/src/+/5855319"

    echo "=== 3. Search & Filters ==="
    "$CRBUGS" -u eugene@chromium.org -s fixed -l 3 --no-auth -q | grep -q "status:FIXED"
    "$CRBUGS" -c "Blink>Media>WebCodecs" -s open -l 2 --no-auth -q --format json \
      | jq -e '.issues | length == 2' >/dev/null
    "$CRBUGS" -u eugene@chromium.org -Q "modified>=2026-01-01" -l 5 --no-auth -q --format json \
      | jq -e '.issues | length == 5' >/dev/null

    echo "=== 4. File Export & Attachments ==="
    # 4a. Full export with all attachments
    "$CRBUGS" 563075803 --no-auth -q -o "$TMPDIR/full/issue.md"
    test -f "$TMPDIR/full/issue.md"
    for f in "82130204_analyze.py" "82116102_seek_test.html" "82130205_seq_server.py"; do
      test -s "$TMPDIR/full/attachments/$f"
      grep -q "./attachments/$f" "$TMPDIR/full/issue.md"
    done

    # 4b. Explicit --attachments-dir (-a) with JSON stdout
    "$CRBUGS" 563075803 --no-auth -q -a "$TMPDIR/custom_att" --format json \
      | jq -e '(.attachments | length == 3) and all(.attachments[]; .download_status.state == "downloaded")' >/dev/null
    test "$(ls -1 "$TMPDIR/custom_att" | wc -l)" -eq 3

    # 4c. Skip attachments (--skip-attachments)
    "$CRBUGS" 563075803 --no-auth -q -o "$TMPDIR/no_att/issue.md" --skip-attachments
    test -f "$TMPDIR/no_att/issue.md"
    test ! -d "$TMPDIR/no_att/attachments"
    grep -q "Remote URL" "$TMPDIR/no_att/issue.md"

    # 4d. Max attachment size filter (--max-attachment-size 3000)
    "$CRBUGS" 563075803 --no-auth -q -o "$TMPDIR/sized/issue.md" --max-attachment-size 3000
    test -f "$TMPDIR/sized/issue.md"
    test "$(ls -1 "$TMPDIR/sized/attachments" | wc -l)" -eq 2
    grep -q "Skipped (exceeds \`--max-attachment-size\`)" "$TMPDIR/sized/issue.md"

    echo "=== 5. Corp Auth Smoke Check ==="
    if [ -x /usr/bin/sso-cred-helper ] && [ -x /usr/bin/sso_client ]; then
      "$CRBUGS" 556233928 --auth corp -q --format json \
        | jq -e '.issue_id == 556233928 and .assignee == "eugene@chromium.org"' >/dev/null
      "$CRBUGS" -u eugene@chromium.org -l 3 --auth corp -q --format json \
        | jq -e '.issues | length == 3' >/dev/null
    else
      echo "Skipping Corp Auth Smoke Check (sso-cred-helper not present on this OS)"
    fi

    echo "ALL CHECKS PASSED"
