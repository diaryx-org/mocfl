//! `inventory.json` — the file that makes an OCFL object self-describing.
//!
//! The inventory is the whole point of the layout. It records, in one
//! human-readable JSON document, every version an object has ever had and the
//! digest of every file in each of them. Given the inventory and the content
//! files, a repository can be rebuilt with no database and no application.
//!
//! ## The two maps, and why there are two
//!
//! - **`manifest`** maps a digest to the *content paths* where those bytes are
//!   physically stored (`v1/content/letter.md`). One digest, many paths: bytes
//!   stored once and referenced from anywhere.
//! - **`versions[vN].state`** maps a digest to the *logical paths* the user sees
//!   at that version (`letter.md`). This is the layer where renames, deletions,
//!   and restorations happen — all without moving a byte.
//!
//! A version's state is the **complete** logical state at that version, not a
//! delta against its predecessor. Nothing needs folding to read version N, which
//! is what makes an object readable by a script and robust to a missing
//! neighbour.

use std::collections::BTreeMap;
use std::fmt;

use fig::{Format, SerializeOptions, Value};

use crate::digest::{Digest, DigestAlgorithm};
use crate::error::{Error, Result};

/// The `type` URI this crate writes. Names the spec version an inventory claims
/// conformance to.
pub const INVENTORY_TYPE: &str = "https://ocfl.io/1.1/spec/#inventory";

/// The default `contentDirectory` when an inventory does not name one.
pub const DEFAULT_CONTENT_DIRECTORY: &str = "content";

/// An OCFL version number: `v1`, `v2`, … or the zero-padded `v0001` form.
///
/// Padding is carried because the spec requires an object to be *consistent*: an
/// object using `v0001` must use that width for every version, so the width has
/// to survive a read/write round-trip rather than being normalized away.
///
/// **Identity is the number alone.** Padding is presentation, and it is not
/// always recoverable from a single name — `v1000` in a zero-padded object is
/// indistinguishable from `v1000` in an unpadded one, since only a leading zero
/// marks the padded form. Equality, hashing, and ordering therefore all ignore
/// it. That is not merely convenient: `Ord` compares numerically, so deriving
/// `PartialEq` over the padding too would let `cmp` report `Equal` for two values
/// that `==` calls different, which breaks the `Ord`/`Eq` contract and quietly
/// corrupts their behaviour as map keys.
#[derive(Debug, Clone, Copy)]
pub struct VersionNum {
    number: u32,
    /// Total digit count for the padded form, or `0` for the unpadded `v1` style.
    padding: usize,
}

impl PartialEq for VersionNum {
    fn eq(&self, other: &Self) -> bool {
        self.number == other.number
    }
}

impl Eq for VersionNum {}

impl std::hash::Hash for VersionNum {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.number.hash(state);
    }
}

impl VersionNum {
    /// The first version of any object.
    pub const FIRST: Self = Self {
        number: 1,
        padding: 0,
    };

    /// An unpadded version number (`v3`).
    pub fn new(number: u32) -> Self {
        Self { number, padding: 0 }
    }

    /// The numeric value, ignoring padding.
    pub fn number(self) -> u32 {
        self.number
    }

    /// The digit width of the padded form, or `0` when unpadded.
    pub fn padding(self) -> usize {
        self.padding
    }

    /// The next version, keeping this one's padding convention.
    ///
    /// A padded scheme has a hard ceiling — `v0001` cannot represent 10000 — and
    /// the spec forbids widening mid-object, so overflowing it is a real error
    /// rather than something to paper over.
    pub fn next(self) -> Result<Self> {
        let number = self.number + 1;
        if self.padding != 0 && number.to_string().len() > self.padding - 1 {
            return Err(Error::Conflict(format!(
                "version {self} exhausts its zero-padded width; the spec forbids widening it"
            )));
        }
        Ok(Self {
            number,
            padding: self.padding,
        })
    }

    /// Parse a version directory name (`v1`, `v0001`).
    ///
    /// Rejects `v0` (numbering starts at 1) and any padded form whose first digit
    /// is nonzero (`v01` is padded, `v11` is not — the distinction is whether a
    /// leading zero is present).
    pub fn parse(name: &str) -> Result<Self> {
        let malformed = || Error::MalformedVersion(name.to_string());
        let digits = name.strip_prefix('v').ok_or_else(malformed)?;
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return Err(malformed());
        }
        let number: u32 = digits.parse().map_err(|_| malformed())?;
        if number == 0 {
            return Err(malformed());
        }
        let padding = if digits.starts_with('0') {
            name.len()
        } else {
            0
        };
        Ok(Self { number, padding })
    }
}

impl fmt::Display for VersionNum {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.padding == 0 {
            write!(f, "v{}", self.number)
        } else {
            write!(f, "v{:0width$}", self.number, width = self.padding - 1)
        }
    }
}

// Ordering is numeric, never lexical — `v10` follows `v9`.
impl PartialOrd for VersionNum {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for VersionNum {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.number.cmp(&other.number)
    }
}

/// Who made a version. Optional in the spec; the field that turns a version
/// history into a provenance record rather than a byte log.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct User {
    /// A human-readable name.
    pub name: Option<String>,
    /// A URI identifying the agent — `mailto:`, an ORCID, an app identifier.
    pub address: Option<String>,
}

impl User {
    /// Whether there is nothing here worth writing.
    pub fn is_empty(&self) -> bool {
        self.name.is_none() && self.address.is_none()
    }
}

/// One version of an object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    /// When the version was created, RFC 3339 with a timezone. Required.
    ///
    /// Held as a string rather than a parsed instant on purpose: this crate has
    /// no clock and no date dependency, the caller supplies the timestamp, and a
    /// value read from a foreign object round-trips byte-for-byte instead of
    /// being reformatted by a parser with different ideas about offsets.
    pub created: String,
    /// A free-text note about the version.
    pub message: Option<String>,
    /// The agent responsible.
    pub user: User,
    /// Digest → the logical paths holding those bytes at this version. The
    /// complete state, not a delta.
    pub state: BTreeMap<Digest, Vec<String>>,
}

impl Version {
    /// The logical paths in this version, sorted, paired with their digests.
    ///
    /// The inverse of [`Version::state`], and the view a caller usually wants:
    /// state is keyed by digest because that is what dedupes, but a person reads
    /// a version as a list of files.
    pub fn files(&self) -> BTreeMap<&str, &Digest> {
        let mut out = BTreeMap::new();
        for (digest, paths) in &self.state {
            for path in paths {
                out.insert(path.as_str(), digest);
            }
        }
        out
    }
}

/// A parsed `inventory.json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inventory {
    /// The object's identifier — a URI, an ARK, any opaque string.
    pub id: String,
    /// The `type` URI naming the spec version.
    pub type_uri: String,
    /// The algorithm the manifest and every state are keyed on.
    pub digest_algorithm: DigestAlgorithm,
    /// The most recent version. Must name a key of `versions`.
    pub head: VersionNum,
    /// The directory inside each version holding content (`content` by default).
    pub content_directory: String,
    /// Digest → the content paths storing those bytes.
    pub manifest: BTreeMap<Digest, Vec<String>>,
    /// Every version, keyed numerically.
    pub versions: BTreeMap<VersionNum, Version>,
    /// Supplementary digests, keyed by algorithm name then digest. Carried
    /// verbatim: this crate never computes them, but must not lose them when it
    /// rewrites an inventory it did not author.
    pub fixity: BTreeMap<String, BTreeMap<String, Vec<String>>>,
}

impl Inventory {
    /// A new, empty inventory with no versions yet.
    ///
    /// Not writable as-is — an object must have at least one version — so this is
    /// the starting point a first commit builds on, never something that reaches
    /// disk on its own.
    pub fn new(id: impl Into<String>, digest_algorithm: DigestAlgorithm) -> Self {
        Self {
            id: id.into(),
            type_uri: INVENTORY_TYPE.to_string(),
            digest_algorithm,
            head: VersionNum::FIRST,
            content_directory: DEFAULT_CONTENT_DIRECTORY.to_string(),
            manifest: BTreeMap::new(),
            versions: BTreeMap::new(),
            fixity: BTreeMap::new(),
        }
    }

    /// The version numbers, oldest first.
    pub fn version_numbers(&self) -> Vec<VersionNum> {
        self.versions.keys().copied().collect()
    }

    /// The most recent version, if the object has one.
    pub fn head_version(&self) -> Option<&Version> {
        self.versions.get(&self.head)
    }

    /// The content path storing `digest`, if the manifest knows it. When several
    /// paths hold the same bytes, the lexically first is returned so the choice
    /// is deterministic.
    pub fn content_path(&self, digest: &Digest) -> Option<&str> {
        self.manifest
            .get(digest)
            .and_then(|paths| paths.iter().min())
            .map(String::as_str)
    }

    // ---- JSON ----

    /// Parse inventory bytes.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let value = fig::Document::parse(bytes, Format::Json)?.to_value()?;
        Self::from_value(&value)
    }

    /// Render to the exact bytes that belong in `inventory.json`.
    ///
    /// Two-space pretty printing, because an inventory is meant to be *read* —
    /// by a person with a text editor, decades from now, with no OCFL tooling.
    /// The extra bytes are the cheapest part of the archival bargain.
    pub fn to_json(&self) -> Result<String> {
        let mut json = self
            .to_value()
            .serialize_with(Format::Json, SerializeOptions::pretty(2))?;
        if !json.ends_with('\n') {
            json.push('\n');
        }
        Ok(json)
    }

    fn to_value(&self) -> Value {
        // Field order follows the spec's own presentation. JSON objects are
        // unordered by definition, so this is for the human reader.
        let mut root: Vec<(Value, Value)> = vec![
            (str_key("id"), Value::Str(self.id.clone())),
            (str_key("type"), Value::Str(self.type_uri.clone())),
            (
                str_key("digestAlgorithm"),
                Value::Str(self.digest_algorithm.as_str().to_string()),
            ),
            (str_key("head"), Value::Str(self.head.to_string())),
            (
                str_key("contentDirectory"),
                Value::Str(self.content_directory.clone()),
            ),
            (str_key("manifest"), digest_map(&self.manifest)),
        ];

        let versions = self
            .versions
            .iter()
            .map(|(num, version)| (str_key(&num.to_string()), version_value(version)))
            .collect();
        root.push((str_key("versions"), Value::Map(versions)));

        if !self.fixity.is_empty() {
            let fixity = self
                .fixity
                .iter()
                .map(|(alg, entries)| {
                    let inner = entries
                        .iter()
                        .map(|(digest, paths)| (str_key(digest), str_seq(paths)))
                        .collect();
                    (str_key(alg), Value::Map(inner))
                })
                .collect();
            root.push((str_key("fixity"), Value::Map(fixity)));
        }

        Value::Map(root)
    }

    fn from_value(value: &Value) -> Result<Self> {
        let root = as_map(value, "<root>")?;

        let id = required_str(root, "id")?.to_string();
        let type_uri = required_str(root, "type")?.to_string();
        let algorithm_name = required_str(root, "digestAlgorithm")?;
        let digest_algorithm = DigestAlgorithm::parse(algorithm_name).ok_or_else(|| {
            Error::Inventory(format!(
                "digestAlgorithm must be sha256 or sha512 for content addressing, got {algorithm_name:?}"
            ))
        })?;
        let head = VersionNum::parse(required_str(root, "head")?)?;
        let content_directory = match get(root, "contentDirectory") {
            Some(v) => as_str(v, "contentDirectory")?.to_string(),
            None => DEFAULT_CONTENT_DIRECTORY.to_string(),
        };

        let manifest = parse_digest_map(
            get(root, "manifest").ok_or_else(|| missing("manifest"))?,
            "manifest",
        )?;

        let versions_value = get(root, "versions").ok_or_else(|| missing("versions"))?;
        let mut versions = BTreeMap::new();
        for (key, value) in as_map(versions_value, "versions")? {
            let name = as_str(key, "versions.<key>")?;
            let num = VersionNum::parse(name)?;
            versions.insert(num, parse_version(value, name)?);
        }

        let mut fixity = BTreeMap::new();
        if let Some(value) = get(root, "fixity") {
            for (key, entries) in as_map(value, "fixity")? {
                let alg = as_str(key, "fixity.<key>")?.to_string();
                let mut inner = BTreeMap::new();
                for (digest, paths) in as_map(entries, "fixity.<alg>")? {
                    inner.insert(
                        as_str(digest, "fixity.<alg>.<key>")?.to_string(),
                        parse_str_seq(paths, "fixity.<alg>.<digest>")?,
                    );
                }
                fixity.insert(alg, inner);
            }
        }

        // The head must name a real version. This is the one cross-field
        // invariant checked at parse time rather than left to validation: an
        // `Inventory` whose head dangles cannot answer the most basic question
        // asked of it ("what does this object look like now?"), so it is not a
        // value worth constructing.
        if !versions.contains_key(&head) {
            return Err(Error::Inventory(format!(
                "head names {head}, which is not among the versions"
            )));
        }

        Ok(Self {
            id,
            type_uri,
            digest_algorithm,
            head,
            content_directory,
            manifest,
            versions,
            fixity,
        })
    }
}

// ---- path rules ----

/// Check a logical or content path against the spec's restrictions.
///
/// Forbidden: empty paths, `.` and `..` segments, empty segments (which is what a
/// leading, trailing, or doubled slash produces), and backslashes. These are the
/// rules that stop a path in an inventory from escaping the object root when a
/// naive implementation joins it — a path in an archive is untrusted input.
pub fn check_path(path: &str) -> Result<()> {
    let illegal = || Error::IllegalPath(path.to_string());
    if path.is_empty() || path.contains('\\') {
        return Err(illegal());
    }
    for segment in path.split('/') {
        if segment.is_empty() || segment == "." || segment == ".." {
            return Err(illegal());
        }
    }
    Ok(())
}

// ---- fig value helpers ----
//
// Mapping is by hand rather than by derive. The spec's shape — digests as map
// keys, a `head` that must resolve, version names that must parse — is checked
// here, at the boundary, so an `Inventory` in memory is one that made sense.

fn str_key(s: &str) -> Value {
    Value::Str(s.to_string())
}

fn str_seq(items: &[String]) -> Value {
    Value::Seq(items.iter().cloned().map(Value::Str).collect())
}

fn digest_map(map: &BTreeMap<Digest, Vec<String>>) -> Value {
    Value::Map(
        map.iter()
            .map(|(digest, paths)| (str_key(digest.as_str()), str_seq(paths)))
            .collect(),
    )
}

fn version_value(version: &Version) -> Value {
    let mut fields: Vec<(Value, Value)> =
        vec![(str_key("created"), Value::Str(version.created.clone()))];
    if let Some(message) = &version.message {
        fields.push((str_key("message"), Value::Str(message.clone())));
    }
    if !version.user.is_empty() {
        let mut user: Vec<(Value, Value)> = Vec::new();
        if let Some(name) = &version.user.name {
            user.push((str_key("name"), Value::Str(name.clone())));
        }
        if let Some(address) = &version.user.address {
            user.push((str_key("address"), Value::Str(address.clone())));
        }
        fields.push((str_key("user"), Value::Map(user)));
    }
    fields.push((str_key("state"), digest_map(&version.state)));
    Value::Map(fields)
}

fn missing(field: &str) -> Error {
    Error::Inventory(format!("required field `{field}` is missing"))
}

fn as_map<'a>(value: &'a Value, at: &str) -> Result<&'a Vec<(Value, Value)>> {
    match value {
        Value::Map(entries) => Ok(entries),
        _ => Err(Error::Inventory(format!("{at} must be a JSON object"))),
    }
}

fn as_str<'a>(value: &'a Value, at: &str) -> Result<&'a str> {
    match value {
        Value::Str(s) => Ok(s),
        _ => Err(Error::Inventory(format!("{at} must be a string"))),
    }
}

fn get<'a>(map: &'a [(Value, Value)], key: &str) -> Option<&'a Value> {
    map.iter()
        .find(|(k, _)| matches!(k, Value::Str(s) if s == key))
        .map(|(_, v)| v)
}

fn required_str<'a>(map: &'a [(Value, Value)], key: &str) -> Result<&'a str> {
    as_str(get(map, key).ok_or_else(|| missing(key))?, key)
}

fn parse_str_seq(value: &Value, at: &str) -> Result<Vec<String>> {
    match value {
        Value::Seq(items) => items
            .iter()
            .map(|item| Ok(as_str(item, at)?.to_string()))
            .collect(),
        _ => Err(Error::Inventory(format!(
            "{at} must be an array of strings"
        ))),
    }
}

fn parse_digest_map(value: &Value, at: &str) -> Result<BTreeMap<Digest, Vec<String>>> {
    let mut out = BTreeMap::new();
    for (key, paths) in as_map(value, at)? {
        let digest = Digest::parse(as_str(key, &format!("{at}.<key>"))?)?;
        let paths = parse_str_seq(paths, &format!("{at}.{digest}"))?;
        for path in &paths {
            check_path(path)?;
        }
        out.insert(digest, paths);
    }
    Ok(out)
}

fn parse_version(value: &Value, name: &str) -> Result<Version> {
    let fields = as_map(value, &format!("versions.{name}"))?;
    let created = required_str(fields, "created")
        .map_err(|_| {
            Error::Inventory(format!(
                "versions.{name}.created is missing or not a string"
            ))
        })?
        .to_string();
    let message = match get(fields, "message") {
        Some(v) => Some(as_str(v, &format!("versions.{name}.message"))?.to_string()),
        None => None,
    };
    let user = match get(fields, "user") {
        Some(v) => {
            let entries = as_map(v, &format!("versions.{name}.user"))?;
            User {
                name: match get(entries, "name") {
                    Some(v) => Some(as_str(v, &format!("versions.{name}.user.name"))?.to_string()),
                    None => None,
                },
                address: match get(entries, "address") {
                    Some(v) => {
                        Some(as_str(v, &format!("versions.{name}.user.address"))?.to_string())
                    }
                    None => None,
                },
            }
        }
        None => User::default(),
    };
    let state = parse_digest_map(
        get(fields, "state").ok_or_else(|| missing(&format!("versions.{name}.state")))?,
        &format!("versions.{name}.state"),
    )?;
    Ok(Version {
        created,
        message,
        user,
        state,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest(hex: &str) -> Digest {
        Digest::parse(hex).unwrap()
    }

    fn sample() -> Inventory {
        let mut inv = Inventory::new("urn:example:1", DigestAlgorithm::Sha256);
        inv.manifest
            .insert(digest("aaaa"), vec!["v1/content/letter.md".to_string()]);
        inv.versions.insert(
            VersionNum::FIRST,
            Version {
                created: "2026-07-27T09:14:00Z".to_string(),
                message: Some("first".to_string()),
                user: User {
                    name: Some("Adam".to_string()),
                    address: Some("mailto:adam@example.org".to_string()),
                },
                state: BTreeMap::from([(digest("aaaa"), vec!["letter.md".to_string()])]),
            },
        );
        inv
    }

    #[test]
    fn version_numbers_order_numerically_not_lexically() {
        // The bug every version-directory implementation writes once: v10 sorts
        // before v9 as text.
        let mut nums = [VersionNum::new(10), VersionNum::new(9), VersionNum::new(1)];
        nums.sort();
        assert_eq!(
            nums.iter().map(|v| v.to_string()).collect::<Vec<_>>(),
            ["v1", "v9", "v10"]
        );
    }

    #[test]
    fn padded_version_names_round_trip_and_keep_their_width() {
        // The spec forbids mixing padded and unpadded names within one object,
        // so the width must survive a read.
        let v = VersionNum::parse("v0001").unwrap();
        assert_eq!(v.number(), 1);
        assert_eq!(v.to_string(), "v0001");
        assert_eq!(v.next().unwrap().to_string(), "v0002");
        assert_eq!(VersionNum::parse("v1").unwrap().to_string(), "v1");
        assert_eq!(VersionNum::parse("v11").unwrap().to_string(), "v11");
    }

    #[test]
    fn a_padded_version_exhausting_its_width_is_refused_rather_than_widened() {
        // An object numbering v01, v02, … cannot reach 100 without widening every
        // name, and the spec forbids changing the width mid-object. Refusing
        // beats writing an object no validator will accept.
        let mut v = VersionNum::parse("v01").unwrap();
        while let Ok(next) = v.next() {
            v = next;
        }
        assert_eq!(v.to_string(), "v99", "the last name this width can hold");
        // Room still inside the width is fine: v0999 -> v1000 is 5 characters either way.
        assert_eq!(
            VersionNum::parse("v0999")
                .unwrap()
                .next()
                .unwrap()
                .to_string(),
            "v1000"
        );
        // The unpadded style has no ceiling at all.
        assert_eq!(VersionNum::new(999).next().unwrap().to_string(), "v1000");
    }

    #[test]
    fn version_identity_ignores_padding_so_ord_and_eq_agree() {
        // `Ord` compares numerically; if `==` also weighed padding, `cmp` could
        // report Equal for values `==` calls different — a broken contract, and a
        // silently misbehaving map key.
        let padded = VersionNum::parse("v0007").unwrap();
        let plain = VersionNum::new(7);
        assert_eq!(padded, plain);
        assert_eq!(padded.cmp(&plain), std::cmp::Ordering::Equal);
        let mut map = BTreeMap::new();
        map.insert(padded, "written as v0007");
        assert!(map.contains_key(&plain), "the same version either way");
    }

    #[test]
    fn version_zero_and_junk_names_are_refused() {
        for bad in ["v0", "v", "1", "version1", "vx", "v-1", ""] {
            assert!(VersionNum::parse(bad).is_err(), "{bad} should be rejected");
        }
    }

    #[test]
    fn an_inventory_round_trips_through_json() {
        let inv = sample();
        let json = inv.to_json().unwrap();
        assert_eq!(Inventory::parse(json.as_bytes()).unwrap(), inv);
    }

    #[test]
    fn the_written_json_carries_the_fields_the_spec_requires() {
        let json = sample().to_json().unwrap();
        for field in [
            "\"id\"",
            "\"type\"",
            "\"digestAlgorithm\"",
            "\"head\"",
            "\"manifest\"",
            "\"versions\"",
            "\"state\"",
            "\"created\"",
        ] {
            assert!(json.contains(field), "missing {field} in:\n{json}");
        }
        // The agent survives the round trip — this is the field that makes a
        // version history a provenance record.
        assert!(json.contains("mailto:adam@example.org"), "{json}");
    }

    #[test]
    fn an_inventory_whose_head_names_no_version_is_refused() {
        let mut inv = sample();
        inv.head = VersionNum::new(7);
        let json = inv.to_json().unwrap();
        let err = Inventory::parse(json.as_bytes()).unwrap_err();
        assert!(err.to_string().contains("head"), "{err}");
    }

    #[test]
    fn a_missing_required_field_names_itself() {
        let err = Inventory::parse(br#"{"id":"x"}"#).unwrap_err();
        assert!(err.to_string().contains("type"), "{err}");
    }

    #[test]
    fn an_unaddressable_digest_algorithm_is_refused() {
        // md5 is legal in `fixity`, never for content addressing.
        let json = br#"{"id":"x","type":"t","digestAlgorithm":"md5","head":"v1",
                        "manifest":{},"versions":{"v1":{"created":"now","state":{}}}}"#;
        let err = Inventory::parse(json).unwrap_err();
        assert!(err.to_string().contains("digestAlgorithm"), "{err}");
    }

    #[test]
    fn paths_that_could_escape_the_object_root_are_refused() {
        // A path in an archive is untrusted input; these are the forms that let
        // a naive join walk out of the object.
        for bad in ["../escape", "a/../b", "/absolute", "trailing/", "a//b", "."] {
            assert!(check_path(bad).is_err(), "{bad} should be rejected");
        }
        for good in ["a.txt", "dir/a.txt", "a/b/c.md"] {
            assert!(check_path(good).is_ok(), "{good} should be accepted");
        }
    }

    #[test]
    fn an_illegal_path_inside_a_parsed_inventory_is_caught() {
        let json = br#"{"id":"x","type":"t","digestAlgorithm":"sha256","head":"v1",
                        "manifest":{"aa":["../outside"]},
                        "versions":{"v1":{"created":"now","state":{}}}}"#;
        assert!(Inventory::parse(json).is_err());
    }

    #[test]
    fn fixity_from_a_foreign_object_survives_a_rewrite() {
        // This crate never computes these digests, but losing them on rewrite
        // would silently discard someone else's preservation metadata.
        let json = br#"{"id":"x","type":"t","digestAlgorithm":"sha256","head":"v1",
                        "manifest":{},"versions":{"v1":{"created":"now","state":{}}},
                        "fixity":{"md5":{"abc123":["v1/content/a.txt"]}}}"#;
        let inv = Inventory::parse(json).unwrap();
        let back = Inventory::parse(inv.to_json().unwrap().as_bytes()).unwrap();
        assert_eq!(back.fixity["md5"]["abc123"], ["v1/content/a.txt"]);
    }

    #[test]
    fn files_inverts_state_into_what_a_person_reads() {
        let inv = sample();
        let files = inv.head_version().unwrap().files();
        assert_eq!(files["letter.md"], &digest("aaaa"));
    }
}
