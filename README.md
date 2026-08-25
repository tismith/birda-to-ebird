# birda-to-ebird

Convert a [Birda](https://birda.org/) CSV export into a CSV that can be imported into eBird.

The program accepts either Birda’s CSV export or the ZIP file containing it. It groups sightings by Birda session, converts UTC timestamps to the local timezone for each session, infers regions from coordinates, and writes eBird’s headerless Record Format.

## Quick start

```sh
cargo run --release -- convert birda-export.zip --output ebird-import.csv
```

Then upload `ebird-import.csv` through eBird’s `Submit` → `Import Data` → `Record Format` workflow:

<https://ebird.org/import>

For a human-readable version with column headings, add `--with-header`. Do not use that version for the eBird upload.

## Import workflow

1. Export your sightings from Birda.
2. Convert the CSV or ZIP export.
3. Review the generated CSV.
4. Upload it to eBird and complete any taxonomy or location cleanup eBird requests.
5. After eBird accepts the import, record it locally:

   ```sh
   cargo run --release -- mark-imported birda-export.zip \
     --ebird-import-id "optional-ebird-import-id"
   ```

The import ledger uses Birda’s stable `sightingId` values. Future conversions refuse sightings already recorded in the ledger. Use `--allow-reimport` only when deliberately retrying or correcting an import.

## What the converter does

- Produces eBird’s 19-column, headerless Record Format CSV.
- Uses one representative coordinate per Birda session.
- Uses the earliest sighting time in a session as that checklist’s start time.
- Rejects sessions that span multiple local calendar dates.
- Infers each session’s timezone from its coordinates using bundled timezone-boundary data.
- Infers country and state/province using OpenStreetMap Nominatim.
- Converts non-exact Birda counts to eBird’s `X` value.
- Preserves Birda notes as species comments.
- Refuses output larger than eBird’s 1 MB import limit.

Birda does not provide all eBird effort fields. Duration, completeness, and exact eBird location/hotspot matching may therefore require review during eBird’s cleanup process.

The eBird import specification is documented here:

<https://support.ebird.org/en/support/solutions/articles/48000907878-upload-spreadsheet-data-to-ebird>

## Configuration

Persistent files use standard XDG locations:

- Configuration: `~/.config/birda-to-ebird/config.toml`
- Import ledger: `~/.local/state/birda-to-ebird/imports.json`
- Geocoding cache: `~/.cache/birda-to-ebird/regions.json`

Example configuration:

```toml
# Optional: override coordinate-based timezone inference for every session.
timezone = "Australia/Brisbane"
location_name = "Birda import"
protocol = "Incidental"
```

Command-line options override configuration-file values. Use `--config`, `--manifest`, or `--region-cache` to select alternate paths.

Country and state can also be supplied explicitly when required:

```sh
cargo run --release -- convert birda-export.zip \
  --output ebird-import.csv \
  --country AU \
  --state QLD
```

## Development

```sh
cargo fmt --all --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --all-features --locked
cargo build --release --all-features --locked
```

CI runs these checks on pushes and pull requests. Dependabot monitors Cargo and GitHub Actions dependencies.
