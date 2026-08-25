# birda-to-ebird

Convert a Birda CSV export (or the ZIP containing it) into eBird's headerless Record Format CSV.

## Usage

```sh
cargo run --release -- convert toby_smith_birda_sightings_25-08-2026.zip \
  --output ebird-import.csv \
  --timezone Australia/Melbourne \
  --location-name "Birda import"
```

The generated file uses the same 19-column eBird Record Format as the old `ausbird` converter: Common Name, Genus, Species, Species Count, Species Comments, Location Name, coordinates, date/time, region, protocol, effort, and checklist fields. Country and state are inferred automatically from the first GPS point in each Birda session using OpenStreetMap Nominatim, with results cached in `.birda-to-ebird-region-cache.json`. Pass `--country` and/or `--state` to override inference. The source export still has no checklist duration or completeness flag, so those remain for review during eBird cleanup. A Birda session is represented by one eBird checklist location: the first GPS point in that session. This avoids turning a moving session into one checklist per GPS coordinate, but should be reviewed before import.

Use `--with-header` to produce a human-inspectable CSV; omit it for the eBird import file.

To prevent accidental re-imports, keep the default local manifest and mark an export only after eBird accepts it:

```sh
cargo run --release -- mark-imported toby_smith_birda_sightings_25-08-2026.zip \
  --ebird-import-id "optional-ebird-id"
```

Future conversions will refuse sightings recorded in that manifest. Use `--allow-reimport` deliberately when testing or correcting an import. The manifest uses Birda's stable `sightingId`; eBird's read API cannot reliably expose that source ID, so it is the authoritative duplicate check.

eBird's import tool is a browser-based authenticated workflow. The public eBird API is for reading/product data, not submitting checklists, so this program deliberately does not store or automate eBird credentials. After conversion, upload the CSV at https://ebird.org/import.

## Development

```sh
cargo test
cargo fmt --check
```
