  ## Test Cases
  ### 1. CLI Smoke & Identifier Parsing
  Verify basic flags, exit codes, and issue identifier formats:

  - crbugs --version and crbugs --help exit 0.
  - Numeric ID (563075803), https://crbug.com/563075803, https://issues.chromium.org/issues/563075803, and b/563075803 all resolve to issue_id: 563075803.
  - Running crbugs with no arguments or a nonexistent issue (999999999999) exits non-zero with a clear error message.

  ### 2. Single Issue Fetch (Markdown, JSON, Timeline Flags)
  Using public issue 563075803:
  - Markdown (default): Contains YAML frontmatter (issue_id: 563075803), # [Issue 563075803], ## Metadata, ## Description, ## Attachments, and ## Comments.
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

  - Export with attachments (-o): crbugs 563075803 --no-auth -o "$TMPDIR/issue.md" creates $TMPDIR/issue.md, downloads 3 files into $TMPDIR/attachments/, and rewrites attachment links in issue.md to .
  /attachments/....
  - Skip attachments (--skip-attachments): crbugs 563075803 --no-auth -o "$TMPDIR/no_att/issue.md" --skip-attachments creates issue.md without creating $TMPDIR/no_att/attachments/.
  - Max attachment size (--max-attachment-size 3000): Downloads attachments < 3000 bytes and marks larger ones as Skipped (exceeds --max-attachment-size).

  ### 5. Corp Auth (--auth corp) Smoke Check

  Verify authenticated corp access (requires active gcert):

  - Restricted issue fetch: crbugs 556233928 --auth corp --format json succeeds and returns "issue_id": 556233928 with unredacted email addresses.
  - Corp search: crbugs -u eugene@chromium.org -l 3 --auth corp --format json succeeds and returns 3 issues.
  
  --------

  ## Copy-Pasteable Execution Script

    set -euo pipefail
    CRBUGS="${CRBUGS:-/google/data/ro/users/ez/ezemtsov/public/bin/crbugs}"
    TMPDIR="$(mktemp -d)"
    trap 'rm -rf "$TMPDIR"' EXIT

    echo "=== 1. Smoke & ID Parsing ==="
    "$CRBUGS" --version
    "$CRBUGS" --help >/dev/null
    for id in "563075803" "https://crbug.com/563075803" "b/563075803"; do
      "$CRBUGS" "$id" --no-auth -q | head -n 5 | grep -q "issue_id: 563075803"
    done
    ! "$CRBUGS" --no-auth >/dev/null 2>&1

    echo "=== 2. Single Issue Fetch & Flags ==="
    "$CRBUGS" 563075803 --no-auth -q --include-field-updates | grep -q "Field Updates:"
    "$CRBUGS" 563075803 --no-auth -q --format json --max-comments 2 \
      | jq -e '.issue_id == 563075803 and (.comments | length == 2) and (.attachments | length == 3)' >/dev/null
    test -z "$("$CRBUGS" 563075803 --no-auth -q 2>&1 >/dev/null)"

    echo "=== 3. Search & Filters ==="
    "$CRBUGS" -u eugene@chromium.org -s fixed -l 3 --no-auth -q | grep -q "status:FIXED"
    "$CRBUGS" -c "Blink>Media>WebCodecs" -s open -l 2 --no-auth -q --format json \
      | jq -e '.issues | length == 2' >/dev/null
    "$CRBUGS" -u eugene@chromium.org -Q "modified>=2026-01-01" -l 5 --no-auth -q --format json \
      | jq -e '.issues | length == 5' >/dev/null

    echo "=== 4. File Export & Attachments ==="
    "$CRBUGS" 563075803 --no-auth -q -o "$TMPDIR/issue.md" --max-attachment-size 3000
    test -f "$TMPDIR/issue.md"
    test "$(ls -1 "$TMPDIR/attachments" | wc -l)" -eq 2
    grep -q "Skipped (exceeds \`--max-attachment-size\`)" "$TMPDIR/issue.md"

    echo "=== 5. Corp Auth Smoke Check ==="
    "$CRBUGS" 556233928 --auth corp -q --format json \
      | jq -e '.issue_id == 556233928 and .assignee == "eugene@chromium.org"' >/dev/null
    "$CRBUGS" -u eugene@chromium.org -l 3 --auth corp -q --format json \
      | jq -e '.issues | length == 3' >/dev/null

    echo "ALL CHECKS PASSED"
