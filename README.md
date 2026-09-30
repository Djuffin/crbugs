# crbugs

A fast CLI utility to fetch and search Chromium issues (`issues.chromium.org`) and attachments into Markdown or JSON.

## Features

- **Single Issue Fetch**: Fetch by Buganizer ID (`563075803`) or legacy Monorail ID (`1275474`).
- **Issue Search**: Search by assignee (`-u`), reporter (`--reporter`), CC (`--cc`), Chromium component (`-c`), status (`-s`), or raw Issue Tracker query (`-Q`).
- **Markdown & JSON Output**: Prints clean Markdown (with YAML frontmatter, metadata table, and chronological comments) or structured JSON (`-f json`) to stdout or a file (`-o`).
- **Attachment Downloads**: Automatically downloads binary attachments concurrently when exporting to a file (`-o`) or when `--attachments-dir` (`-a`) is specified.
- **Zero-Config Auth**: Works out-of-the-box without authentication for public Chromium issues, or automatically uses corp `gcert` authentication (`sso-cred-helper` + `sso_client`) when available to access restricted issues and unredacted emails.
