//! Catalogs and icons (`spec/v0/catalog.md`): what a store reads from a catalog file, lists
//! across several, and shows of an icon. Nothing here fetches anything: a catalog's text and
//! the source it came from are passed in.

use alloc::collections::BTreeMap;

use serde_json::Value;

use crate::error::{Class, Error};
use crate::limits::Settings;
use crate::names::{normalize_name, valid_overlay_name, valid_relative_path, valid_tool_name};
use crate::read::{Document, parse_payload, read_document};
use crate::source::{Source, SourceKind};
use crate::validate::{about_field_valid, catalog_field_valid, is_digest, is_git_ref};

/// Why a store does not list a catalog entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum EntryRefusal {
    /// A name that is no overlay name, or no skill name.
    InvalidName,
    /// A name two keys of the catalog share once normalized.
    DuplicateName,
    /// An entry that is neither a source string nor a table with a `source` and known keys.
    InvalidEntry,
    /// A source string AOS refuses, or a location that is neither Git nor the web.
    InvalidSource,
    /// A `pin` that is no digest.
    InvalidPin,
    /// A `ref` on a source that takes none.
    RefNotAllowed,
    /// A relative source that leaves the repository the catalog is in.
    LeavesRepository,
    /// A source on this computer in a catalog from elsewhere.
    LocalSource,
    /// A source that shows a credential.
    SourceCredentials,
}

impl EntryRefusal {}

/// An entry a store lists, its source made absolute.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct CatalogEntry {
    /// Where it comes from, absolute.
    pub source: Source,
    /// The tree digest it must have.
    pub pin: Option<String>,
    /// What it says it is.
    pub description: Option<String>,
    /// Its valid informational fields.
    pub about: Option<Document>,
}

/// The entries of one table of a catalog and those refused.
#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct CatalogSection {
    /// The entries listed, by name as the catalog writes it.
    pub entries: BTreeMap<String, CatalogEntry>,
    /// The entries refused, by name.
    pub refused: Vec<(String, EntryRefusal)>,
}

/// Whether every entry of a catalog is a source string.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum CatalogForm {
    /// Every entry is a string.
    Minimal,
    /// At least one entry is a table.
    Full,
}

/// What a store reads from one catalog.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct Catalog {
    /// The form.
    pub form: CatalogForm,
    /// The catalog's `name`, when valid.
    pub name: Option<String>,
    /// Its `description`, when valid.
    pub description: Option<String>,
    /// Its `homepage`, when valid.
    pub homepage: Option<String>,
    /// The overlays it lists.
    pub overlays: CatalogSection,
    /// The skills it lists, when it has a `skills` table.
    pub skills: Option<CatalogSection>,
    /// The pointers of informational fields left out, sorted.
    pub ignored: Vec<String>,
}

/// The format a catalog's file name selects, or none for the rule of an AOS document.
#[must_use]
pub fn catalog_format(name: &str) -> Option<&'static str> {
    let last = name.replace('\\', "/");
    let last = last.rsplit('/').next().unwrap_or_default();
    let (_, extension) = last.rsplit_once('.')?;
    match extension.to_ascii_lowercase().as_str() {
        "toml" => Some("toml"),
        "json" => Some("json"),
        "yaml" | "yml" => Some("yaml"),
        _ => None,
    }
}

/// A catalog's data model value: its file name's extension names the format, else the rule of an AOS document.
///
/// # Errors
///
/// The reading classes, and `too-large` past `catalog_bytes`.
pub fn read_catalog_document(
    data: &[u8],
    name: &str,
    settings: &Settings,
) -> Result<Document, Error> {
    let bounded = Settings {
        manifest_bytes: settings.catalog_bytes,
        ..*settings
    };
    let Some(format) = catalog_format(name) else {
        return read_document(data, &bounded);
    };
    if !bounded.manifest_bytes.allows(data.len()) {
        return Err(Error::new(Class::TooLarge, "a catalog"));
    }
    let data = data.strip_prefix(b"\xef\xbb\xbf").unwrap_or(data);
    parse_payload(data, format, &bounded, false)
}

fn escape(token: &str) -> String {
    token.replace('~', "~0").replace('/', "~1")
}

fn has_drive(location: &str) -> bool {
    let mut chars = location.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic()) && chars.next() == Some(':')
}

fn has_scheme(location: &str) -> bool {
    let Some((scheme, _)) = location.split_once(':') else {
        return false;
    };
    let mut chars = scheme.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && scheme.len() >= 2
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'))
}

fn absolute_directory(location: &str) -> bool {
    location.starts_with(['/', '\\']) || has_drive(location)
}

/// `posixpath.normpath`.
fn normpath(path: &str) -> String {
    let rooted = path.starts_with('/');
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if parts.last().is_some_and(|last| *last != "..") {
                    parts.pop();
                } else if !rooted {
                    parts.push("..");
                }
            }
            other => parts.push(other),
        }
    }
    let joined = parts.join("/");
    match (rooted, joined.is_empty()) {
        (true, _) => format!("/{joined}"),
        (false, true) => ".".to_owned(),
        (false, false) => joined,
    }
}

/// `posixpath.normpath(posixpath.join(*parts))` over the parts that are not empty.
fn join(parts: &[&str]) -> String {
    let mut out = String::new();
    for part in parts.iter().filter(|part| !part.is_empty()) {
        if part.starts_with('/') {
            out = (*part).to_owned();
        } else if out.is_empty() || out.ends_with('/') {
            out.push_str(part);
        } else {
            out.push('/');
            out.push_str(part);
        }
    }
    normpath(if out.is_empty() { "." } else { &out })
}

/// `posixpath.dirname`.
fn dirname(path: &str) -> String {
    let cut = path.rfind('/').map_or(0, |at| at + 1);
    let head = &path[..cut];
    if !head.is_empty() && head.chars().any(|c| c != '/') {
        head.trim_end_matches('/').to_owned()
    } else {
        head.to_owned()
    }
}

/// `urllib.parse.urljoin` for a base with an authority.
fn url_join(base: &str, reference: &str) -> String {
    if reference.is_empty() {
        return base.to_owned();
    }
    if has_scheme(reference) {
        return reference.to_owned();
    }
    let Some((scheme, rest)) = base.split_once("://") else {
        return reference.to_owned();
    };
    let cut = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(cut);
    if let Some(network) = reference.strip_prefix("//") {
        return format!("{scheme}://{network}");
    }
    let tail = tail.split('#').next().unwrap_or_default();
    let (base_path, base_query) = tail.split_once('?').unwrap_or((tail, ""));
    let (ref_path, ref_suffix) = match reference.find(['?', '#']) {
        Some(at) => reference.split_at(at),
        None => (reference, ""),
    };
    let path = if ref_path.is_empty() {
        let mut path = base_path.to_owned();
        if ref_suffix.starts_with('#') && !base_query.is_empty() {
            path.push('?');
            path.push_str(base_query);
        }
        return format!("{scheme}://{authority}{path}{ref_suffix}");
    } else if ref_path.starts_with('/') {
        remove_dots(ref_path)
    } else {
        let head = &base_path[..base_path.rfind('/').map_or(0, |at| at + 1)];
        let head = if head.is_empty() && !authority.is_empty() {
            "/"
        } else {
            head
        };
        remove_dots(&format!("{head}{ref_path}"))
    };
    format!("{scheme}://{authority}{path}{ref_suffix}")
}

fn remove_dots(path: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    let segments: Vec<&str> = path.split('/').collect();
    let last = segments.len().saturating_sub(1);
    for (index, segment) in segments.iter().enumerate() {
        match *segment {
            "." => {
                if index == last {
                    out.push("");
                }
            }
            ".." => {
                if out.len() > 1 {
                    out.pop();
                }
                if index == last {
                    out.push("");
                }
            }
            other => out.push(other),
        }
    }
    let joined = out.join("/");
    if joined.starts_with('/') {
        joined
    } else {
        format!("/{joined}")
    }
}

fn resolve_entry_source(entry: &Source, origin: Option<&Source>) -> Result<Source, EntryRefusal> {
    let location = entry.location.as_str();
    let relative = entry.kind == SourceKind::Directory
        && !absolute_directory(location)
        && !has_scheme(location);
    let local = !relative && entry.is_local();
    let Some(origin) = origin.filter(|origin| origin.kind != SourceKind::Directory) else {
        if relative && let Some(origin) = origin {
            let folder = dirname(&origin.location.replace('\\', "/"));
            let mut resolved = entry.clone();
            resolved.location = join(&[&folder, location]);
            return Ok(resolved);
        }
        return Ok(entry.clone());
    };
    if local {
        return Err(EntryRefusal::LocalSource);
    }
    if !relative {
        if entry.kind != SourceKind::Git && !entry.is_web_url() {
            return Err(EntryRefusal::InvalidSource);
        }
        if entry.holds_credentials() {
            return Err(EntryRefusal::SourceCredentials);
        }
        return Ok(entry.clone());
    }
    if origin.kind == SourceKind::Git {
        let folder = dirname(origin.path.as_deref().unwrap_or_default());
        let path = join(&[&folder, location, entry.path.as_deref().unwrap_or_default()]);
        if path == ".." || path.starts_with("../") {
            return Err(EntryRefusal::LeavesRepository);
        }
        if path != "." && !valid_relative_path(&path) {
            return Err(EntryRefusal::InvalidSource);
        }
        let mut resolved = origin.clone();
        resolved.path = (path != ".").then_some(path);
        return Ok(resolved);
    }
    let mut resolved = entry.clone();
    resolved.kind = SourceKind::Url;
    resolved.location = url_join(&origin.location, location);
    resolved.reference = None;
    Ok(resolved)
}

fn store_about(about: &Value, at: &str, ignored: &mut Vec<String>) -> Option<Document> {
    let Some(map) = about.as_object() else {
        ignored.push(at.to_owned());
        return None;
    };
    let mut shown = Document::new();
    for (field, value) in map {
        if about_field_valid(field, value) {
            shown.insert(field.clone(), value.clone());
        } else {
            ignored.push(format!("{at}/{}", escape(field)));
        }
    }
    (!shown.is_empty()).then_some(shown)
}

fn catalog_entry(
    value: &Value,
    at: &str,
    origin: Option<&Source>,
    ignored: &mut Vec<String>,
) -> Result<CatalogEntry, EntryRefusal> {
    let single;
    let fields = match value {
        Value::String(source) => {
            single = Document::from_iter([("source".to_owned(), Value::String(source.clone()))]);
            &single
        }
        Value::Object(map) => {
            if map.keys().any(|key| {
                !matches!(
                    key.as_str(),
                    "source" | "ref" | "pin" | "description" | "about"
                )
            }) {
                return Err(EntryRefusal::InvalidEntry);
            }
            map
        }
        _ => return Err(EntryRefusal::InvalidEntry),
    };
    let Some(text) = fields.get("source").and_then(Value::as_str) else {
        return Err(EntryRefusal::InvalidEntry);
    };
    let parsed = Source::parse(text).map_err(|_| EntryRefusal::InvalidSource)?;
    let mut source = resolve_entry_source(&parsed, origin)?;
    if let Some(reference) = fields.get("ref") {
        let Some(reference) = reference.as_str().filter(|reference| is_git_ref(reference)) else {
            return Err(EntryRefusal::InvalidEntry);
        };
        if parsed.kind != SourceKind::Git || parsed.reference.is_some() {
            return Err(EntryRefusal::RefNotAllowed);
        }
        source.reference = Some(reference.to_owned());
    }
    let mut entry = CatalogEntry {
        source,
        pin: None,
        description: None,
        about: None,
    };
    if let Some(pin) = fields.get("pin") {
        match pin.as_str() {
            Some(pin) if is_digest(pin) => entry.pin = Some(pin.to_owned()),
            _ => return Err(EntryRefusal::InvalidPin),
        }
    }
    if let Some(description) = fields.get("description") {
        match description.as_str() {
            Some(text) => entry.description = Some(text.to_owned()),
            None => ignored.push(format!("{at}/description")),
        }
    }
    if let Some(about) = fields.get("about") {
        entry.about = store_about(about, &format!("{at}/about"), ignored);
    }
    Ok(entry)
}

fn section(
    table: &Document,
    prefix: &str,
    name_ok: impl Fn(&str) -> bool,
    fold: impl Fn(&str) -> String,
    origin: Option<&Source>,
    ignored: &mut Vec<String>,
) -> CatalogSection {
    let mut holders: BTreeMap<String, usize> = BTreeMap::new();
    for key in table.keys().filter(|key| name_ok(key)) {
        *holders.entry(fold(key)).or_default() += 1;
    }
    let mut out = CatalogSection::default();
    let mut keys: Vec<&String> = table.keys().collect();
    keys.sort();
    for key in keys {
        if !name_ok(key) {
            out.refused.push((key.clone(), EntryRefusal::InvalidName));
        } else if holders[&fold(key)] > 1 {
            out.refused.push((key.clone(), EntryRefusal::DuplicateName));
        } else {
            let at = format!("{prefix}/{}", escape(key));
            match catalog_entry(&table[key], &at, origin, ignored) {
                Ok(entry) => {
                    out.entries.insert(key.clone(), entry);
                }
                Err(refusal) => out.refused.push((key.clone(), refusal)),
            }
        }
    }
    out
}

/// What a store reads from one catalog: its form, informational fields, the entries it lists with
/// their sources made absolute, the entries it refuses and the fields it ignores.
///
/// `name` is the catalog's file name, which selects the format; `base` is the source string the
/// catalog file itself came from, against which relative sources resolve. Without a `base` nothing
/// is made absolute and no source is refused as local or for a credential: pass one for any
/// catalog that was not read from the user's own disk. A description is the catalog's own text:
/// a store must not render it as markup.
///
/// # Errors
///
/// The reading classes of the whole catalog, `too-large` past `catalog_bytes`, and
/// `not-a-catalog` for a document with neither an `overlays` nor a `skills` table.
pub fn read_catalog(
    data: &[u8],
    name: &str,
    base: Option<&str>,
    settings: &Settings,
) -> Result<Catalog, Error> {
    let document = read_catalog_document(data, name, settings)?;
    let overlays = document.get("overlays").and_then(Value::as_object);
    let skills = document.get("skills").and_then(Value::as_object);
    if overlays.is_none() && skills.is_none() {
        return Err(Error::new(Class::NotACatalog, ""));
    }
    let origin = match base {
        Some(base) => Some(
            Source::parse(base)
                .map_err(|_| Error::new(Class::InvalidSource, "the catalog's base"))?,
        ),
        None => None,
    };
    let mut ignored = Vec::new();
    if overlays.is_none() && document.contains_key("overlays") {
        ignored.push("/overlays".to_owned());
    }
    let listed = overlays
        .into_iter()
        .chain(skills)
        .flat_map(|table| table.values());
    let form = if listed.into_iter().all(Value::is_string) {
        CatalogForm::Minimal
    } else {
        CatalogForm::Full
    };
    let mut fields: BTreeMap<&str, Option<String>> = BTreeMap::new();
    for field in ["name", "description", "homepage"] {
        if let Some(value) = document.get(field) {
            if catalog_field_valid(field, value) {
                fields.insert(field, value.as_str().map(str::to_owned));
            } else {
                ignored.push(format!("/{field}"));
            }
        }
    }
    let empty = Document::new();
    let overlay_section = section(
        overlays.unwrap_or(&empty),
        "/overlays",
        valid_overlay_name,
        normalize_name,
        origin.as_ref(),
        &mut ignored,
    );
    let skill_section = match skills {
        Some(table) => Some(section(
            table,
            "/skills",
            |name| valid_tool_name(name) && valid_relative_path(name),
            |name| name.to_lowercase(),
            origin.as_ref(),
            &mut ignored,
        )),
        None => {
            if document.contains_key("skills") {
                ignored.push("/skills".to_owned());
            }
            None
        }
    };
    ignored.sort();
    Ok(Catalog {
        form,
        name: fields.remove("name").flatten(),
        description: fields.remove("description").flatten(),
        homepage: fields.remove("homepage").flatten(),
        overlays: overlay_section,
        skills: skill_section,
        ignored,
    })
}

/// One entry of a [`Listing`].
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct ListedEntry {
    /// The catalog's label.
    pub catalog: String,
    /// The name as the catalog writes it.
    pub name: String,
    /// The name normalized.
    pub identity: String,
    /// The name as shown: qualified `<label>/<name>` when more than one catalog offers it.
    pub shown: String,
}

/// A store's listing of several catalogs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Listing {
    /// Every entry, by identity then label.
    pub entries: Vec<ListedEntry>,
    /// The catalogs that offer each identity more than one catalog offers.
    pub collisions: BTreeMap<String, Vec<String>>,
}

/// Lists several catalogs, each with the label the consumer gave it. An identity two catalogs
/// offer is shown once per catalog, qualified by its label; entries are never merged.
///
/// # Errors
///
/// `invalid-label` for a label that is no overlay name, or two equal once normalized.
pub fn list_catalogs(catalogs: &[(&str, &Catalog)]) -> Result<Listing, Error> {
    let mut seen = alloc::collections::BTreeSet::new();
    for (label, _) in catalogs {
        if !valid_overlay_name(label) || !seen.insert(normalize_name(label)) {
            return Err(Error::new(Class::InvalidLabel, ""));
        }
    }
    let mut offered: BTreeMap<String, Vec<(&str, &str)>> = BTreeMap::new();
    for (label, catalog) in catalogs {
        for name in catalog.overlays.entries.keys() {
            offered
                .entry(normalize_name(name))
                .or_default()
                .push((label, name));
        }
    }
    let mut listing = Listing::default();
    for (identity, mut holders) in offered {
        holders.sort_unstable();
        let qualified = holders.len() > 1;
        if qualified {
            listing.collisions.insert(
                identity.clone(),
                holders
                    .iter()
                    .map(|(label, _)| (*label).to_owned())
                    .collect(),
            );
        }
        for (label, name) in holders {
            listing.entries.push(ListedEntry {
                catalog: label.to_owned(),
                name: name.to_owned(),
                identity: identity.clone(),
                shown: if qualified {
                    format!("{label}/{name}")
                } else {
                    name.to_owned()
                },
            });
        }
    }
    Ok(listing)
}

impl Listing {
    /// The entry a requested name picks: `<label>/<name>`, or a name one catalog alone offers.
    ///
    /// # Errors
    ///
    /// `not-found`, or `ambiguous` when more than one catalog offers an unqualified name.
    pub fn choose(&self, requested: &str) -> Result<&ListedEntry, Error> {
        let (label, name) = match requested.rsplit_once('/') {
            Some((label, name)) => (Some(label), name),
            None => (None, requested),
        };
        if !valid_overlay_name(name) || label.is_some_and(|label| !valid_overlay_name(label)) {
            return Err(Error::new(Class::NotFound, ""));
        }
        let mut found = self.entries.iter().filter(|entry| {
            entry.identity == normalize_name(name)
                && label.is_none_or(|label| normalize_name(&entry.catalog) == normalize_name(label))
        });
        let first = found
            .next()
            .ok_or_else(|| Error::new(Class::NotFound, ""))?;
        if found.next().is_some() {
            return Err(Error::new(Class::Ambiguous, ""));
        }
        Ok(first)
    }
}

/// The image formats an icon may have.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum IconFormat {
    /// PNG.
    Png,
    /// WebP, lossy, lossless or extended.
    Webp,
}

impl IconFormat {}

/// An icon's format and size, from its header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Icon {
    /// The format.
    pub format: IconFormat,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
}

fn le(data: &[u8]) -> u32 {
    data.iter()
        .rev()
        .fold(0u32, |value, byte| (value << 8) | u32::from(*byte))
}

fn png_size(data: &[u8]) -> Result<(u32, u32), Error> {
    if data.len() < 24 || data[8..16] != *b"\x00\x00\x00\x0dIHDR" {
        return Err(Error::new(Class::IconUnreadable, ""));
    }
    let be = |bytes: &[u8]| {
        bytes
            .iter()
            .fold(0u32, |value, byte| (value << 8) | u32::from(*byte))
    };
    Ok((be(&data[16..20]), be(&data[20..24])))
}

fn webp_size(data: &[u8]) -> Result<(u32, u32), Error> {
    let unreadable = || Error::new(Class::IconUnreadable, "");
    if data.len() < 20 || u64::from(le(&data[4..8])) + 8 > data.len() as u64 {
        return Err(unreadable());
    }
    let chunk = &data[12..16];
    if chunk == b"VP8 " && data.len() >= 30 && data[23..26] == *b"\x9d\x01\x2a" {
        return Ok((le(&data[26..28]) & 0x3FFF, le(&data[28..30]) & 0x3FFF));
    }
    if chunk == b"VP8L" && data.len() >= 25 && data[20] == 0x2F {
        let bits = le(&data[21..25]);
        return Ok(((bits & 0x3FFF) + 1, ((bits >> 14) & 0x3FFF) + 1));
    }
    if chunk == b"VP8X" && data.len() >= 30 {
        return Ok((le(&data[24..27]) + 1, le(&data[27..30]) + 1));
    }
    Err(unreadable())
}

/// An icon's format and dimensions from its header, or why a store refuses it. The checks run in
/// order: the name's extension, the size in bytes, the signature against the extension, the
/// header, the dimensions. Nothing past the header is decoded.
///
/// # Errors
///
/// `icon-format`, `icon-too-large`, `icon-mismatch`, `icon-unreadable` or `icon-dimensions`.
pub fn read_icon(data: &[u8], name: &str, settings: &Settings) -> Result<Icon, Error> {
    let last = name.rsplit('/').next().unwrap_or_default();
    let extension = last
        .rsplit_once('.')
        .map_or(String::new(), |(_, extension)| {
            extension.to_ascii_lowercase()
        });
    if !matches!(extension.as_str(), "png" | "webp") {
        return Err(Error::new(Class::IconFormat, ""));
    }
    if !settings.icon_bytes.allows(data.len()) {
        return Err(Error::new(Class::IconTooLarge, ""));
    }
    let format = if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        IconFormat::Png
    } else if data.get(..4) == Some(b"RIFF") && data.get(8..12) == Some(b"WEBP") {
        IconFormat::Webp
    } else {
        return Err(Error::new(Class::IconFormat, ""));
    };
    if format.as_str() != extension {
        return Err(Error::new(Class::IconMismatch, ""));
    }
    let (width, height) = match format {
        IconFormat::Png => png_size(data)?,
        IconFormat::Webp => webp_size(data)?,
    };
    if width == 0 || height == 0 {
        return Err(Error::new(Class::IconUnreadable, ""));
    }
    if !settings.icon_width.allows(width) || !settings.icon_height.allows(height) {
        return Err(Error::new(Class::IconDimensions, ""));
    }
    Ok(Icon {
        format,
        width,
        height,
    })
}

impl CatalogForm {
    /// The word the specification and its vectors write.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Minimal => "minimal",
            Self::Full => "full",
        }
    }

    /// What a word stands for.
    #[must_use]
    pub fn named(word: &str) -> Option<Self> {
        match word {
            "minimal" => Some(Self::Minimal),
            "full" => Some(Self::Full),
            _ => None,
        }
    }
}

impl EntryRefusal {
    /// The class the specification's vectors name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidName => "invalid-name",
            Self::DuplicateName => "duplicate-name",
            Self::InvalidEntry => "invalid-entry",
            Self::InvalidSource => "invalid-source",
            Self::InvalidPin => "invalid-pin",
            Self::RefNotAllowed => "ref-not-allowed",
            Self::LeavesRepository => "leaves-repository",
            Self::LocalSource => "local-source",
            Self::SourceCredentials => "source-credentials",
        }
    }

    /// What a word stands for.
    #[must_use]
    pub fn named(word: &str) -> Option<Self> {
        match word {
            "invalid-name" => Some(Self::InvalidName),
            "duplicate-name" => Some(Self::DuplicateName),
            "invalid-entry" => Some(Self::InvalidEntry),
            "invalid-source" => Some(Self::InvalidSource),
            "invalid-pin" => Some(Self::InvalidPin),
            "ref-not-allowed" => Some(Self::RefNotAllowed),
            "leaves-repository" => Some(Self::LeavesRepository),
            "local-source" => Some(Self::LocalSource),
            "source-credentials" => Some(Self::SourceCredentials),
            _ => None,
        }
    }
}

impl IconFormat {
    /// `png` or `webp`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Webp => "webp",
        }
    }

    /// What a word stands for.
    #[must_use]
    pub fn named(word: &str) -> Option<Self> {
        match word {
            "png" => Some(Self::Png),
            "webp" => Some(Self::Webp),
            _ => None,
        }
    }
}
