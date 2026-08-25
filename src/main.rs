use anyhow::{bail, Context, Result};
use chrono::{DateTime, Datelike, Timelike, Utc};
use chrono_tz::Tz;
use clap::{Parser, Subcommand, ValueEnum};
use csv::{ReaderBuilder, WriterBuilder};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::File,
    io::{Read, Write},
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};
use tzf_rs::DefaultFinder;
use zip::ZipArchive;

#[derive(Parser, Debug)]
#[command(author, version, about)]
struct Cli {
    /// Configuration file; defaults to ~/.config/birda-to-ebird/config.toml.
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Convert a Birda CSV or ZIP export to an eBird Record Format CSV.
    Convert {
        input: PathBuf,
        #[arg(short, long)]
        output: PathBuf,
        /// IANA timezone used to turn Birda's UTC timestamps into eBird dates/times.
        #[arg(long)]
        timezone: Option<String>,
        /// eBird country code, for example AU or US.
        #[arg(long)]
        country: Option<String>,
        /// eBird state/province code, for example VIC or CA.
        #[arg(long)]
        state: Option<String>,
        /// Name to use for the imported eBird location.
        #[arg(long)]
        location_name: Option<String>,
        /// Cache file for reverse-geocoded country/state results.
        #[arg(long)]
        region_cache: Option<PathBuf>,
        /// Local ledger of Birda sightings confirmed as imported into eBird.
        #[arg(long)]
        manifest: Option<PathBuf>,
        /// Permit converting sightings already marked as imported.
        #[arg(long)]
        allow_reimport: bool,
        #[arg(long, value_enum)]
        protocol: Option<Protocol>,
        /// Preserve the source header for inspection. Omit it for direct eBird import.
        #[arg(long)]
        with_header: bool,
    },
    /// Mark a Birda export as imported after eBird accepts it.
    MarkImported {
        input: PathBuf,
        #[arg(long)]
        manifest: Option<PathBuf>,
        /// Optional import identifier copied from eBird.
        #[arg(long)]
        ebird_import_id: Option<String>,
    },
}

#[derive(Clone, Debug, Deserialize, ValueEnum)]
enum Protocol {
    Stationary,
    Traveling,
    Incidental,
    Historical,
}

#[derive(Debug, Default, Deserialize)]
struct FileConfig {
    timezone: Option<String>,
    country: Option<String>,
    state: Option<String>,
    location_name: Option<String>,
    region_cache: Option<PathBuf>,
    manifest: Option<PathBuf>,
    protocol: Option<Protocol>,
}

#[derive(Debug)]
struct AppConfig {
    timezone: Option<String>,
    country: Option<String>,
    state: Option<String>,
    location_name: String,
    region_cache: PathBuf,
    manifest: PathBuf,
    protocol: Protocol,
}

fn default_app_paths() -> Result<(PathBuf, PathBuf, PathBuf)> {
    let home = dirs::home_dir().context("cannot determine home directory")?;
    let config_home = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".config"));
    let state_home = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".local/state"));
    let cache_home = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".cache"));
    Ok((
        config_home.join("birda-to-ebird/config.toml"),
        state_home.join("birda-to-ebird/imports.json"),
        cache_home.join("birda-to-ebird/regions.json"),
    ))
}

fn load_config(path: Option<&Path>) -> Result<AppConfig> {
    let (default_config, default_manifest, default_cache) = default_app_paths()?;
    let config_path = path.map(PathBuf::from).unwrap_or(default_config);
    let file_config: FileConfig = if config_path.exists() {
        let text = std::fs::read_to_string(&config_path)
            .with_context(|| format!("read config {}", config_path.display()))?;
        toml::from_str(&text).with_context(|| format!("parse config {}", config_path.display()))?
    } else {
        FileConfig::default()
    };
    Ok(AppConfig {
        timezone: file_config.timezone,
        country: file_config.country,
        state: file_config.state,
        location_name: file_config
            .location_name
            .unwrap_or_else(|| "Birda import".to_string()),
        region_cache: file_config.region_cache.unwrap_or(default_cache),
        manifest: file_config.manifest.unwrap_or(default_manifest),
        protocol: file_config.protocol.unwrap_or(Protocol::Incidental),
    })
}

impl Protocol {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Stationary => "Stationary",
            Self::Traveling => "Traveling",
            Self::Incidental => "Incidental",
            Self::Historical => "Historical",
        }
    }
}

#[derive(Debug, Deserialize)]
struct BirdaRow {
    #[serde(rename = "sightingId")]
    sighting_id: String,
    #[serde(rename = "date")]
    date: String,
    longitude: String,
    latitude: String,
    #[serde(rename = "scientificName")]
    scientific_name: String,
    #[serde(rename = "commonName")]
    common_name: String,
    count: String,
    #[serde(rename = "countType")]
    count_type: String,
    note: String,
    #[serde(rename = "sessionId")]
    session_id: String,
}

// eBird Record Format, in the documented column order. eBird requires no header row.
const HEADER: [&str; 19] = [
    "Common Name",
    "Genus",
    "Species",
    "Species Count",
    "Species Comments",
    "Location Name",
    "Latitude",
    "Longitude",
    "Date",
    "Start Time",
    "State",
    "Country",
    "Protocol",
    "Number of Observers",
    "Duration",
    "All Observations Reported",
    "Distance Covered",
    "Area Covered",
    "Checklist Comments",
];

fn main() -> Result<()> {
    let cli = Cli::parse();
    let config = load_config(cli.config.as_deref())?;
    match cli.command {
        Command::Convert {
            input,
            output,
            timezone,
            country,
            state,
            location_name,
            region_cache,
            manifest,
            allow_reimport,
            protocol,
            with_header,
        } => convert(
            &input,
            &output,
            timezone.as_deref().or(config.timezone.as_deref()),
            country.or(config.country),
            state.or(config.state),
            location_name.unwrap_or(config.location_name),
            region_cache.as_deref().unwrap_or(&config.region_cache),
            manifest.as_deref().unwrap_or(&config.manifest),
            allow_reimport,
            protocol.unwrap_or(config.protocol),
            with_header,
        ),
        Command::MarkImported {
            input,
            manifest,
            ebird_import_id,
        } => mark_imported(
            &input,
            manifest.as_deref().unwrap_or(&config.manifest),
            ebird_import_id,
        ),
    }
}

#[allow(clippy::too_many_arguments)] // CLI options are kept explicit for readable call-site mapping.
fn convert(
    input: &Path,
    output: &Path,
    timezone_override: Option<&str>,
    country: Option<String>,
    state: Option<String>,
    location_name: String,
    region_cache_path: &Path,
    manifest_path: &Path,
    allow_reimport: bool,
    protocol: Protocol,
    with_header: bool,
) -> Result<()> {
    let finder = DefaultFinder::new();
    let bytes = read_input(input)?;
    let mut reader = ReaderBuilder::new()
        .flexible(true)
        .from_reader(bytes.as_slice());
    let headers = reader.headers().context("read Birda CSV header")?.clone();
    for required in [
        "sightingId",
        "date",
        "longitude",
        "latitude",
        "scientificName",
        "commonName",
        "count",
        "countType",
        "sessionId",
    ] {
        if !headers.iter().any(|h| h == required) {
            bail!("Birda CSV is missing required column {required:?}");
        }
    }

    let mut rows = Vec::new();
    for (line, result) in reader.deserialize().enumerate() {
        let row: BirdaRow =
            result.with_context(|| format!("invalid Birda CSV row {}", line + 2))?;
        let instant = DateTime::parse_from_rfc3339(&row.date)
            .with_context(|| format!("invalid timestamp {:?} on row {}", row.date, line + 2))?
            .with_timezone(&Utc);
        let latitude: f64 = row
            .latitude
            .parse()
            .with_context(|| format!("invalid latitude on row {}", line + 2))?;
        let longitude: f64 = row
            .longitude
            .parse()
            .with_context(|| format!("invalid longitude on row {}", line + 2))?;
        if !(-90.0..=90.0).contains(&latitude) || !(-180.0..=180.0).contains(&longitude) {
            bail!("coordinates out of range on row {}", line + 2);
        }
        rows.push((row, instant, latitude, longitude));
    }
    if rows.is_empty() {
        bail!("Birda CSV contains no sightings");
    }
    let sightings = rows.len();
    let imported = read_manifest(manifest_path)?;
    let already_imported: Vec<_> = rows
        .iter()
        .filter_map(|(row, _, _, _)| {
            imported
                .get(&row.sighting_id)
                .map(|_| row.sighting_id.clone())
        })
        .collect();
    if !already_imported.is_empty() && !allow_reimport {
        bail!(
            "{} sightings are already marked imported; use --allow-reimport to override",
            already_imported.len()
        );
    }

    // Use one representative point per Birda session. This keeps a session a single
    // eBird checklist even when the source recorded GPS movement within that session.
    let mut session_locations: BTreeMap<String, (f64, f64)> = BTreeMap::new();
    for (row, _, lat, lon) in &rows {
        session_locations
            .entry(row.session_id.clone())
            .or_insert((*lat, *lon));
    }
    let mut session_timezones: BTreeMap<String, Tz> = BTreeMap::new();
    for (session_id, (latitude, longitude)) in &session_locations {
        let timezone_name = timezone_override
            .map(str::to_owned)
            .unwrap_or_else(|| finder.get_tz_name(*longitude, *latitude).to_owned());
        let tz: Tz = timezone_name.parse().with_context(|| {
            format!("invalid or unknown timezone {timezone_name:?} for session {session_id}")
        })?;
        session_timezones.insert(session_id.clone(), tz);
    }
    let mut session_starts: BTreeMap<String, DateTime<Utc>> = BTreeMap::new();
    let mut session_dates: BTreeMap<String, chrono::NaiveDate> = BTreeMap::new();
    for (row, instant, _, _) in &rows {
        let local_date = instant
            .with_timezone(&session_timezones[&row.session_id])
            .date_naive();
        if let Some(previous_date) = session_dates.insert(row.session_id.clone(), local_date) {
            if previous_date != local_date {
                bail!("Birda session {} spans multiple local calendar dates; split it before eBird import", row.session_id);
            }
        }
        session_starts
            .entry(row.session_id.clone())
            .and_modify(|start| *start = (*start).min(*instant))
            .or_insert(*instant);
    }
    let regions = infer_regions(&session_locations, country, state, region_cache_path)?;
    let session_location_names: BTreeMap<_, _> = session_locations
        .iter()
        .map(|(session_id, (latitude, longitude))| {
            (
                session_id.clone(),
                location_label(&location_name, *latitude, *longitude),
            )
        })
        .collect();

    let mut writer = WriterBuilder::new()
        .has_headers(false)
        .from_path(output)
        .with_context(|| format!("create {}", output.display()))?;
    if with_header {
        writer.write_record(HEADER)?;
    }
    for (row, _, _, _) in rows {
        let local =
            session_starts[&row.session_id].with_timezone(&session_timezones[&row.session_id]);
        let (lat, lon) = session_locations[&row.session_id];
        let number = if row.count_type.eq_ignore_ascii_case("EXACT") {
            clean(&row.count)
        } else {
            "X".to_string()
        };
        let species_comment = clean(&row.note);
        let (genus, species) = split_scientific_name(&row.scientific_name);
        writer.write_record([
            ebird_common_name(&row.common_name),
            genus,
            species,
            number,
            species_comment,
            session_location_names[&row.session_id].clone(),
            lat.to_string(),
            lon.to_string(),
            format!(
                "{:02}/{:02}/{:04}",
                local.month(),
                local.day(),
                local.year()
            ),
            format!("{:02}:{:02}", local.hour(), local.minute()),
            regions[&row.session_id].state.clone(),
            regions[&row.session_id].country.clone(),
            protocol.as_str().to_string(),
            "1".to_string(),
            "".to_string(),
            "N".to_string(),
            "".to_string(),
            "".to_string(),
            "".to_string(),
        ])?;
    }
    writer.flush()?;
    let sessions = session_locations.len();
    let missing_region = regions
        .values()
        .any(|region| region.country.len() != 2 || !(1..=3).contains(&region.state.len()));
    eprintln!(
        "Converted {} sightings from {} Birda sessions to {}",
        sightings,
        sessions,
        output.display()
    );
    if missing_region {
        eprintln!(
            "warning: one or more country/state values could not be inferred; eBird may require cleanup"
        );
    }
    eprintln!("warning: duration, completeness, and exact checklist locations are not present in the Birda export");
    if output.metadata()?.len() > 1_000_000 {
        bail!("output exceeds eBird's 1 MB import limit");
    }
    Ok(())
}

#[derive(Clone, Debug, Default, Deserialize, serde::Serialize)]
struct Region {
    country: String,
    state: String,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct ImportEntry {
    imported_at: String,
    ebird_import_id: Option<String>,
}

type ImportManifest = BTreeMap<String, ImportEntry>;

fn read_manifest(path: &Path) -> Result<ImportManifest> {
    if !path.exists() {
        return Ok(BTreeMap::new());
    }
    let file =
        File::open(path).with_context(|| format!("open import manifest {}", path.display()))?;
    serde_json::from_reader(file)
        .with_context(|| format!("read import manifest {}", path.display()))
}

fn ensure_parent(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create directory {}", parent.display()))?;
    }
    Ok(())
}

fn mark_imported(
    input: &Path,
    manifest_path: &Path,
    ebird_import_id: Option<String>,
) -> Result<()> {
    let bytes = read_input(input)?;
    let mut reader = ReaderBuilder::new()
        .flexible(true)
        .from_reader(bytes.as_slice());
    let headers = reader.headers().context("read Birda CSV header")?.clone();
    if !headers.iter().any(|header| header == "sightingId") {
        bail!("Birda CSV is missing required column \"sightingId\"");
    }
    let mut manifest = read_manifest(manifest_path)?;
    let mut count = 0;
    for result in reader.deserialize() {
        let row: SourceIdRow = result.context("read sightingId")?;
        manifest.insert(
            row.sighting_id,
            ImportEntry {
                imported_at: Utc::now().to_rfc3339(),
                ebird_import_id: ebird_import_id.clone(),
            },
        );
        count += 1;
    }
    ensure_parent(manifest_path)?;
    let mut file = File::create(manifest_path)
        .with_context(|| format!("write import manifest {}", manifest_path.display()))?;
    serde_json::to_writer_pretty(&mut file, &manifest).context("serialize import manifest")?;
    file.write_all(b"\n")?;
    eprintln!(
        "Marked {count} sightings as imported in {}",
        manifest_path.display()
    );
    Ok(())
}

#[derive(Debug, Deserialize)]
struct SourceIdRow {
    #[serde(rename = "sightingId")]
    sighting_id: String,
}

#[derive(Debug, Deserialize)]
struct NominatimResponse {
    address: Option<NominatimAddress>,
}

#[derive(Debug, Deserialize)]
struct NominatimAddress {
    country_code: Option<String>,
    #[serde(flatten)]
    values: BTreeMap<String, serde_json::Value>,
}

fn infer_regions(
    sessions: &BTreeMap<String, (f64, f64)>,
    explicit_country: Option<String>,
    explicit_state: Option<String>,
    cache_path: &Path,
) -> Result<BTreeMap<String, Region>> {
    let mut cache: BTreeMap<String, Region> = if cache_path.exists() {
        let file = File::open(cache_path)
            .with_context(|| format!("open region cache {}", cache_path.display()))?;
        serde_json::from_reader(file)
            .with_context(|| format!("read region cache {}", cache_path.display()))?
    } else {
        BTreeMap::new()
    };
    let client = reqwest::blocking::Client::builder()
        .user_agent("birda-to-ebird/0.1 (https://github.com/tismith/birda-to-ebird)")
        .timeout(Duration::from_secs(15))
        .build()
        .context("create reverse-geocoding client")?;
    let mut last_request: Option<Instant> = None;
    let mut result = BTreeMap::new();

    for (session_id, (latitude, longitude)) in sessions {
        let key = format!("{latitude:.5},{longitude:.5}");
        let mut region = cache.get(&key).cloned().unwrap_or_default();
        let needs_inference = explicit_country.is_none() || explicit_state.is_none();
        if needs_inference && (region.country.is_empty() || region.state.is_empty()) {
            if let Some(previous) = last_request {
                if let Some(remaining) = Duration::from_secs(1).checked_sub(previous.elapsed()) {
                    thread::sleep(remaining);
                }
            }
            eprintln!("inferring region for session {session_id} ({latitude}, {longitude})");
            match reverse_geocode(&client, *latitude, *longitude) {
                Ok(found) => {
                    if region.country.is_empty() {
                        region.country = found.country;
                    }
                    if region.state.is_empty() {
                        region.state = found.state;
                    }
                    cache.insert(key, region.clone());
                }
                Err(error) => {
                    eprintln!("warning: could not infer region for session {session_id}: {error}")
                }
            }
            last_request = Some(Instant::now());
        }
        if let Some(country) = explicit_country.as_ref() {
            region.country = country.to_uppercase();
        }
        if let Some(state) = explicit_state.as_ref() {
            region.state = state.to_uppercase();
        }
        result.insert(session_id.clone(), region);
    }
    if !cache.is_empty() {
        ensure_parent(cache_path)?;
        let mut file = File::create(cache_path)
            .with_context(|| format!("write region cache {}", cache_path.display()))?;
        serde_json::to_writer_pretty(&mut file, &cache).context("serialize region cache")?;
        file.write_all(b"\n")?;
    }
    Ok(result)
}

fn reverse_geocode(
    client: &reqwest::blocking::Client,
    latitude: f64,
    longitude: f64,
) -> Result<Region> {
    let response: NominatimResponse = client
        .get("https://nominatim.openstreetmap.org/reverse")
        .query(&[
            ("format", "jsonv2"),
            ("lat", &latitude.to_string()),
            ("lon", &longitude.to_string()),
            ("zoom", "10"),
            ("addressdetails", "1"),
        ])
        .send()
        .context("request Nominatim")?
        .error_for_status()
        .context("Nominatim returned an error")?
        .json()
        .context("decode Nominatim response")?;
    let address = response
        .address
        .context("Nominatim response has no address")?;
    let country = address.country_code.unwrap_or_default().to_uppercase();
    let state = address
        .values
        .iter()
        .filter_map(|(key, value)| {
            key.strip_prefix("ISO3166-2-lvl")
                .map(|level| (level, value))
        })
        .filter_map(|(_, value)| value.as_str())
        .find_map(|value| {
            value
                .split_once('-')
                .map(|(_, suffix)| suffix.to_uppercase())
        })
        .unwrap_or_default();
    Ok(Region { country, state })
}

fn clean(value: &str) -> String {
    value.replace('"', "'").replace(['\r', '\n'], " ")
}

fn location_label(base: &str, latitude: f64, longitude: f64) -> String {
    format!("{} ({latitude:.5}, {longitude:.5})", clean(base))
}

fn ebird_common_name(name: &str) -> String {
    match name {
        "Australian White Ibis" => "Australian Ibis".to_string(),
        "Willie Wagtail" => "Willie-wagtail".to_string(),
        _ => clean(name),
    }
}

fn split_scientific_name(name: &str) -> (String, String) {
    let mut parts = name.splitn(2, char::is_whitespace);
    (
        parts.next().unwrap_or_default().to_string(),
        parts.next().unwrap_or_default().trim().to_string(),
    )
}

fn read_input(path: &Path) -> Result<Vec<u8>> {
    if path
        .extension()
        .and_then(|x| x.to_str())
        .is_some_and(|x| x.eq_ignore_ascii_case("zip"))
    {
        let file = File::open(path).with_context(|| format!("open {}", path.display()))?;
        let mut archive = ZipArchive::new(file).context("read ZIP export")?;
        let index = (0..archive.len())
            .find(|&i| {
                archive
                    .by_index(i)
                    .map(|f| f.name().to_ascii_lowercase().ends_with(".csv"))
                    .unwrap_or(false)
            })
            .context("ZIP contains no CSV file")?;
        let mut csv = archive.by_index(index).context("open CSV in ZIP")?;
        let mut bytes = Vec::new();
        csv.read_to_end(&mut bytes).context("read CSV in ZIP")?;
        Ok(bytes)
    } else {
        let mut bytes = Vec::new();
        File::open(path)
            .with_context(|| format!("open {}", path.display()))?
            .read_to_end(&mut bytes)
            .context("read CSV")?;
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_binomial_scientific_name() {
        assert_eq!(
            split_scientific_name("Trichoglossus moluccanus"),
            ("Trichoglossus".into(), "moluccanus".into())
        );
        assert_eq!(split_scientific_name("Genus"), ("Genus".into(), "".into()));
    }

    #[test]
    fn cleans_quotes_and_line_breaks() {
        assert_eq!(clean("a \"quoted\"\ncomment"), "a 'quoted' comment");
    }

    #[test]
    fn normalizes_ebird_common_name_aliases() {
        assert_eq!(
            ebird_common_name("Australian White Ibis"),
            "Australian Ibis"
        );
        assert_eq!(ebird_common_name("Willie Wagtail"), "Willie-wagtail");
        assert_eq!(ebird_common_name("Rainbow Lorikeet"), "Rainbow Lorikeet");
    }

    #[test]
    fn location_labels_keep_coordinates_distinct() {
        assert_eq!(
            location_label("Birda import", -27.4698, 153.0251),
            "Birda import (-27.46980, 153.02510)"
        );
        assert_ne!(
            location_label("Birda import", -27.4698, 153.0251),
            location_label("Birda import", -33.8688, 151.2093)
        );
    }

    #[test]
    fn protocol_names_match_ebird_values() {
        assert_eq!(Protocol::Incidental.as_str(), "Incidental");
        assert_eq!(Protocol::Historical.as_str(), "Historical");
    }

    #[test]
    fn config_file_overrides_defaults() {
        let path =
            std::env::temp_dir().join(format!("birda-to-ebird-test-{}.toml", std::process::id()));
        std::fs::write(&path, "timezone = \"Australia/Brisbane\"\nlocation_name = \"Test location\"\nprotocol = \"Historical\"\n").unwrap();
        let config = load_config(Some(&path)).unwrap();
        assert_eq!(config.timezone.as_deref(), Some("Australia/Brisbane"));
        assert_eq!(config.location_name, "Test location");
        assert_eq!(config.protocol.as_str(), "Historical");
        let _ = std::fs::remove_file(path);
    }
}
