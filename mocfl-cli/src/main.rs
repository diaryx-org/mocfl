//! `mocfl` — a command-line companion for the [`mocfl`] library.
//!
//! The library is the product; this is the way to *see* it. Every command here
//! is a thin call into the crate, with one deliberate exception: the clock. The
//! library takes every timestamp from its caller so it stays deterministic, and
//! a command-line run is the caller that actually reads the wall clock (see
//! [`time`]).
//!
//! Output is plain and greppable — tab-separated where it is data, prose only
//! where it is a summary. Errors go to stderr prefixed `mocfl:`; `validate` is
//! the only command that exits nonzero on a *finding* rather than an error.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use mocfl::{DigestAlgorithm, Object, StdFs, VersionMeta, VersionNum, validate, validate_deep};

mod time;
mod tree;

type Fallible = Result<(), Box<dyn std::error::Error>>;

#[derive(Parser)]
#[command(
    name = "mocfl",
    version,
    about = "Inspect, version, and validate an OCFL object.",
    long_about = "A minimal OCFL object tool.\n\n\
                  An OCFL object is a directory that says what it is, carries a \
                  readable JSON inventory of every version it has ever had, and \
                  stores content addressed by digest. Nothing here is needed to \
                  read one — that is the point of the layout — but it is a great \
                  deal faster than reading inventory.json by hand."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create an empty object.
    ///
    /// An OCFL object must have at least one version, so this is not an empty
    /// *directory* — it is a real `v1` whose state happens to be empty, which the
    /// spec allows and its own fixtures include. The object is valid and
    /// checksummed the moment this returns; a later `commit` adds `v2`.
    ///
    /// Not required: `commit --id` creates an object too. This exists for the
    /// familiar create-then-fill order, and for recording the moment an archive
    /// was established as its own event.
    Init {
        /// The object directory to create.
        object: PathBuf,
        /// The object's identifier — permanent and external: a URI, an ARK,
        /// anything opaque and stable.
        #[arg(long)]
        id: String,
        /// A note describing this version.
        #[arg(short, long)]
        message: Option<String>,
        /// The name of the agent responsible.
        #[arg(long)]
        user: Option<String>,
        /// A URI for that agent, e.g. `mailto:someone@example.org`.
        #[arg(long)]
        email: Option<String>,
        /// Timestamp to record (RFC 3339). Defaults to now.
        #[arg(long)]
        at: Option<String>,
    },

    /// Record a directory's current contents as a new version.
    ///
    /// The directory is captured whole: a file it no longer contains is deleted
    /// as of this version, which is what makes a version's state complete rather
    /// than a delta. Bytes already stored under an earlier version are never
    /// written again, so an unchanged photo costs nothing and a rename moves
    /// nothing.
    Commit {
        /// The object directory. Created on first commit.
        object: PathBuf,
        /// The working directory to capture.
        #[arg(long)]
        from: PathBuf,
        /// The object's identifier. Required only when creating it.
        #[arg(long)]
        id: Option<String>,
        /// A note describing this version.
        #[arg(short, long)]
        message: Option<String>,
        /// The name of the agent responsible.
        #[arg(long)]
        user: Option<String>,
        /// A URI for that agent, e.g. `mailto:someone@example.org`.
        #[arg(long)]
        email: Option<String>,
        /// Timestamp to record (RFC 3339). Defaults to now.
        #[arg(long)]
        at: Option<String>,
        /// Capture `.git`, `.DS_Store` and friends too.
        #[arg(long)]
        include_all: bool,
        /// Record a version that removes every file, when the directory is empty.
        #[arg(long)]
        allow_empty: bool,
    },

    /// Show an object's versions, or one file's history across them.
    ///
    /// With `--path`, this is the query the whole layout makes cheap: a
    /// rename-robust lineage of one file, answered from the inventory alone, with
    /// no content read and no similarity guessing.
    Log {
        /// The object directory.
        object: PathBuf,
        /// Show the history of one logical path instead of the version list.
        #[arg(long)]
        path: Option<String>,
    },

    /// List the files present at a version.
    Ls {
        /// The object directory.
        object: PathBuf,
        /// Which version (default: the head).
        #[arg(long)]
        version: Option<String>,
        /// Also show each file's digest.
        #[arg(short, long)]
        long: bool,
    },

    /// Write one file's contents at a version to stdout.
    Cat {
        /// The object directory.
        object: PathBuf,
        /// The logical path to read.
        path: String,
        /// Which version (default: the head).
        #[arg(long)]
        version: Option<String>,
    },

    /// Summarize the object: identity, versions, and what deduplication saved.
    Show {
        /// The object directory.
        object: PathBuf,
    },

    /// Compare two versions.
    Diff {
        /// The object directory.
        object: PathBuf,
        /// The older version (default: the one before `--to`).
        #[arg(long)]
        from: Option<String>,
        /// The newer version (default: the head).
        #[arg(long)]
        to: Option<String>,
    },

    /// Compare a working directory against the head version.
    ///
    /// Answers "what would a commit record right now?" without recording it.
    Status {
        /// The object directory.
        object: PathBuf,
        /// The working directory to compare.
        #[arg(long)]
        from: PathBuf,
        /// Compare `.git`, `.DS_Store` and friends too.
        #[arg(long)]
        include_all: bool,
    },

    /// Write a version's files back out to a directory.
    Restore {
        /// The object directory.
        object: PathBuf,
        /// Where to write. Must be empty or absent unless `--force`.
        #[arg(long)]
        to: PathBuf,
        /// Which version (default: the head).
        #[arg(long)]
        version: Option<String>,
        /// Write into a non-empty directory.
        #[arg(long)]
        force: bool,
    },

    /// Check the object for conformance. Exits 1 if anything is found.
    Validate {
        /// The object directory.
        object: PathBuf,
        /// Also rehash every content file — the bit-rot pass.
        #[arg(long)]
        deep: bool,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("mocfl: {err}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Fallible {
    match cli.command {
        Command::Init {
            object,
            id,
            message,
            user,
            email,
            at,
        } => init(object, id, message, user, email, at),
        Command::Commit {
            object,
            from,
            id,
            message,
            user,
            email,
            at,
            include_all,
            allow_empty,
        } => commit(
            object,
            from,
            id,
            message,
            user,
            email,
            at,
            include_all,
            allow_empty,
        ),
        Command::Log { object, path } => log(object, path),
        Command::Ls {
            object,
            version,
            long,
        } => ls(object, version, long),
        Command::Cat {
            object,
            path,
            version,
        } => cat(object, path, version),
        Command::Show { object } => show(object),
        Command::Diff { object, from, to } => diff(object, from, to),
        Command::Status {
            object,
            from,
            include_all,
        } => status(object, from, include_all),
        Command::Restore {
            object,
            to,
            version,
            force,
        } => restore(object, to, version, force),
        Command::Validate { object, deep } => check(object, deep),
    }
}

// ---- helpers ----

/// Open an existing object, or prepare a new one when the directory holds none.
fn open_or_create(
    path: &Path,
    id: Option<String>,
) -> Result<Object<StdFs>, Box<dyn std::error::Error>> {
    let declared = path.is_dir()
        && std::fs::read_dir(path)?.any(|e| {
            e.map(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("0=ocfl_object_")
            })
            .unwrap_or(false)
        });

    if declared {
        if id.is_some() {
            eprintln!("mocfl: --id ignored; the object already has one");
        }
        return Ok(Object::open(StdFs, path)?);
    }

    let id = id.ok_or(
        "no object here yet — pass --id <identifier> to create one. \
         The id is permanent and external: a URI, an ARK, anything opaque and stable.",
    )?;
    Ok(Object::create(StdFs, path, id, DigestAlgorithm::Sha256))
}

/// Resolve a `--version` flag, defaulting to the head.
fn resolve(
    object: &Object<StdFs>,
    version: Option<String>,
) -> Result<VersionNum, Box<dyn std::error::Error>> {
    match version {
        Some(name) => Ok(VersionNum::parse(&name)?),
        None => object
            .head()
            .ok_or_else(|| "the object has no versions yet".into()),
    }
}

/// A version's `logical path -> digest` map, in the shape [`tree::diff`] wants.
fn state_of(
    object: &Object<StdFs>,
    version: VersionNum,
) -> Result<BTreeMap<String, String>, Box<dyn std::error::Error>> {
    Ok(object
        .files_at(version)?
        .into_iter()
        .map(|(path, digest)| (path.to_string(), digest.to_string()))
        .collect())
}

fn excludes(include_all: bool) -> Vec<String> {
    if include_all {
        Vec::new()
    } else {
        tree::DEFAULT_EXCLUDES
            .iter()
            .map(|s| s.to_string())
            .collect()
    }
}

/// Assemble the version metadata every writing command records.
fn meta(
    message: Option<String>,
    user: Option<String>,
    email: Option<String>,
    at: Option<String>,
) -> Result<VersionMeta, Box<dyn std::error::Error>> {
    // The clock lives here, not in the library — see `time`.
    let mut meta = VersionMeta::at(at.unwrap_or_else(time::now));
    meta.message = message;
    match (user, email) {
        (Some(name), address) => Ok(meta.user(name, address)),
        (None, None) => Ok(meta),
        (None, Some(_)) => Err("--email needs --user; an address with no name names nobody".into()),
    }
}

fn report(changes: &tree::Changes) {
    for (from, to) in &changes.renamed {
        println!("R\t{from} -> {to}");
    }
    for path in &changes.added {
        println!("A\t{path}");
    }
    for path in &changes.modified {
        println!("M\t{path}");
    }
    for path in &changes.removed {
        println!("D\t{path}");
    }
}

// ---- commands ----

fn init(
    object_path: PathBuf,
    id: String,
    message: Option<String>,
    user: Option<String>,
    email: Option<String>,
    at: Option<String>,
) -> Fallible {
    if object_path.is_dir()
        && std::fs::read_dir(&object_path)?.any(|e| {
            e.map(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("0=ocfl_object_")
            })
            .unwrap_or(false)
        })
    {
        return Err(format!("{} is already an OCFL object", object_path.display()).into());
    }

    let mut object = Object::create(StdFs, &object_path, &id, DigestAlgorithm::Sha256);
    // An empty `v1` — valid OCFL, and what the spec's own `minimal_no_content`
    // fixture looks like. No `content/` directory is written at all.
    let version = object.commit(
        std::iter::empty(),
        meta(
            message.or_else(|| Some("object created".to_string())),
            user,
            email,
            at,
        )?,
    )?;
    println!("{version}\t{id}\t{}", object_path.display());
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn commit(
    object_path: PathBuf,
    from: PathBuf,
    id: Option<String>,
    message: Option<String>,
    user: Option<String>,
    email: Option<String>,
    at: Option<String>,
    include_all: bool,
    allow_empty: bool,
) -> Fallible {
    if !from.is_dir() {
        return Err(format!("{} is not a directory", from.display()).into());
    }

    let mut object = open_or_create(&object_path, id)?;
    let before = match object.head() {
        Some(head) => state_of(&object, head)?,
        None => BTreeMap::new(),
    };

    let entries = tree::walk(&from, &excludes(include_all))?;
    // An empty version is legal OCFL, so the guard is not about emptiness — it is
    // about *destruction*. Committing an empty directory over an object that
    // holds files removes every one of them, which is a real thing to want and
    // also exactly what a mistyped `--from` looks like.
    if entries.is_empty() && !before.is_empty() && !allow_empty {
        return Err(format!(
            "{} is empty; committing it would remove all {} file(s) from the object. \
             Pass --allow-empty if that is what you mean.",
            from.display(),
            before.len()
        )
        .into());
    }

    let meta = meta(message, user, email, at)?;

    // Read lazily, one file at a time, so a large tree is never all in memory.
    let mut failed = None;
    let files = entries
        .iter()
        .map_while(|entry| match std::fs::read(&entry.source) {
            Ok(bytes) => Some((entry.logical.clone(), bytes)),
            Err(e) => {
                failed = Some(format!("{}: {e}", entry.source.display()));
                None
            }
        });
    let version = object.commit(files, meta)?;
    if let Some(detail) = failed {
        return Err(format!("could not read {detail}").into());
    }

    let changes = tree::diff(&before, &state_of(&object, version)?);
    report(&changes);
    println!(
        "{version}\t{} file(s), {} change(s)",
        object.files_at(version)?.len(),
        changes.total()
    );
    Ok(())
}

fn log(object_path: PathBuf, path: Option<String>) -> Fallible {
    let object = Object::open(StdFs, &object_path)?;

    if let Some(logical) = path {
        let history = object.history(&logical);
        if history.is_empty() {
            return Err(format!("{logical} appears in no version of this object").into());
        }
        // Oldest first reads as a life story; that is what this view is for. The
        // path column is the name the file had *then*, so a rename is visible as
        // the name changing under a digest that did not.
        for entry in history {
            let created = &object.version(entry.version)?.created;
            println!(
                "{}\t{created}\t{}\t{}",
                entry.version, entry.digest, entry.logical_path
            );
        }
        return Ok(());
    }

    let mut previous: Option<VersionNum> = None;
    let mut rows = Vec::new();
    for version in object.versions() {
        let record = object.version(version)?;
        let changed = match previous {
            Some(prev) => {
                tree::diff(&state_of(&object, prev)?, &state_of(&object, version)?).total()
            }
            None => record.files().len(),
        };
        let who = record.user.name.clone().unwrap_or_else(|| "-".into());
        let what = record.message.clone().unwrap_or_else(|| "-".into());
        rows.push(format!(
            "{version}\t{}\t{who}\t{changed} change(s)\t{what}",
            record.created
        ));
        previous = Some(version);
    }
    // Newest first: a log is read from the top.
    for row in rows.into_iter().rev() {
        println!("{row}");
    }
    Ok(())
}

fn ls(object_path: PathBuf, version: Option<String>, long: bool) -> Fallible {
    let object = Object::open(StdFs, &object_path)?;
    let version = resolve(&object, version)?;
    for (path, digest) in object.files_at(version)? {
        if long {
            println!("{digest}\t{path}");
        } else {
            println!("{path}");
        }
    }
    Ok(())
}

fn cat(object_path: PathBuf, path: String, version: Option<String>) -> Fallible {
    use std::io::Write;
    let object = Object::open(StdFs, &object_path)?;
    let version = resolve(&object, version)?;
    // Straight to the raw handle: the file may well be a PNG, and `println!`
    // would mangle it.
    std::io::stdout().write_all(&object.read(version, &path)?)?;
    Ok(())
}

fn show(object_path: PathBuf) -> Fallible {
    let object = Object::open(StdFs, &object_path)?;
    let inventory = object.inventory();
    let head = object.head().ok_or("the object has no versions yet")?;

    println!("id\t{}", object.id());
    println!("spec\t{}", object.spec_version());
    println!("digest\t{}", inventory.digest_algorithm);
    println!("versions\t{}", inventory.versions.len());
    println!("head\t{head}");
    println!("files at head\t{}", object.files_at(head)?.len());

    // The number that makes the model click: bytes stored on disk versus bytes
    // the versions logically contain. The gap is everything deduplication saved.
    let mut stored = 0u64;
    let mut blob_size: BTreeMap<String, u64> = BTreeMap::new();
    for (digest, paths) in &inventory.manifest {
        for content_path in paths {
            let mut full = object.root().to_path_buf();
            for segment in content_path.split('/') {
                full.push(segment);
            }
            if let Ok(meta) = std::fs::metadata(&full) {
                stored += meta.len();
                blob_size.insert(digest.to_string(), meta.len());
            }
        }
    }
    let logical: u64 = inventory
        .versions
        .values()
        .flat_map(|v| v.files().into_values())
        .filter_map(|d| blob_size.get(d.as_str()).copied())
        .sum();

    println!("unique blobs\t{}", inventory.manifest.len());
    println!("stored bytes\t{stored}");
    println!("logical bytes\t{logical}");
    if stored > 0 {
        println!("dedup factor\t{:.2}x", logical as f64 / stored as f64);
    }
    Ok(())
}

fn diff(object_path: PathBuf, from: Option<String>, to: Option<String>) -> Fallible {
    let object = Object::open(StdFs, &object_path)?;
    let to_version = resolve(&object, to)?;
    let from_version = match from {
        Some(name) => VersionNum::parse(&name)?,
        None => {
            let previous: Vec<VersionNum> = object
                .versions()
                .into_iter()
                .filter(|v| *v < to_version)
                .collect();
            match previous.last() {
                Some(v) => *v,
                // v1 has nothing before it; diffing against the empty state is
                // the honest answer, and shows the whole object as added.
                None => {
                    let changes = tree::diff(&BTreeMap::new(), &state_of(&object, to_version)?);
                    report(&changes);
                    return Ok(());
                }
            }
        }
    };

    let changes = tree::diff(
        &state_of(&object, from_version)?,
        &state_of(&object, to_version)?,
    );
    report(&changes);
    if changes.is_empty() {
        println!("{from_version} and {to_version} hold identical state");
    }
    Ok(())
}

fn status(object_path: PathBuf, from: PathBuf, include_all: bool) -> Fallible {
    let object = Object::open(StdFs, &object_path)?;
    let head = object.head().ok_or("the object has no versions yet")?;

    let algorithm = object.inventory().digest_algorithm;
    let mut working = BTreeMap::new();
    for entry in tree::walk(&from, &excludes(include_all))? {
        let bytes = std::fs::read(&entry.source)?;
        working.insert(entry.logical, algorithm.digest(&bytes)?.to_string());
    }

    let changes = tree::diff(&state_of(&object, head)?, &working);
    report(&changes);
    if changes.is_empty() {
        println!("clean — {} would record nothing new", from.display());
    }
    Ok(())
}

fn restore(object_path: PathBuf, to: PathBuf, version: Option<String>, force: bool) -> Fallible {
    let object = Object::open(StdFs, &object_path)?;
    let version = resolve(&object, version)?;

    if to.is_dir() && std::fs::read_dir(&to)?.next().is_some() && !force {
        return Err(format!(
            "{} is not empty; pass --force to write into it anyway",
            to.display()
        )
        .into());
    }
    std::fs::create_dir_all(&to)?;

    let paths: Vec<String> = object
        .files_at(version)?
        .into_keys()
        .map(str::to_string)
        .collect();
    for path in &paths {
        let mut full = to.clone();
        for segment in path.split('/') {
            full.push(segment);
        }
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&full, object.read(version, path)?)?;
    }
    println!("{version}\t{} file(s) -> {}", paths.len(), to.display());
    Ok(())
}

fn check(object_path: PathBuf, deep: bool) -> Fallible {
    let object = Object::open(StdFs, &object_path)?;
    let found = if deep {
        validate_deep(&object)?
    } else {
        validate(&object)?
    };

    if found.is_empty() {
        println!(
            "valid\t{}\t{} version(s){}",
            object.id(),
            object.versions().len(),
            if deep { ", content verified" } else { "" }
        );
        return Ok(());
    }

    for violation in &found {
        println!("{violation}");
    }
    // A finding is a report about the archive, not a failure of this program —
    // so it is stdout plus a nonzero exit, the shape a script can branch on.
    Err(format!("{} finding(s)", found.len()).into())
}
