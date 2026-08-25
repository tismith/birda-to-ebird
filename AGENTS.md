# AGENTS.md

## Project purpose

`birda-to-ebird` converts Birda CSV or ZIP exports into eBird Record Format CSV files. It also maintains a local ledger of Birda `sightingId` values that have been confirmed as imported.

## Before changing code

- Read this file and `README.md`.
- Treat Birda exports as personal data. Do not commit source ZIP/CSV exports, generated eBird files, region caches, or import manifests.
- Preserve the eBird import specification documented at https://support.ebird.org/en/support/solutions/articles/48000907878-upload-spreadsheet-data-to-ebird.

## Data and conversion rules

- The output is eBird’s 19-column Record Format CSV and must be headerless unless `--with-header` is explicitly requested for inspection.
- Keep the exact eBird column order, comma delimiter, MM/DD/YYYY dates, and 1 MB maximum output size.
- A Birda session must become one eBird checklist: use its representative location and earliest local start time for every row in that session.
- Reject sessions that span local calendar dates; eBird checklists cannot span dates.
- Timezone comes from the representative coordinate using bundled timezone-boundary data; an explicit CLI/config timezone is an intentional override. Country and state/province come from reverse geocoding the representative coordinate, with explicit CLI values taking precedence. Cache lookups locally, use a clear User-Agent, respect service rate limits, and tolerate lookup failure with a review warning.
- Do not invent duration, completeness, observer count, or exact eBird hotspot identity when Birda does not provide it. Document assumptions and leave fields for eBird cleanup where appropriate.
- Preserve source `sightingId` and `sessionId` for validation and deduplication, but never add them to the eBird CSV.

## Import tracking

- The local import manifest is authoritative for duplicate prevention because eBird does not expose Birda source IDs.
- `convert` must not mark sightings imported. Only run `mark-imported` after eBird has accepted the upload.
- Never silently overwrite an imported sighting. Require `--allow-reimport` for deliberate retries or corrections.
- Do not automate or store eBird passwords, cookies, or API credentials.

## Verification

Run before committing:

```sh
cargo fmt --check
cargo check --offline
cargo test --offline
```

For a supplied export, also run a conversion with explicit test output outside the repository and verify row/column counts, dates, session grouping, region fields, and file size.

## Git workflow

- Keep commits focused and descriptive.
- Review `git diff --check`, `git status`, and the staged file list before committing.
- Never stage personal exports or generated import artifacts.
- Push the requested commit to the repository’s configured GitHub remote after verification.
