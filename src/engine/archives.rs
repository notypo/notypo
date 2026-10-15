//! Archive arguments and bounded member listings. Only fixed listing
//! operations run; no extraction options, passwords, or file lists are
//! forwarded. Member names stay separate from paths on the host filesystem.

use super::native::{self, Flavor};
use super::probe::{self, Budget, Capture, Probe};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tool {
    Tar,
    Zip,
    Unzip,
    Zipinfo,
    SevenZip,
    Ar,
    Jar,
    Pax,
    Cpio,
    Ditto,
    /// unar archive [member ...]; lsar lists the same way.
    Unar,
    Lsar,
    Ouch,
    Atool(AtoolMode),
    Unsquashfs,
    Mksquashfs,
    /// PowerShell's Expand-Archive (true) and Compress-Archive (false).
    PowerShellArchive(bool),
    Inputs(Family),
}

/// Tools whose operands are all input files on the host: single-file
/// compressors (except lz4's and pixz's second operand, which names the
/// output) and extractors that take whole archives (cabextract, dtrx).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Family {
    Gzip,
    Bzip2,
    Xz,
    Zstd,
    Lz4,
    /// lz4cat always takes several inputs.
    Lz4Cat,
    Brotli,
    Compress,
    Lzip,
    Lzop,
    Pixz,
    Cabextract,
    Dtrx,
}

/// atool's operation, from its name (aunpack, als, ...) or an option.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AtoolMode {
    Unknown,
    List,
    Extract,
    Cat,
    Add,
    Diff,
    Repack,
}

pub(crate) fn tool(program: &str) -> Option<Tool> {
    // PowerShell resolves cmdlets without regard to case, also when
    // qualified by their module (`Microsoft.PowerShell.Archive\Expand-Archive`).
    let cmdlet = program.rsplit(['\\', '/']).next()?;
    if cmdlet.eq_ignore_ascii_case("Expand-Archive") {
        return Some(Tool::PowerShellArchive(true));
    }
    if cmdlet.eq_ignore_ascii_case("Compress-Archive") {
        return Some(Tool::PowerShellArchive(false));
    }
    Some(match Path::new(program).file_name()?.to_str()? {
        "tar" | "gtar" | "bsdtar" => Tool::Tar,
        "zip" => Tool::Zip,
        "unzip" => Tool::Unzip,
        "zipinfo" => Tool::Zipinfo,
        "7z" | "7za" | "7zr" | "7zz" => Tool::SevenZip,
        "ar" => Tool::Ar,
        "jar" => Tool::Jar,
        "pax" => Tool::Pax,
        "cpio" | "bsdcpio" => Tool::Cpio,
        "ditto" => Tool::Ditto,
        "unar" => Tool::Unar,
        "lsar" => Tool::Lsar,
        "ouch" => Tool::Ouch,
        "atool" => Tool::Atool(AtoolMode::Unknown),
        "aunpack" => Tool::Atool(AtoolMode::Extract),
        "als" => Tool::Atool(AtoolMode::List),
        "acat" => Tool::Atool(AtoolMode::Cat),
        "apack" => Tool::Atool(AtoolMode::Add),
        "adiff" => Tool::Atool(AtoolMode::Diff),
        "arepack" => Tool::Atool(AtoolMode::Repack),
        "unsquashfs" => Tool::Unsquashfs,
        "mksquashfs" => Tool::Mksquashfs,
        "gzip" | "gunzip" | "zcat" | "gzcat" | "pigz" | "unpigz" => Tool::Inputs(Family::Gzip),
        "bzip2" | "bunzip2" | "bzcat" | "pbzip2" => Tool::Inputs(Family::Bzip2),
        "xz" | "unxz" | "xzcat" | "lzma" | "unlzma" | "lzcat" => Tool::Inputs(Family::Xz),
        "zstd" | "unzstd" | "zstdcat" | "zstdmt" => Tool::Inputs(Family::Zstd),
        "lz4" | "unlz4" => Tool::Inputs(Family::Lz4),
        "lz4cat" => Tool::Inputs(Family::Lz4Cat),
        "brotli" => Tool::Inputs(Family::Brotli),
        "compress" | "uncompress" => Tool::Inputs(Family::Compress),
        "lzip" | "plzip" | "lunzip" | "clzip" | "pdlzip" => Tool::Inputs(Family::Lzip),
        "lzop" => Tool::Inputs(Family::Lzop),
        "pixz" => Tool::Inputs(Family::Pixz),
        "cabextract" => Tool::Inputs(Family::Cabextract),
        "dtrx" => Tool::Inputs(Family::Dtrx),
        _ => return None,
    })
}

/// Tools without archive members: when their syntax is declined, their
/// operands are still ordinary host paths for the generic path provider.
pub(crate) fn host_paths_only(tool: Tool) -> bool {
    matches!(
        tool,
        Tool::Inputs(_)
            | Tool::Zip
            | Tool::Ditto
            | Tool::Ouch
            | Tool::Atool(_)
            | Tool::Mksquashfs
            | Tool::PowerShellArchive(_)
    )
}

pub(crate) fn old_style_flags(word: &str) -> bool {
    !word.is_empty()
        && !word.starts_with('-')
        && word
            .chars()
            .all(|c| "ctxruAfCTXbILvzjJaphHkPmOSwWqynolU".contains(c))
        && word.chars().filter(|&c| "ctxruA".contains(c)).count() == 1
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct PathArgument {
    pub index: usize,
    /// Attached option text, left intact when its path changes.
    pub prefix: String,
    pub directory: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Layout {
    pub archive: PathArgument,
    pub members: Vec<usize>,
    pub paths: Vec<PathArgument>,
    pub reads_members: bool,
}

/// Declines opaque words, unknown option arities, stdin archives, and
/// ambiguous modes. Indices refer to arguments after the effective program.
pub(crate) fn layout(tool: Tool, args: &[Option<&str>]) -> Option<Layout> {
    let args: Vec<&str> = args.iter().copied().collect::<Option<_>>()?;
    let layout = match tool {
        Tool::Tar => tar_layout(&args),
        Tool::Zip => zip_layout(&args),
        Tool::Unzip => unzip_layout(&args),
        Tool::Zipinfo => zipinfo_layout(&args),
        Tool::SevenZip => seven_zip_layout(&args),
        Tool::Ar => ar_layout(&args),
        Tool::Jar => jar_layout(&args),
        Tool::Pax => pax_layout(&args),
        Tool::Cpio => cpio_layout(&args),
        Tool::Ditto => ditto_layout(&args),
        Tool::Unar | Tool::Lsar => unar_layout(&args),
        Tool::Ouch => ouch_layout(&args),
        Tool::Atool(mode) => atool_layout(mode, &args),
        Tool::Unsquashfs => unsquashfs_layout(&args),
        Tool::Mksquashfs => mksquashfs_layout(&args),
        Tool::PowerShellArchive(expand) => powershell_layout(expand, &args),
        Tool::Inputs(family) => inputs_layout(family, &args),
    }?;
    let archive = args[layout.archive.index].strip_prefix(&layout.archive.prefix)?;
    (!archive.is_empty()
        && archive != "-"
        && !archive.contains("://")
        && !archive.chars().any(char::is_control))
    .then_some(layout)
}

fn path(index: usize, prefix: &str, directory: bool) -> PathArgument {
    PathArgument {
        index,
        prefix: prefix.into(),
        directory,
    }
}

fn tar_layout(args: &[&str]) -> Option<Layout> {
    let mut archive = None;
    let mut paths = Vec::new();
    let mut members = Vec::new();
    let mut mode = None;
    let mut file_list = false;
    let mut i = 0;
    let mut options = true;
    while i < args.len() {
        let arg = args[i];
        if options && arg == "--" {
            options = false;
        } else if options && arg.starts_with("--") {
            let (name, attached) = arg
                .split_once('=')
                .map_or((arg, None), |(a, b)| (a, Some(b)));
            let operation = match name {
                "--extract" | "--get" => Some('x'),
                "--list" => Some('t'),
                "--create" => Some('c'),
                "--append" => Some('r'),
                "--update" => Some('u'),
                "--concatenate" | "--catenate" => Some('A'),
                _ => None,
            };
            if let Some(operation) = operation {
                if mode.replace(operation).is_some() || attached.is_some() {
                    return None;
                }
            } else if [
                "--file",
                "--directory",
                "--files-from",
                "--exclude-from",
                "--exclude",
                "--strip-components",
                "--format",
                "--use-compress-program",
                "--blocking-factor",
                "--checkpoint",
                "--checkpoint-action",
                "--newer",
                "--newer-mtime",
                "--owner",
                "--group",
                "--mtime",
                "--transform",
                "--options",
                "--passphrase",
            ]
            .contains(&name)
            {
                let (at, prefix) = if let Some(value) = attached {
                    if value.is_empty() {
                        return None;
                    }
                    (i, format!("{name}="))
                } else {
                    i += 1;
                    args.get(i)?;
                    (i, String::new())
                };
                match name {
                    "--file" => {
                        if archive.replace(path(at, &prefix, false)).is_some() {
                            return None;
                        }
                    }
                    "--directory" => paths.push(path(at, &prefix, true)),
                    "--files-from" | "--exclude-from" | "--exclude" | "--transform" => {
                        file_list = true
                    }
                    _ => {}
                }
            } else if attached.is_some()
                || ![
                    "--verbose",
                    "--gzip",
                    "--gunzip",
                    "--bzip2",
                    "--xz",
                    "--zstd",
                    "--lzma",
                    "--auto-compress",
                    "--overwrite",
                    "--unlink-first",
                    "--recursive-unlink",
                    "--keep-old-files",
                    "--skip-old-files",
                    "--preserve-permissions",
                    "--same-owner",
                    "--no-same-owner",
                    "--numeric-owner",
                    "--absolute-names",
                    "--null",
                    "--no-recursion",
                    "--wildcards",
                    "--no-wildcards",
                    "--ignore-case",
                ]
                .contains(&name)
            {
                return None;
            } else if matches!(name, "--wildcards" | "--ignore-case" | "--absolute-names") {
                file_list = true;
            }
        } else if options && (arg.starts_with('-') || i == 0) {
            let old_style = !arg.starts_with('-');
            let flags = arg.strip_prefix('-').unwrap_or(arg);
            if flags.is_empty() {
                return None;
            }
            for (at, letter) in flags.char_indices() {
                if "ctxruA".contains(letter) {
                    if mode.replace(letter).is_some() {
                        return None;
                    }
                } else if "fCTXbIL".contains(letter) {
                    let rest = &flags[at + letter.len_utf8()..];
                    let (value_at, prefix) = if !old_style && !rest.is_empty() {
                        (i, arg[..arg.len() - rest.len()].to_owned())
                    } else {
                        i += 1;
                        args.get(i)?;
                        (i, String::new())
                    };
                    match letter {
                        'f' => {
                            if archive.replace(path(value_at, &prefix, false)).is_some() {
                                return None;
                            }
                        }
                        'C' => paths.push(path(value_at, &prefix, true)),
                        'T' | 'X' => file_list = true,
                        _ => {}
                    }
                    if !old_style {
                        break;
                    }
                } else if !"vzjJaphHkPmOSwWqynolU".contains(letter) {
                    return None;
                }
            }
        } else {
            members.push(i);
        }
        i += 1;
    }
    let reads_members = matches!(mode?, 'x' | 't');
    let archive = archive?;
    if reads_members {
        paths.insert(0, path(archive.index, &archive.prefix, false));
    } else {
        // These words name local inputs, not members of an existing archive.
        paths.extend(members.iter().map(|&i| path(i, "", false)));
    }
    if !reads_members || file_list {
        members.clear();
    }
    Some(Layout {
        archive,
        members,
        paths,
        reads_members,
    })
}

fn unzip_layout(args: &[&str]) -> Option<Layout> {
    let mut archive = None;
    let mut members = Vec::new();
    let mut paths = Vec::new();
    let mut excludes = false;
    let mut i = 0;
    while i < args.len() {
        let arg = args[i];
        if arg == "-x" {
            excludes = true;
        } else if arg.starts_with("-d") || arg.starts_with("-P") {
            let directory = arg.starts_with("-d");
            let (at, prefix) = if arg.len() > 2 {
                (i, &arg[..2])
            } else {
                i += 1;
                args.get(i)?;
                (i, "")
            };
            if directory {
                paths.push(path(at, prefix, true));
            }
        } else if let Some(flags) = arg.strip_prefix('-') {
            if flags.is_empty()
                || !flags
                    .chars()
                    .all(|c| "abcCDEfFhHiIjJkKlLMnNpqsStTuUvVwWXYZ0123456789-".contains(c))
            {
                return None;
            }
            // Unknown/combined value options cannot establish member positions.
            if flags.contains(['d', 'P', 'x']) || flags.starts_with('-') {
                return None;
            }
            if flags.contains('C') {
                excludes = true;
            }
        } else if archive.is_none() {
            archive = Some(path(i, "", false));
        } else if !excludes {
            members.push(i);
        }
        i += 1;
    }
    let archive = archive?;
    paths.insert(0, path(archive.index, "", false));
    Some(Layout {
        archive,
        members,
        paths,
        reads_members: true,
    })
}

fn seven_zip_layout(args: &[&str]) -> Option<Layout> {
    let mut mode = None;
    let mut archive = None;
    let mut members = Vec::new();
    let mut paths = Vec::new();
    let mut options = true;
    for (i, &arg) in args.iter().enumerate() {
        if options && arg == "--" {
            options = false;
        } else if options && arg.starts_with('-') {
            if arg.starts_with("-o") && arg.len() > 2 {
                paths.push(path(i, "-o", true));
            } else if arg == "-o"
                || ["-i", "-x", "-si", "-an"]
                    .iter()
                    .any(|prefix| arg.starts_with(prefix))
            {
                return None;
            }
            // Every other 7-Zip switch has either no value or an attached value.
        } else if arg.starts_with('@') {
            return None;
        } else if mode.is_none() {
            if !["x", "e", "l", "t", "a", "u", "d", "rn"].contains(&arg) {
                return None;
            }
            mode = Some(arg);
        } else if archive.is_none() {
            archive = Some(path(i, "", false));
        } else {
            members.push(i);
        }
    }
    let mode = mode?;
    let reads_members = matches!(mode, "x" | "e" | "l" | "t");
    let archive = archive?;
    if reads_members {
        paths.insert(0, path(archive.index, "", false));
    } else if matches!(mode, "a" | "u") {
        paths.extend(members.iter().map(|&i| path(i, "", false)));
    }
    if !reads_members {
        members.clear();
    }
    Some(Layout {
        archive,
        members,
        paths,
        reads_members,
    })
}

/// `zip [options] archive path...`: the archive is created or updated and
/// the paths are host inputs. Short options may be two letters (`-ds 10`
/// is not `-d -s`), so only whole words from this list are accepted; the
/// internal modes (-d, -U), name lists (-@), and filesync are declined.
fn zip_layout(args: &[&str]) -> Option<Layout> {
    const FLAGS: &[&str] = &[
        "-0",
        "-1",
        "-2",
        "-3",
        "-4",
        "-5",
        "-6",
        "-7",
        "-8",
        "-9",
        "-r",
        "-q",
        "-v",
        "-j",
        "-m",
        "-o",
        "-X",
        "-y",
        "-D",
        "-T",
        "-k",
        "-l",
        "-ll",
        "-u",
        "-f",
        "-g",
        "-nw",
        "-ws",
        "-e",
        "--recurse-paths",
        "--quiet",
        "--verbose",
        "--junk-paths",
        "--move",
        "--encrypt",
        "--symlinks",
        "--no-dir-entries",
        "--test",
        "--update",
        "--freshen",
        "--grow",
        "--no-wild",
        "--wild-stop-dirs",
        "--latest-time",
    ];
    const VALUED: &[&str] = &[
        "-P",
        "-b",
        "-n",
        "-t",
        "-tt",
        "-Z",
        "--password",
        "--temp-path",
        "--suffixes",
        "--from-date",
        "--before-date",
        "--compression-method",
    ];
    let mut operands = Vec::new();
    let mut options = true;
    let mut i = 0;
    while i < args.len() {
        let arg = args[i];
        if !options || !arg.starts_with('-') {
            operands.push(i);
        } else if arg == "--" {
            options = false;
        } else if matches!(arg, "-x" | "-i" | "--exclude" | "--include") {
            // Patterns follow, up to the next option.
            while args.get(i + 1).is_some_and(|next| !next.starts_with('-')) {
                i += 1;
            }
        } else if VALUED.contains(&arg) {
            i += 1;
            args.get(i)?;
        } else if !FLAGS.contains(&arg)
            && !arg
                .split_once('=')
                .is_some_and(|(name, value)| VALUED.contains(&name) && !value.is_empty())
        {
            return None;
        }
        i += 1;
    }
    let (&archive, inputs) = operands.split_first()?;
    if inputs.is_empty() || args[archive] == "-" || inputs.iter().any(|&at| args[at] == "-") {
        return None;
    }
    Some(Layout {
        archive: path(archive, "", false),
        members: Vec::new(),
        paths: inputs.iter().map(|&at| path(at, "", false)).collect(),
        reads_members: false,
    })
}

/// zipinfo [-12smlvhMtTzC] archive [member ...] [-x member ...]
fn zipinfo_layout(args: &[&str]) -> Option<Layout> {
    let mut archive = None;
    let mut members = Vec::new();
    let mut excludes = false;
    for (i, &arg) in args.iter().enumerate() {
        if arg == "-x" {
            excludes = true;
        } else if let Some(flags) = arg.strip_prefix('-') {
            // -O/-I name character sets; anything else is unknown here.
            if flags.is_empty() || !flags.chars().all(|c| "12smlvhMtTzC".contains(c)) {
                return None;
            }
        } else if archive.is_none() {
            archive = Some(path(i, "", false));
        } else if !excludes {
            members.push(i);
        }
    }
    let archive = archive?;
    Some(Layout {
        paths: vec![path(archive.index, "", false)],
        archive,
        members,
        reads_members: true,
    })
}

/// `ar [-]operation[modifiers] [relpos] [count] archive [member ...]`, with
/// BSD ar's separate `-t -v` words as well. Members name archive entries
/// for d/m/p/t/x and host files for q/r.
fn ar_layout(args: &[&str]) -> Option<Layout> {
    let mut operation = None;
    let mut modifiers = String::new();
    let mut i = 0;
    while i < args.len() {
        let arg = args[i];
        let letters = match arg.strip_prefix('-') {
            Some(letters) if !letters.starts_with('-') => letters,
            // --plugin, --target, --output, response files, and the like.
            Some(_) => return None,
            None if i == 0 => arg,
            None => break,
        };
        if letters.is_empty() {
            return None;
        }
        for letter in letters.chars() {
            if "dmpqrtx".contains(letter) {
                if operation.replace(letter).is_some() {
                    return None;
                }
            } else if "abcDfiLlNoOPsSTuUvV".contains(letter) {
                modifiers.push(letter);
            } else {
                return None;
            }
        }
        i += 1;
    }
    // `ar s archive` only rebuilds the symbol table.
    let operation = operation.or(modifiers.contains('s').then_some('s'))?;
    if modifiers.contains(['a', 'b', 'i']) && matches!(operation, 'm' | 'r' | 'q') {
        i += 1;
    }
    if modifiers.contains('N') {
        i += 1;
    }
    let archive = path(i, "", false);
    args.get(i)?;
    let rest: Vec<usize> = (i + 1..args.len()).collect();
    if rest.iter().any(|&at| args[at].starts_with('@')) {
        return None;
    }
    let reads_members = matches!(operation, 'd' | 'm' | 'p' | 't' | 'x');
    let mut paths = Vec::new();
    if reads_members {
        paths.push(path(archive.index, "", false));
    } else if matches!(operation, 'q' | 'r') {
        paths.extend(rest.iter().map(|&at| path(at, "", false)));
    }
    Some(Layout {
        archive,
        members: if reads_members { rest } else { Vec::new() },
        paths,
        reads_members,
    })
}

/// jar: `jar tf a.jar [member...]` (old style: the values of f, m, and e
/// follow in the order of their letters) or GNU style (`--list --file=a.jar`,
/// `-xvf a.jar`). `-C dir file` names a file relative to dir.
fn jar_layout(args: &[&str]) -> Option<Layout> {
    let mut mode = None;
    let mut archive = None;
    let mut operands = Vec::new();
    // Manifest files are host inputs too.
    let mut manifests = Vec::new();
    let set = |letter: char, mode: &mut Option<char>| mode.replace(letter).is_none();
    let mut i = 0;
    if let Some(letters) = args.first().filter(|first| !first.starts_with(['-', '@'])) {
        i = 1;
        for letter in letters.chars() {
            match letter {
                'c' | 't' | 'x' | 'u' => {
                    if !set(letter, &mut mode) {
                        return None;
                    }
                }
                'f' | 'm' | 'e' => {
                    args.get(i)?;
                    if letter == 'f' {
                        archive = Some(path(i, "", false));
                    } else if letter == 'm' {
                        manifests.push(path(i, "", false));
                    }
                    i += 1;
                }
                'v' | 'n' | '0' | 'P' | 'M' => {}
                _ => return None,
            }
        }
    }
    while i < args.len() {
        let arg = args[i];
        if arg.starts_with('@') {
            return None;
        } else if arg == "-C" {
            // The directory and the file that follows it.
            i += 2;
            args.get(i)?;
        } else if let Some(long) = arg.strip_prefix("--") {
            let (name, value) = long
                .split_once('=')
                .map_or((long, None), |(name, value)| (name, Some(value)));
            let letter = match name {
                "create" => Some('c'),
                "list" => Some('t'),
                "extract" => Some('x'),
                "update" => Some('u'),
                "describe-module" => Some('d'),
                _ => None,
            };
            if let Some(letter) = letter {
                if value.is_some() || !set(letter, &mut mode) {
                    return None;
                }
            } else if [
                "file",
                "main-class",
                "manifest",
                "module-version",
                "hash-modules",
                "module-path",
                "date",
                "release",
                "dir",
            ]
            .contains(&name)
            {
                let at = match value {
                    Some(value) if !value.is_empty() => {
                        path(i, &arg[..arg.len() - value.len()], false)
                    }
                    Some(_) => return None,
                    None => {
                        i += 1;
                        args.get(i)?;
                        path(i, "", false)
                    }
                };
                if name == "manifest" {
                    manifests.push(at);
                } else if name == "file" && archive.replace(at).is_some() {
                    return None;
                }
            } else if value.is_some()
                || ![
                    "verbose",
                    "no-manifest",
                    "no-compress",
                    "keep-old-files",
                    "validate",
                    "normalize",
                ]
                .contains(&name)
            {
                return None;
            }
        } else if let Some(letters) = arg.strip_prefix('-').filter(|l| !l.is_empty()) {
            for (at, letter) in letters.char_indices() {
                match letter {
                    'c' | 't' | 'x' | 'u' | 'd' => {
                        if !set(letter, &mut mode) {
                            return None;
                        }
                    }
                    // Values are separate words in GNU style.
                    'f' | 'e' | 'm' | 'p' => {
                        if at + 1 != letters.len() {
                            return None;
                        }
                        i += 1;
                        args.get(i)?;
                        if letter == 'm' {
                            manifests.push(path(i, "", false));
                        } else if letter == 'f' && archive.replace(path(i, "", false)).is_some() {
                            return None;
                        }
                    }
                    'v' | 'M' | '0' | 'k' | 'n' | 'P' => {}
                    _ => return None,
                }
            }
        } else {
            operands.push(i);
        }
        i += 1;
    }
    let archive: PathArgument = archive?;
    let (members, mut paths, reads_members) = match mode? {
        't' | 'x' => (
            operands,
            vec![path(archive.index, &archive.prefix, false)],
            true,
        ),
        'd' => (
            Vec::new(),
            vec![path(archive.index, &archive.prefix, false)],
            false,
        ),
        _ => (
            Vec::new(),
            operands.iter().map(|&at| path(at, "", false)).collect(),
            false,
        ),
    };
    paths.extend(manifests);
    Some(Layout {
        archive,
        members,
        paths,
        reads_members,
    })
}

/// pax: list (no -r/-w) and read modes take `-f archive`; write mode
/// creates it from the operands. Operands are patterns when reading.
fn pax_layout(args: &[&str]) -> Option<Layout> {
    let (mut read, mut write) = (false, false);
    let mut archive = None;
    let mut operands = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let arg = args[i];
        if arg == "--" {
            operands.extend(i + 1..args.len());
            break;
        }
        let Some(letters) = arg.strip_prefix('-').filter(|l| !l.is_empty()) else {
            operands.push(i);
            i += 1;
            continue;
        };
        for (at, letter) in letters.char_indices() {
            if "fsopEUGTBbx".contains(letter) {
                let rest = &letters[at + 1..];
                let (value, prefix) = if rest.is_empty() {
                    i += 1;
                    args.get(i)?;
                    (i, "")
                } else {
                    (i, &arg[..arg.len() - rest.len()])
                };
                if letter == 'f' && archive.replace(path(value, prefix, false)).is_some() {
                    return None;
                }
                break;
            }
            match letter {
                'r' => read = true,
                'w' => write = true,
                _ if "acdiklntuvzDHLOPXYZ0".contains(letter) => {}
                _ => return None,
            }
        }
        i += 1;
    }
    let archive: PathArgument = archive?;
    match (read, write) {
        (true, true) => None,
        (false, true) => Some(Layout {
            archive,
            members: Vec::new(),
            paths: operands.iter().map(|&at| path(at, "", false)).collect(),
            reads_members: false,
        }),
        _ => Some(Layout {
            paths: vec![path(archive.index, &archive.prefix, false)],
            archive,
            members: Vec::new(),
            reads_members: false,
        }),
    }
}

/// cpio -i/-t with `-F`/`-I archive` (or --file); copy-out and pass modes
/// read their inputs from standard input. Operands are patterns.
fn cpio_layout(args: &[&str]) -> Option<Layout> {
    let mut mode = None;
    let mut archive = None;
    let mut i = 0;
    while i < args.len() {
        let arg = args[i];
        if let Some(long) = arg.strip_prefix("--") {
            let (name, value) = long
                .split_once('=')
                .map_or((long, None), |(n, v)| (n, Some(v)));
            match (name, value) {
                ("file", Some(value)) if !value.is_empty() => {
                    if archive.replace(path(i, "--file=", false)).is_some() {
                        return None;
                    }
                }
                ("format" | "owner", Some(_)) => {}
                (
                    "make-directories"
                    | "no-preserve-owner"
                    | "preserve-modification-time"
                    | "quiet"
                    | "verbose"
                    | "insecure"
                    | "numeric-uid-gid"
                    | "unconditional"
                    | "extract"
                    | "list",
                    None,
                ) => {
                    if name == "extract" {
                        mode.get_or_insert('i');
                    } else if name == "list" {
                        mode = Some('t');
                    }
                }
                _ => return None,
            }
        } else if let Some(letters) = arg.strip_prefix('-').filter(|l| !l.is_empty()) {
            for (at, letter) in letters.char_indices() {
                if "FIOHRC".contains(letter) {
                    let rest = &letters[at + 1..];
                    let (value, prefix) = if rest.is_empty() {
                        i += 1;
                        args.get(i)?;
                        (i, "")
                    } else {
                        (i, &arg[..arg.len() - rest.len()])
                    };
                    if matches!(letter, 'F' | 'I')
                        && archive.replace(path(value, prefix, false)).is_some()
                    {
                        return None;
                    }
                    break;
                }
                match letter {
                    'i' => {
                        mode.get_or_insert('i');
                    }
                    't' => mode = Some('t'),
                    'o' | 'p' => return None,
                    // -E reads patterns from a file.
                    _ if "aBcdfjJlLmnruvVyzZ06".contains(letter) => {}
                    _ => return None,
                }
            }
        }
        i += 1;
    }
    mode?;
    let archive: PathArgument = archive?;
    // GNU cpio reads `host:file` from a remote tape server.
    if args[archive.index][archive.prefix.len()..].contains(':') {
        return None;
    }
    Some(Layout {
        paths: vec![path(archive.index, &archive.prefix, false)],
        archive,
        members: Vec::new(),
        reads_members: false,
    })
}

/// macOS ditto: `-x archive destination` reads an archive; `-c sources...
/// archive` creates one. Plain copies are left to the generic provider.
fn ditto_layout(args: &[&str]) -> Option<Layout> {
    let mut mode = None;
    let mut operands = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let arg = args[i];
        if let Some(name) = arg.strip_prefix("--") {
            if ["arch", "bom", "zlibCompressionLevel"].contains(&name) {
                i += 1;
                args.get(i)?;
            } else if ![
                "keepParent",
                "sequesterRsrc",
                "rsrc",
                "norsrc",
                "extattr",
                "noextattr",
                "qtn",
                "noqtn",
                "acl",
                "noacl",
                "nocache",
                "hfsCompression",
                "nohfsCompression",
                "preserveHFSCompression",
                "nopreserveHFSCompression",
                "clone",
                "noclone",
                "password",
                "help",
            ]
            .contains(&name)
            {
                return None;
            }
        } else if let Some(letters) = arg.strip_prefix('-').filter(|l| !l.is_empty()) {
            for letter in letters.chars() {
                match letter {
                    'x' | 'c' => {
                        if mode.replace(letter).is_some() {
                            return None;
                        }
                    }
                    _ if "hvVXzjk".contains(letter) => {}
                    _ => return None,
                }
            }
        } else {
            operands.push(i);
        }
        i += 1;
    }
    match (mode?, operands.as_slice()) {
        ('x', &[archive, _]) => Some(Layout {
            archive: path(archive, "", false),
            members: Vec::new(),
            paths: vec![path(archive, "", false)],
            reads_members: false,
        }),
        ('c', [sources @ .., archive]) if !sources.is_empty() => Some(Layout {
            archive: path(*archive, "", false),
            members: Vec::new(),
            paths: sources.iter().map(|&at| path(at, "", false)).collect(),
            reads_members: false,
        }),
        _ => None,
    }
}

/// XADMaster's options are whole words (`-o`, `-output-directory`, also
/// with two dashes) anywhere on the line. The first operand is the archive;
/// later operands name members, or indexes with -i.
fn unar_layout(args: &[&str]) -> Option<Layout> {
    let mut operands = Vec::new();
    let mut listable = true;
    let mut i = 0;
    while i < args.len() {
        let arg = args[i];
        if arg == "--" {
            operands.extend(i + 1..args.len());
            break;
        }
        let Some(name) = arg.strip_prefix("--").or_else(|| arg.strip_prefix('-')) else {
            operands.push(i);
            i += 1;
            continue;
        };
        match name {
            "o" | "output-directory" | "p" | "password" | "k" | "forks" | "e" | "encoding"
            | "E" | "password-encoding" => {
                // Another encoding changes the names a selector matches.
                if matches!(name, "e" | "encoding" | "E" | "password-encoding") {
                    listable = false;
                }
                i += 1;
                args.get(i)?;
            }
            "i" | "indexes" | "nr" | "no-recursion" => listable = false,
            "f"
            | "force-overwrite"
            | "r"
            | "force-rename"
            | "s"
            | "force-skip"
            | "d"
            | "force-directory"
            | "D"
            | "no-directory"
            | "t"
            | "copy-time"
            | "test"
            | "nq"
            | "no-quarantine"
            | "q"
            | "quiet"
            | "v"
            | "version"
            | "h"
            | "help"
            | "l"
            | "long"
            | "L"
            | "verylong"
            | "pe"
            | "print-encoding"
            | "j"
            | "json"
            | "jss"
            | "json-skip-solid-information"
            | "ja"
            | "json-ascii" => {}
            _ => return None,
        }
        i += 1;
    }
    let (&archive, members) = operands.split_first()?;
    Some(Layout {
        archive: path(archive, "", false),
        members: if listable {
            members.to_vec()
        } else {
            Vec::new()
        },
        paths: vec![path(archive, "", false)],
        reads_members: listable,
    })
}

/// ouch (clap): `decompress|d` and `list|l|ls` take archives, `compress|c`
/// takes inputs and then the archive it creates. Options may come anywhere.
fn ouch_layout(args: &[&str]) -> Option<Layout> {
    let mut operands = Vec::new();
    let mut options = true;
    let mut i = 0;
    while i < args.len() {
        let arg = args[i];
        if !options || arg == "-" || !arg.starts_with('-') {
            operands.push(i);
        } else if arg == "--" {
            options = false;
        } else if let Some(long) = arg.strip_prefix("--") {
            let (name, value) = long
                .split_once('=')
                .map_or((long, None), |(n, v)| (n, Some(v)));
            match (name, value) {
                ("format" | "password" | "threads" | "dir" | "level", None) => {
                    i += 1;
                    args.get(i)?;
                }
                ("format" | "password" | "threads" | "dir" | "level", Some(_)) => {}
                (
                    "yes" | "no" | "accessible" | "hidden" | "quiet" | "gitignore" | "help"
                    | "version" | "tree" | "follow-symlinks" | "remove" | "here" | "fast" | "slow",
                    None,
                ) => {}
                _ => return None,
            }
        } else {
            for (at, letter) in arg[1..].char_indices() {
                if "fpcdl".contains(letter) {
                    if at + 2 == arg.len() {
                        i += 1;
                        args.get(i)?;
                    }
                    break;
                }
                if !"ynAHqghVtSr".contains(letter) {
                    return None;
                }
            }
        }
        i += 1;
    }
    let (&command, rest) = operands.split_first()?;
    let (inputs, archive) = match args[command] {
        "decompress" | "d" | "list" | "l" | "ls" => (rest, *rest.first()?),
        "compress" | "c" => {
            let (&archive, inputs) = rest.split_last()?;
            if inputs.is_empty() {
                return None;
            }
            (inputs, archive)
        }
        _ => return None,
    };
    if inputs.iter().any(|&at| args[at] == "-") {
        return None;
    }
    Some(Layout {
        archive: path(archive, "", false),
        members: Vec::new(),
        paths: inputs.iter().map(|&at| path(at, "", false)).collect(),
        reads_members: false,
    })
}

/// atool and its aliases: `ARCHIVE [FILE]...`, every operand an archive
/// with -e, `apack ARCHIVE FILE...`, `adiff A B`, `arepack OLD NEW`.
fn atool_layout(mut mode: AtoolMode, args: &[&str]) -> Option<Layout> {
    let mut each = false;
    let mut operands = Vec::new();
    let mut i = 0;
    let set = |mode: &mut AtoolMode, new: AtoolMode| {
        if *mode != AtoolMode::Unknown && *mode != new {
            return false;
        }
        *mode = new;
        true
    };
    while i < args.len() {
        let arg = args[i];
        if arg == "--" {
            operands.extend(i + 1..args.len());
            break;
        }
        if let Some(long) = arg.strip_prefix("--") {
            let (name, value) = long
                .split_once('=')
                .map_or((long, None), |(n, v)| (n, Some(v)));
            let new = match name {
                "list" => Some(AtoolMode::List),
                "extract" | "extract-to" => Some(AtoolMode::Extract),
                "add" => Some(AtoolMode::Add),
                "cat" => Some(AtoolMode::Cat),
                "diff" => Some(AtoolMode::Diff),
                "repack" => Some(AtoolMode::Repack),
                _ => None,
            };
            if let Some(new) = new
                && !set(&mut mode, new)
            {
                return None;
            }
            match (name, value) {
                (
                    "extract-to" | "format" | "format-option" | "verbosity" | "option" | "config"
                    | "save-outdir",
                    None,
                ) => {
                    i += 1;
                    args.get(i)?;
                }
                (
                    "extract-to" | "format" | "format-option" | "verbosity" | "option" | "config"
                    | "save-outdir",
                    Some(_),
                ) => {}
                (
                    "list" | "extract" | "add" | "cat" | "diff" | "repack" | "help" | "version"
                    | "each" | "subdir" | "force" | "quiet" | "verbose" | "page" | "explain"
                    | "simulate",
                    None,
                ) => each |= name == "each",
                // --null reads file names from stdin.
                _ => return None,
            }
        } else if let Some(letters) = arg.strip_prefix('-').filter(|l| !l.is_empty()) {
            for (at, letter) in letters.char_indices() {
                let new = match letter {
                    'l' => Some(AtoolMode::List),
                    'x' | 'X' => Some(AtoolMode::Extract),
                    'a' => Some(AtoolMode::Add),
                    'c' => Some(AtoolMode::Cat),
                    'd' => Some(AtoolMode::Diff),
                    'r' => Some(AtoolMode::Repack),
                    _ => None,
                };
                if let Some(new) = new
                    && !set(&mut mode, new)
                {
                    return None;
                }
                if "XFOVo".contains(letter) {
                    if at + 1 == letters.len() {
                        i += 1;
                        args.get(i)?;
                    }
                    break;
                }
                match letter {
                    'e' => each = true,
                    _ if "lxacdrDfqvpES".contains(letter) => {}
                    _ => return None,
                }
            }
        } else {
            operands.push(i);
        }
        i += 1;
    }
    let (&first, rest) = operands.split_first()?;
    let (archive, inputs): (usize, Vec<usize>) = match mode {
        AtoolMode::Unknown => return None,
        _ if each && mode != AtoolMode::Add => (first, operands.clone()),
        AtoolMode::List | AtoolMode::Extract | AtoolMode::Cat => (first, vec![first]),
        AtoolMode::Add if !rest.is_empty() => (first, rest.to_vec()),
        AtoolMode::Diff if rest.len() == 1 => (first, operands.clone()),
        AtoolMode::Repack if rest.len() == 1 => (first, vec![first]),
        _ => return None,
    };
    Some(Layout {
        archive: path(archive, "", false),
        members: Vec::new(),
        paths: inputs.into_iter().map(|at| path(at, "", false)).collect(),
        reads_members: false,
    })
}

/// `unsquashfs [OPTIONS] FILESYSTEM [files...]`: whole-word options, each
/// with its short and long spelling (`-d[est]`).
fn unsquashfs_layout(args: &[&str]) -> Option<Layout> {
    const VALUED: &[&str] = &[
        "d",
        "dest",
        "max",
        "max-depth",
        "all",
        "all-time",
        "pf",
        "pseudo-file",
        "xattrs-exclude",
        "xattrs-include",
        "p",
        "processors",
        "mem",
        "mem-percent",
        "help-option",
        "ho",
        "help-section",
        "hs",
        "cols",
        "o",
        "offset",
    ];
    const FLAGS: &[&str] = &[
        "excludes",
        "match",
        "follow",
        "follow-symlinks",
        "missing",
        "missing-symlinks",
        "no-wild",
        "no-wildcards",
        "r",
        "regex",
        "cat",
        "f",
        "force",
        "s",
        "stat",
        "i",
        "info",
        "li",
        "linfo",
        "l",
        "ls",
        "ll",
        "lls",
        "lln",
        "llnumeric",
        "lc",
        "llc",
        "full",
        "full-precision",
        "UTC",
        "mkfs-time",
        "fstime",
        "no",
        "no-xattrs",
        "x",
        "xattrs",
        "v",
        "version",
        "q",
        "quiet",
        "n",
        "no-progress",
        "percentage",
        "ig",
        "ignore-errors",
        "st",
        "strict-errors",
        "no-exit",
        "no-exit-code",
        "h",
        "help",
        "help-all",
        "ha",
        "no-pager",
        "L",
    ];
    let mut i = 0;
    while let Some(name) = args.get(i).and_then(|arg| arg.strip_prefix('-')) {
        if VALUED.contains(&name) {
            i += 1;
            args.get(i)?;
        } else if !FLAGS.contains(&name) {
            // -ef/-excf/-ex read selector lists.
            return None;
        }
        i += 1;
    }
    args.get(i)?;
    Some(Layout {
        archive: path(i, "", false),
        members: Vec::new(),
        paths: vec![path(i, "", false)],
        reads_members: false,
    })
}

/// Expand-Archive and Compress-Archive (Microsoft.PowerShell.Archive):
/// parameter names are case-insensitive unique prefixes, values may be
/// attached with `:`, and operands bind to -Path, then -DestinationPath.
/// Arrays (`a,b`) and -Path wildcards are declined.
fn powershell_layout(expand: bool, args: &[&str]) -> Option<Layout> {
    // (name, takes a value); aliases map to these names below.
    let mut parameters = vec![
        ("path", true),
        ("literalpath", true),
        ("destinationpath", true),
        ("force", false),
        ("passthru", false),
        ("whatif", false),
        ("confirm", false),
        ("verbose", false),
        ("debug", false),
        ("erroraction", true),
        ("warningaction", true),
        ("informationaction", true),
        ("progressaction", true),
        ("errorvariable", true),
        ("warningvariable", true),
        ("informationvariable", true),
        ("outvariable", true),
        ("outbuffer", true),
        ("pipelinevariable", true),
    ];
    if !expand {
        parameters.extend([("compressionlevel", true), ("update", false)]);
    }
    let alias = |name: &str| {
        Some(match name {
            "pspath" | "lp" => "literalpath",
            "ea" => "erroraction",
            "wa" => "warningaction",
            "infa" => "informationaction",
            "proga" => "progressaction",
            "ev" => "errorvariable",
            "wv" => "warningvariable",
            "iv" => "informationvariable",
            "ov" => "outvariable",
            "ob" => "outbuffer",
            "pv" => "pipelinevariable",
            "vb" => "verbose",
            "db" => "debug",
            "wi" => "whatif",
            "cf" => "confirm",
            _ => return None,
        })
    };
    let mut bound: Vec<(&str, PathArgument)> = Vec::new();
    let mut operands = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let arg = args[i];
        let Some(spelled) = arg.strip_prefix('-').filter(|name| !name.is_empty()) else {
            operands.push(i);
            i += 1;
            continue;
        };
        let (spelled, attached) = spelled
            .split_once(':')
            .map_or((spelled, None), |(name, value)| (name, Some(value)));
        let lower = spelled.to_ascii_lowercase();
        let name = match alias(&lower) {
            Some(name) => name,
            None => {
                let mut matches = parameters
                    .iter()
                    .map(|(name, _)| *name)
                    .filter(|name| name.starts_with(lower.as_str()));
                let first = matches.next()?;
                if first != lower && matches.next().is_some() {
                    return None;
                }
                first
            }
        };
        let valued = parameters.iter().any(|&(n, valued)| n == name && valued);
        let value = match (valued, attached) {
            (true, Some(value)) if !value.is_empty() => {
                path(i, &arg[..arg.len() - value.len()], false)
            }
            (true, None) => {
                i += 1;
                args.get(i)?;
                path(i, "", false)
            }
            (false, None) => {
                i += 1;
                continue;
            }
            // `-Force:$false` is opaque; `-Path:` without a value is an error.
            _ => return None,
        };
        if matches!(name, "path" | "literalpath" | "destinationpath") {
            if bound.iter().any(|(bound, _)| *bound == name) {
                return None;
            }
            bound.push((name, value));
        }
        i += 1;
    }
    let mut positions = ["path", "destinationpath"]
        .into_iter()
        .filter(|name| !bound.iter().any(|(bound, _)| bound == name))
        .collect::<Vec<_>>()
        .into_iter();
    for at in operands {
        bound.push((positions.next()?, path(at, "", false)));
    }
    let take = |name: &str| {
        bound
            .iter()
            .find(|(bound, _)| *bound == name)
            .map(|(_, argument)| argument)
    };
    let (input, literal) = match (take("path"), take("literalpath")) {
        (Some(input), None) => (input, false),
        (None, Some(input)) => (input, true),
        _ => return None,
    };
    let text = &args[input.index][input.prefix.len()..];
    if text.contains(',') || !literal && text.contains(['*', '?', '[', ']']) {
        return None;
    }
    let input = path(input.index, &input.prefix, false);
    let archive = if expand {
        path(input.index, &input.prefix, false)
    } else {
        let archive = take("destinationpath")?;
        path(archive.index, &archive.prefix, false)
    };
    Some(Layout {
        archive,
        members: Vec::new(),
        paths: vec![input],
        reads_members: false,
    })
}

/// `mksquashfs source... FILESYSTEM [OPTIONS]`: options follow the image.
fn mksquashfs_layout(args: &[&str]) -> Option<Layout> {
    let operands = args.iter().take_while(|arg| !arg.starts_with('-')).count();
    if operands < 2 {
        return None;
    }
    Some(Layout {
        archive: path(operands - 1, "", false),
        members: Vec::new(),
        paths: (0..operands - 1).map(|at| path(at, "", false)).collect(),
        reads_members: false,
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Attach {
    /// getopt: the rest of the word, else the next word (`-S.z`, `-S .z`).
    Getopt,
    /// zstd: `-o=FILE` or `-o FILE`.
    Equals,
    /// brotli: only the next word.
    Separate,
}

struct Syntax {
    /// Letters without a value; digits are compression levels.
    flags: &'static str,
    /// Letters that take a value.
    valued: &'static str,
    /// Letters whose value is the rest of the word (`-T0`, `-B7`, `-BD`).
    attached: &'static str,
    attach: Attach,
    long_flags: &'static [&'static str],
    /// Long options whose value may be the next word.
    long_valued: &'static [&'static str],
    /// Long options whose value can only be attached with `=`.
    long_attached: &'static [&'static str],
    /// Long options with an optional attached value (`--fast[=#]`).
    long_optional: &'static [&'static str],
    /// Recursion, file lists, benchmarks, and training change what the
    /// operands mean.
    declined: &'static [&'static str],
    /// `tool [input] [output]` (lz4, pixz).
    operand_output: bool,
    /// Letters whose value names an input file (pixz -i).
    input_valued: &'static str,
}

fn syntax(family: Family) -> Syntax {
    match family {
        // GNU gzip, Apple's gzip, and pigz.
        Family::Gzip => Syntax {
            flags: "0123456789acdfFhHikKlLmMnNOqRtUvVYz",
            valued: "AbCIJpS",
            attached: "",
            attach: Attach::Getopt,
            long_flags: &[
                "--fast",
                "--best",
                "--ascii",
                "--stdout",
                "--to-stdout",
                "--decompress",
                "--uncompress",
                "--force",
                "--first",
                "--help",
                "--huffman",
                "--independent",
                "--keep",
                "--zip",
                "--list",
                "--license",
                "--no-time",
                "--time",
                "--no-name",
                "--name",
                "--oneblock",
                "--quiet",
                "--rsyncable",
                "--test",
                "--rle",
                "--verbose",
                "--version",
                "--synchronous",
                "--zlib",
            ],
            long_valued: &[
                "--suffix",
                "--alias",
                "--blocksize",
                "--comment",
                "--iterations",
                "--maxsplits",
                "--processes",
            ],
            long_attached: &[],
            long_optional: &[],
            declined: &["-r", "--recursive"],
            operand_output: false,
            input_valued: "",
        },
        // bzip2 and pbzip2 (whose -r reads the input into memory).
        Family::Bzip2 => Syntax {
            flags: "0123456789dzkftcqvLVshlr",
            valued: "",
            attached: "bmpS",
            attach: Attach::Getopt,
            long_flags: &[
                "--help",
                "--decompress",
                "--compress",
                "--keep",
                "--force",
                "--test",
                "--stdout",
                "--quiet",
                "--verbose",
                "--license",
                "--version",
                "--small",
                "--fast",
                "--best",
                "--repetitive-fast",
                "--repetitive-best",
            ],
            long_valued: &[],
            long_attached: &["--ignore-trailing-garbage"],
            long_optional: &[],
            declined: &[],
            operand_output: false,
            input_valued: "",
        },
        Family::Xz => Syntax {
            flags: "0123456789zdtlkfceqvhHVQ",
            valued: "TFCSM",
            attached: "",
            attach: Attach::Getopt,
            long_flags: &[
                "--compress",
                "--decompress",
                "--uncompress",
                "--test",
                "--list",
                "--keep",
                "--force",
                "--stdout",
                "--to-stdout",
                "--extreme",
                "--quiet",
                "--verbose",
                "--help",
                "--long-help",
                "--version",
                "--no-adjust",
                "--robot",
                "--single-stream",
                "--no-sparse",
                "--ignore-check",
                "--info-memory",
                "--no-warn",
                "--fast",
                "--best",
                "--filters-help",
            ],
            long_valued: &[
                "--format",
                "--check",
                "--suffix",
                "--memlimit",
                "--memory",
                "--memlimit-compress",
                "--memlimit-decompress",
                "--memlimit-mt",
                "--threads",
                "--block-size",
                "--block-list",
                "--flush-timeout",
                "--filters",
                "--filters1",
                "--filters2",
                "--filters3",
                "--filters4",
                "--filters5",
                "--filters6",
                "--filters7",
                "--filters8",
                "--filters9",
            ],
            long_attached: &[],
            long_optional: &[
                "--x86",
                "--arm",
                "--armthumb",
                "--arm64",
                "--powerpc",
                "--ia64",
                "--sparc",
                "--riscv",
                "--lzma1",
                "--lzma2",
                "--delta",
            ],
            declined: &["--files", "--files0"],
            operand_output: false,
            input_valued: "",
        },
        Family::Zstd => Syntax {
            flags: "0123456789dzcfkqvhHVltC",
            valued: "oD",
            attached: "TBM",
            attach: Attach::Equals,
            long_flags: &[
                "--keep",
                "--rm",
                "--decompress",
                "--uncompress",
                "--compress",
                "--force",
                "--help",
                "--version",
                "--stdout",
                "--verbose",
                "--quiet",
                "--progress",
                "--no-progress",
                "--asyncio",
                "--no-asyncio",
                "--check",
                "--no-check",
                "--ultra",
                "--single-thread",
                "--rsyncable",
                "--exclude-compressed",
                "--no-dictID",
                "--compress-literals",
                "--no-compress-literals",
                "--row-match-finder",
                "--no-row-match-finder",
                "--mmap-dict",
                "--no-mmap-dict",
                "--list",
                "--test",
                "--sparse",
                "--no-sparse",
                "--content-size",
                "--no-content-size",
                "--pass-through",
                "--no-pass-through",
                "--show-default-cparams",
            ],
            long_valued: &[
                "--trace",
                "--output-dir-flat",
                "--output-dir-mirror",
                "--patch-from",
            ],
            long_attached: &[
                "--memory",
                "--memlimit",
                "--memlimit-decompress",
                "--block-size",
                "--stream-size",
                "--size-hint",
                "--target-compressed-block-size",
                "--zstd",
                "--auto-threads",
                "--threads",
                "--format",
            ],
            long_optional: &["--fast", "--long", "--adapt"],
            declined: &[
                "-r",
                "-b",
                "-e",
                "-i",
                "-S",
                "--filelist",
                "--train",
                "--train-cover",
                "--train-fastcover",
                "--train-legacy",
            ],
            operand_output: false,
            input_valued: "",
        },
        Family::Lz4 | Family::Lz4Cat => Syntax {
            flags: "0123456789dzcfkqvhHVltm",
            valued: "D",
            attached: "TB",
            attach: Attach::Getopt,
            long_flags: &[
                "--compress",
                "--decompress",
                "--uncompress",
                "--stdout",
                "--to-stdout",
                "--force",
                "--keep",
                "--rm",
                "--quiet",
                "--verbose",
                "--help",
                "--version",
                "--test",
                "--multiple",
                "--sparse",
                "--no-sparse",
                "--content-size",
                "--no-content-size",
                "--frame-crc",
                "--no-frame-crc",
                "--favor-decSpeed",
                "--best",
                "--list",
            ],
            long_valued: &[],
            long_attached: &[],
            long_optional: &["--fast"],
            declined: &["-r", "-b", "-e", "-i"],
            operand_output: true,
            input_valued: "",
        },
        Family::Brotli => Syntax {
            flags: "0123456789cdfhjsktvnKVZ",
            valued: "oqwCDS",
            attached: "",
            attach: Attach::Separate,
            long_flags: &[
                "--stdout",
                "--decompress",
                "--force",
                "--help",
                "--rm",
                "--squash",
                "--keep",
                "--no-copy-stat",
                "--test",
                "--verbose",
                "--concatenated",
                "--version",
                "--best",
            ],
            long_valued: &[],
            long_attached: &[
                "--output",
                "--quality",
                "--lgwin",
                "--large_window",
                "--comment",
                "--dictionary",
                "--suffix",
            ],
            long_optional: &[],
            declined: &[],
            operand_output: false,
            input_valued: "",
        },
        Family::Compress => Syntax {
            flags: "cfv",
            valued: "b",
            attached: "",
            attach: Attach::Getopt,
            long_flags: &[],
            long_valued: &[],
            long_attached: &[],
            long_optional: &[],
            declined: &[],
            operand_output: false,
            input_valued: "",
        },
        // lzip, plzip, lunzip, and clzip share lzip's arg_parser syntax.
        Family::Lzip => Syntax {
            flags: "0123456789hVacdfFklqtv",
            valued: "bmosSBn",
            attached: "",
            attach: Attach::Getopt,
            long_flags: &[
                "--help",
                "--version",
                "--trailing-error",
                "--stdout",
                "--decompress",
                "--force",
                "--recompress",
                "--keep",
                "--list",
                "--quiet",
                "--test",
                "--verbose",
                "--loose-trailing",
                "--check-lib",
                "--fast",
                "--best",
            ],
            long_valued: &[
                "--member-size",
                "--match-length",
                "--output",
                "--dictionary-size",
                "--volume-size",
                "--data-size",
                "--threads",
                "--in-slots",
                "--out-slots",
            ],
            long_attached: &[],
            long_optional: &[],
            declined: &[],
            operand_output: false,
            input_valued: "",
        },
        // -p takes an optional attached directory.
        Family::Lzop => Syntax {
            flags: "0123456789dxlthIVLqvcfFnNPkU",
            valued: "oS",
            attached: "p",
            attach: Attach::Getopt,
            long_flags: &[
                "--decompress",
                "--uncompress",
                "--extract",
                "--list",
                "--info",
                "--sysinfo",
                "--license",
                "--help",
                "--version",
                "--test",
                "--stdout",
                "--to-stdout",
                "--force",
                "--name",
                "--no-name",
                "--keep",
                "--delete",
                "--quiet",
                "--silent",
                "--verbose",
                "--no-warn",
                "--ignore-warn",
                "--no-stdin",
                "--crc32",
                "--no-checksum",
                "--no-mode",
                "--no-time",
                "--fast",
                "--best",
                "--path",
            ],
            long_valued: &["--output", "--suffix"],
            long_attached: &["--path"],
            long_optional: &["--ls"],
            declined: &["-r", "--recursive", "--filter"],
            operand_output: false,
            input_valued: "",
        },
        // pixz [-d|-l] [-i input] [-o output] [input [output]]; -x selects
        // tarball members read from stdin.
        Family::Pixz => Syntax {
            flags: "0123456789dltkceh",
            valued: "iop",
            attached: "",
            attach: Attach::Getopt,
            long_flags: &[],
            long_valued: &[],
            long_attached: &[],
            long_optional: &[],
            declined: &["-x"],
            operand_output: true,
            input_valued: "i",
        },
        Family::Cabextract => Syntax {
            flags: "vhltqLfiknps",
            valued: "Fed",
            attached: "",
            attach: Attach::Getopt,
            long_flags: &[
                "--version",
                "--help",
                "--list",
                "--test",
                "--quiet",
                "--lowercase",
                "--fix",
                "--interactive",
                "--no-overwrite",
                "--keep-symlinks",
                "--pipe",
                "--single",
            ],
            long_valued: &["--filter", "--encoding", "--directory"],
            long_attached: &[],
            long_optional: &[],
            declined: &[],
            operand_output: false,
            input_valued: "",
        },
        // optparse; -r extracts nested archives and keeps operands archives.
        Family::Dtrx => Syntax {
            flags: "hltmrnofvq",
            valued: "p",
            attached: "",
            attach: Attach::Getopt,
            long_flags: &[
                "--version",
                "--help",
                "--list",
                "--table",
                "--metadata",
                "--recursive",
                "--noninteractive",
                "--overwrite",
                "--flat",
                "--no-directory",
                "--list-extensions",
                "--verbose",
                "--quiet",
            ],
            long_valued: &["--one", "--one-entry", "--password"],
            long_attached: &[],
            long_optional: &[],
            declined: &[],
            operand_output: false,
            input_valued: "",
        },
    }
}

fn inputs_layout(family: Family, args: &[&str]) -> Option<Layout> {
    let syntax = syntax(family);
    let lz4 = matches!(family, Family::Lz4 | Family::Lz4Cat);
    let mut multiple = family == Family::Lz4Cat;
    let mut inputs = Vec::new();
    // Option values naming inputs (pixz -i): (index, attached prefix).
    let mut valued_inputs: Vec<PathArgument> = Vec::new();
    let mut options = true;
    let mut i = 0;
    while i < args.len() {
        let arg = args[i];
        if !options || arg == "-" || !arg.starts_with('-') {
            inputs.push(i);
        } else if arg == "--" {
            options = false;
        } else if arg.starts_with("--") {
            let (name, value) = arg
                .split_once('=')
                .map_or((arg, None), |(name, value)| (name, Some(value)));
            if syntax.declined.contains(&name) {
                return None;
            }
            match value {
                None if syntax.long_flags.contains(&name)
                    || syntax.long_optional.contains(&name) => {}
                None if syntax.long_valued.contains(&name) => {
                    i += 1;
                    args.get(i)?;
                }
                Some(value)
                    if !value.is_empty()
                        && (syntax.long_valued.contains(&name)
                            || syntax.long_attached.contains(&name)
                            || syntax.long_optional.contains(&name)) => {}
                _ => return None,
            }
            if lz4 && matches!(name, "--multiple" | "--list") {
                multiple = true;
            }
        } else {
            let letters = &arg[1..];
            for (at, letter) in letters.char_indices() {
                let rest = &letters[at + letter.len_utf8()..];
                if syntax
                    .declined
                    .iter()
                    .any(|d| d.len() == 2 && d.ends_with(letter))
                {
                    return None;
                }
                if syntax.attached.contains(letter) {
                    break;
                }
                if syntax.valued.contains(letter) {
                    let separate = match (syntax.attach, rest) {
                        (_, "") => true,
                        (Attach::Getopt, _) => false,
                        (Attach::Equals, rest) if rest.len() > 1 && rest.starts_with('=') => false,
                        _ => return None,
                    };
                    if separate {
                        i += 1;
                        args.get(i)?;
                    }
                    if syntax.input_valued.contains(letter) {
                        valued_inputs.push(if separate {
                            path(i, "", false)
                        } else {
                            path(i, &arg[..arg.len() - rest.len()], false)
                        });
                    }
                    break;
                }
                if !syntax.flags.contains(letter) {
                    return None;
                }
                if lz4 && letter == 'm' {
                    multiple = true;
                }
            }
        }
        i += 1;
    }
    if syntax.operand_output && !multiple {
        // tool [input] [output]: a third operand is an error.
        if inputs.len() > 2 || !valued_inputs.is_empty() && !inputs.is_empty() {
            return None;
        }
        inputs.truncate(1);
    }
    if inputs
        .iter()
        .any(|&at| args[at] == "-" || lz4 && ["stdin", "stdout", "null"].contains(&args[at]))
    {
        return None;
    }
    let mut paths: Vec<PathArgument> = inputs.iter().map(|&at| path(at, "", false)).collect();
    paths.extend(valued_inputs);
    let first = paths.first()?;
    Some(Layout {
        archive: path(first.index, &first.prefix, false),
        members: Vec::new(),
        paths,
        reads_members: false,
    })
}

#[derive(Clone, Copy, Debug)]
enum Format {
    GnuTar,
    BsdTar,
    Unzip,
    Zipinfo,
    SevenZip,
    Ar,
    Lsar,
    Jar,
}

pub(crate) struct Listing {
    program: PathBuf,
    archive: PathBuf,
    format: Format,
    directory: PathBuf,
    pub members: Vec<String>,
}

fn listing_env(directory: &Path) -> Vec<(OsString, Option<OsString>)> {
    let mut env = native::offline_env(Flavor::Cobra);
    env.extend(
        [
            "TAR_OPTIONS",
            "TAPE",
            "GZIP",
            "BZIP",
            "BZIP2",
            "XZ_OPT",
            "XZ_DEFAULTS",
            "ZSTD_CLEVEL",
            "ZSTD_NBTHREADS",
            "UNZIP",
            "UNZIPOPT",
            "ZIPINFO",
            "ZIPINFOOPT",
            "ZIPOPT",
            "LD_PRELOAD",
            "DYLD_INSERT_LIBRARIES",
            "DYLD_LIBRARY_PATH",
            // JVM agents and classpath additions run code inside jar.
            "JAVA_TOOL_OPTIONS",
            "JDK_JAVA_OPTIONS",
            "_JAVA_OPTIONS",
            "CLASSPATH",
        ]
        .map(|name| (name.into(), None)),
    );
    env.extend([
        ("HOME".into(), Some(directory.as_os_str().to_owned())),
        ("LC_ALL".into(), Some("C".into())),
    ]);
    env
}

fn query(
    program: &Path,
    directory: &Path,
    args: Vec<OsString>,
    budget: &mut Budget,
) -> Result<Vec<u8>, String> {
    let output = probe::run_in(
        &Probe {
            program,
            args,
            env: listing_env(directory),
            capture: Capture::Stdout,
        },
        directory,
        budget,
    )
    .map_err(|e| e.to_string())?;
    if output.truncated || output.status != Some(0) {
        return Err("archive listing failed or exceeded its output limit".into());
    }
    Ok(output.data)
}

impl Listing {
    pub fn open(
        tool: Tool,
        program: &Path,
        archive: &Path,
        budget: &mut Budget,
    ) -> Result<Self, String> {
        if !probe::is_trusted_location(program) {
            return Err("archive tool is not at an absolute path".into());
        }
        // unar lists through lsar, installed beside it by the same package.
        let lister;
        let program = if tool == Tool::Unar {
            lister = program.with_file_name("lsar");
            if !lister.is_file() {
                return Err("lsar is not installed beside unar".into());
            }
            lister.as_path()
        } else {
            program
        };
        let archive = std::fs::canonicalize(archive).map_err(|e| e.to_string())?;
        if !archive.is_file() {
            return Err("archive is not a regular file".into());
        }
        let directory = crate::utils::cache_dir().join("archive-probes");
        std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
        let format = match tool {
            Tool::Tar => {
                let banner = query(program, &directory, vec!["--version".into()], budget)?;
                let banner = std::str::from_utf8(&banner).map_err(|e| e.to_string())?;
                if banner.starts_with("tar (GNU tar)") || banner.starts_with("gtar (GNU tar)") {
                    Format::GnuTar
                } else if banner.starts_with("bsdtar ") && banner.contains("libarchive") {
                    Format::BsdTar
                } else {
                    return Err("unrecognized tar implementation".into());
                }
            }
            Tool::Unzip => Format::Unzip,
            Tool::Zipinfo => Format::Zipinfo,
            Tool::SevenZip => Format::SevenZip,
            Tool::Ar => Format::Ar,
            Tool::Jar => Format::Jar,
            Tool::Unar | Tool::Lsar => Format::Lsar,
            Tool::Zip
            | Tool::Pax
            | Tool::Cpio
            | Tool::Ditto
            | Tool::Ouch
            | Tool::Atool(_)
            | Tool::Unsquashfs
            | Tool::Mksquashfs
            | Tool::PowerShellArchive(_)
            | Tool::Inputs(_) => {
                return Err("this tool has no member listing".into());
            }
        };
        let mut listing = Self {
            program: program.into(),
            archive,
            format,
            directory,
            members: Vec::new(),
        };
        listing.members = listing.read(None, budget)?;
        Ok(listing)
    }

    fn read(&self, member: Option<&str>, budget: &mut Budget) -> Result<Vec<String>, String> {
        let args: Vec<OsString> = match self.format {
            Format::GnuTar => vec![
                "--list".into(),
                "--quoting-style=c".into(),
                "--force-local".into(),
                "--file".into(),
                self.archive.as_os_str().to_owned(),
            ],
            Format::BsdTar => vec!["-tf".into(), self.archive.as_os_str().to_owned()],
            Format::Unzip => vec!["-Z1".into(), self.archive.as_os_str().to_owned()],
            Format::Zipinfo => vec!["-1".into(), self.archive.as_os_str().to_owned()],
            Format::Ar => vec!["t".into(), self.archive.as_os_str().to_owned()],
            // The JVM prints names in its console encoding, which follows
            // the C locale unless these properties say otherwise.
            Format::Jar => vec![
                "-J-Dstdout.encoding=UTF-8".into(),
                "-J-Dsun.stdout.encoding=UTF-8".into(),
                "tf".into(),
                self.archive.as_os_str().to_owned(),
            ],
            // ASCII JSON escapes every name losslessly.
            Format::Lsar => vec![
                "-ja".into(),
                "-jss".into(),
                self.archive.as_os_str().to_owned(),
            ],
            Format::SevenZip => vec![
                "l".into(),
                "-slt".into(),
                "-ba".into(),
                "-bd".into(),
                "-sccUTF-8".into(),
                "-spd".into(),
                "-r-".into(),
                "--".into(),
                self.archive.as_os_str().to_owned(),
            ],
        };
        let mut args = args;
        if let Some(member) = member {
            args.push(member.into());
        }
        let data = query(&self.program, &self.directory, args, budget)?;
        parse_listing(self.format, &data, budget.max_candidates)
    }

    /// Zip/7-Zip console displays can be lossy. Selecting the displayed
    /// name must return that exact name again before it becomes a candidate.
    pub fn verifies(&self, member: &str, budget: &mut Budget) -> bool {
        match self.format {
            Format::GnuTar | Format::BsdTar => true,
            Format::Unzip
            | Format::Zipinfo
            | Format::SevenZip
            | Format::Ar
            | Format::Lsar
            | Format::Jar => self
                .read(Some(member), budget)
                .is_ok_and(|members| members.iter().any(|name| name == member)),
        }
    }
}

pub(crate) fn literal_member(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 4096
        && !name.starts_with(['/', '\\', '-', '@'])
        && !name.contains(['*', '?', '[', ']', '\\', ':'])
        && !name.chars().any(|c| {
            c.is_control() || matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        })
        && !name.split('/').any(|component| component == "..")
}

pub(crate) fn contains(members: &[String], typed: &str) -> bool {
    members.iter().any(|member| {
        member == typed || member.starts_with(&format!("{}/", typed.trim_end_matches('/')))
    })
}

fn parse_listing(format: Format, data: &[u8], limit: usize) -> Result<Vec<String>, String> {
    if let Format::Lsar = format {
        return parse_lsar(data, limit);
    }
    let mut names = Vec::new();
    for line in data.split(|&b| b == b'\n').filter(|line| !line.is_empty()) {
        // Info-ZIP may print a name in an encoding it cannot represent.
        let Ok(line) = std::str::from_utf8(line) else {
            if matches!(format, Format::Unzip | Format::Zipinfo) {
                continue;
            }
            return Err("archive listing is not UTF-8".into());
        };
        let name = match format {
            Format::GnuTar => unescape(
                line.strip_prefix('"')
                    .and_then(|s| s.strip_suffix('"'))
                    .ok_or("malformed GNU tar member")?,
            )?,
            Format::BsdTar => unescape(line)?,
            Format::Unzip | Format::Zipinfo | Format::Ar | Format::Jar => line.to_owned(),
            Format::SevenZip => {
                let Some(name) = line.strip_prefix("Path = ") else {
                    if !line.contains(" = ") {
                        return Err("malformed 7-Zip listing".into());
                    }
                    continue;
                };
                name.to_owned()
            }
            // Parsed as a whole above.
            Format::Lsar => return Err("lsar listings are JSON".into()),
        };
        // Count every displayed entry, including ones we cannot use.
        if names.len() >= limit {
            return Err("archive has too many members".into());
        }
        names.push(name);
    }
    names.retain(|name| literal_member(name));
    names.sort();
    names.dedup();
    Ok(names)
}

fn parse_lsar(data: &[u8], limit: usize) -> Result<Vec<String>, String> {
    let value: serde_json::Value =
        serde_json::from_slice(data).map_err(|_| "malformed lsar listing")?;
    let entries = value
        .get("lsarContents")
        .and_then(|contents| contents.as_array())
        .ok_or("lsar listing has no contents")?;
    if entries.len() > limit {
        return Err("archive has too many members".into());
    }
    let mut names = Vec::new();
    for entry in entries {
        names.push(
            entry
                .get("XADFileName")
                .and_then(|name| name.as_str())
                .ok_or("lsar entry has no name")?
                .to_owned(),
        );
    }
    names.retain(|name| literal_member(name));
    names.sort();
    names.dedup();
    Ok(names)
}

/// Both tar implementations escape bytes with C-style escapes and octal.
fn unescape(value: &str) -> Result<String, String> {
    let mut output = Vec::new();
    let mut bytes = value.bytes();
    while let Some(byte) = bytes.next() {
        if byte != b'\\' {
            output.push(byte);
            continue;
        }
        let byte = bytes.next().ok_or("incomplete tar escape")?;
        output.push(match byte {
            b'a' => 7,
            b'b' => 8,
            b'f' => 12,
            b'n' => b'\n',
            b'r' => b'\r',
            b't' => b'\t',
            b'v' => 11,
            b'\\' | b'"' => byte,
            b'0'..=b'7' => {
                let b = bytes
                    .next()
                    .filter(|b| (b'0'..=b'7').contains(b))
                    .ok_or("invalid tar octal escape")?;
                let c = bytes
                    .next()
                    .filter(|b| (b'0'..=b'7').contains(b))
                    .ok_or("invalid tar octal escape")?;
                let octal =
                    u16::from(byte - b'0') * 64 + u16::from(b - b'0') * 8 + u16::from(c - b'0');
                u8::try_from(octal).map_err(|_| "invalid tar octal byte")?
            }
            _ => return Err("unknown tar escape".into()),
        });
    }
    String::from_utf8(output).map_err(|_| "tar member is not UTF-8".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(tool: Tool, args: &[&str]) -> Option<Layout> {
        layout(tool, &args.iter().copied().map(Some).collect::<Vec<_>>())
    }

    #[test]
    fn tar_clusters_old_style_and_attached_paths_keep_their_slots() {
        for args in [
            vec!["xvzf", "a.tar.gz", "dir/file"],
            vec!["-xvzf", "a.tar.gz", "dir/file"],
            vec!["--extract", "--file", "a.tar.gz", "dir/file"],
        ] {
            let l = parse(Tool::Tar, &args).unwrap();
            assert_eq!(
                l.archive.index,
                1.max(if args[0] == "--extract" { 2 } else { 1 })
            );
            assert_eq!(l.members, vec![args.len() - 1]);
        }
        let l = parse(Tool::Tar, &["-xfarchive.tar", "-Cout", "dir/file"]).unwrap();
        assert_eq!(l.archive, path(0, "-xf", false));
        assert_eq!(l.paths[1], path(1, "-C", true));
        assert_eq!(l.members, vec![2]);
        let l = parse(Tool::Tar, &["cf", "new.tar", "input.txt"]).unwrap();
        assert!(!l.reads_members);
        assert_eq!(l.paths, vec![path(2, "", false)]);
        assert!(l.members.is_empty());
    }

    #[test]
    fn selectors_passwords_and_destinations_are_not_members() {
        let l = parse(
            Tool::Unzip,
            &[
                "-P",
                "secret",
                "a.zip",
                "dir/file",
                "-dout",
                "-x",
                "skip/file",
            ],
        )
        .unwrap();
        assert_eq!(l.archive.index, 2);
        assert_eq!(l.members, vec![3]);
        assert_eq!(l.paths[1], path(4, "-d", true));
        let l = parse(
            Tool::SevenZip,
            &["x", "-psecret", "-mx=9", "a.7z", "dir/file", "-oout"],
        )
        .unwrap();
        assert_eq!(l.archive.index, 3);
        assert_eq!(l.members, vec![4]);
        assert_eq!(l.paths[1], path(5, "-o", true));
        for args in [
            vec!["xf", "-"],
            vec!["-x", "-f"],
            vec!["xf", "a.tar", "--unknown", "value"],
            vec!["cxvf", "a.tar"],
        ] {
            assert!(parse(Tool::Tar, &args).is_none(), "{args:?}");
        }
        assert!(layout(Tool::Tar, &[Some("xf"), None, Some("member")]).is_none());
        assert!(parse(Tool::SevenZip, &["x", "a.7z", "@file-list"]).is_none());
        assert!(parse(Tool::SevenZip, &["x", "-si", "a.7z"]).is_none());
        assert!(parse(Tool::Unzip, &["-O", "utf8", "a.zip", "member"]).is_none());
        assert!(
            parse(Tool::Tar, &["xf", "a.tar", "-T", "list"])
                .unwrap()
                .members
                .is_empty()
        );
    }

    fn inputs(family: Family, args: &[&str]) -> Option<Vec<usize>> {
        parse(Tool::Inputs(family), args).map(|l| l.paths.iter().map(|p| p.index).collect())
    }

    #[test]
    fn compressor_operands_skip_option_values_and_outputs() {
        let gzip = Family::Gzip;
        assert_eq!(
            inputs(gzip, &["-dc", "-S", ".z", "a.z", "b.z"]),
            Some(vec![3, 4])
        );
        assert_eq!(inputs(gzip, &["-S.z", "a.z"]), Some(vec![1]));
        assert_eq!(inputs(gzip, &["--", "-dash.gz"]), Some(vec![1]));
        assert_eq!(inputs(Family::Xz, &["--format", "raw", "a"]), Some(vec![2]));
        assert_eq!(
            inputs(Family::Xz, &["-dT0", "--lzma2=preset=6", "a.xz"]),
            Some(vec![2])
        );
        assert_eq!(
            inputs(Family::Zstd, &["-d", "a.zst", "-o", "out"]),
            Some(vec![1])
        );
        assert_eq!(
            inputs(Family::Zstd, &["-19", "-T0", "-o=out", "a"]),
            Some(vec![3])
        );
        assert_eq!(inputs(Family::Lz4, &["-d", "a.lz4", "out"]), Some(vec![1]));
        assert_eq!(
            inputs(Family::Lz4, &["-m", "a.lz4", "b.lz4"]),
            Some(vec![1, 2])
        );
        assert_eq!(
            inputs(Family::Lz4Cat, &["a.lz4", "b.lz4"]),
            Some(vec![0, 1])
        );
        assert_eq!(inputs(Family::Brotli, &["-q", "5", "a"]), Some(vec![2]));
        assert_eq!(
            inputs(Family::Brotli, &["--output=o.br", "a"]),
            Some(vec![1])
        );
        assert_eq!(inputs(Family::Compress, &["-b12", "a"]), Some(vec![1]));
        for (family, args) in [
            (gzip, vec!["-r", "dir"]),
            (gzip, vec!["-dr", "dir"]),
            (gzip, vec!["--bogus", "a.gz"]),
            (gzip, vec!["-d"]),
            (gzip, vec!["-d", "-"]),
            (gzip, vec!["-S"]),
            (Family::Xz, vec!["--files=list"]),
            (Family::Zstd, vec!["-ofile", "a"]),
            (Family::Zstd, vec!["--filelist", "list"]),
            (Family::Zstd, vec!["-b3", "a"]),
            (Family::Lz4, vec!["a", "b", "c"]),
            (Family::Lz4, vec!["stdin", "out"]),
            (Family::Brotli, vec!["-q5", "a"]),
            (Family::Brotli, vec!["--output", "o.br", "a"]),
        ] {
            assert!(inputs(family, &args).is_none(), "{family:?} {args:?}");
        }
    }

    #[test]
    fn archivers_find_their_archive_operands() {
        let l = parse(Tool::Zipinfo, &["-1", "a.zip", "m", "-x", "skip"]).unwrap();
        assert_eq!((l.archive.index, l.members.clone()), (1, vec![2]));
        assert!(parse(Tool::Zipinfo, &["-O", "cp437", "a.zip"]).is_none());
        let l = parse(Tool::Ar, &["t", "lib.a", "m.o"]).unwrap();
        assert_eq!((l.archive.index, l.members.clone()), (1, vec![2]));
        assert!(l.reads_members);
        let l = parse(Tool::Ar, &["-t", "-v", "lib.a", "m.o"]).unwrap();
        assert_eq!((l.archive.index, l.members.clone()), (2, vec![3]));
        let l = parse(Tool::Ar, &["rcs", "lib.a", "f.o"]).unwrap();
        assert!(!l.reads_members && l.members.is_empty());
        assert_eq!(l.paths, vec![path(2, "", false)]);
        // a/b/i name a position member before the archive.
        let l = parse(Tool::Ar, &["mb", "f.o", "lib.a", "g.o"]).unwrap();
        assert_eq!((l.archive.index, l.members.clone()), (2, vec![3]));
        assert_eq!(parse(Tool::Ar, &["s", "lib.a"]).unwrap().archive.index, 1);
        for args in [
            vec!["x", "lib.a", "@list"],
            vec!["--plugin", "p", "t", "lib.a"],
            vec!["tx", "lib.a"],
            vec!["t"],
        ] {
            assert!(parse(Tool::Ar, &args).is_none(), "{args:?}");
        }
        let l = parse(Tool::Pax, &["-rvf", "a.pax", "pattern"]).unwrap();
        assert_eq!(l.paths, vec![path(1, "", false)]);
        assert!(l.members.is_empty());
        assert_eq!(
            parse(Tool::Pax, &["-fa.pax"]).unwrap().paths,
            vec![path(0, "-f", false)]
        );
        let l = parse(Tool::Pax, &["-w", "-f", "new.pax", "a", "b"]).unwrap();
        assert_eq!(l.paths, vec![path(3, "", false), path(4, "", false)]);
        assert!(parse(Tool::Pax, &["-rw", "a", "dir"]).is_none());
        assert!(parse(Tool::Pax, &["-r"]).is_none());
        let l = parse(Tool::Cpio, &["-it", "-F", "a.cpio", "pattern"]).unwrap();
        assert_eq!(l.paths, vec![path(2, "", false)]);
        assert_eq!(
            parse(Tool::Cpio, &["--file=a.cpio", "-i"]).unwrap().paths,
            vec![path(0, "--file=", false)]
        );
        for args in [
            vec!["-it"],
            vec!["-o", "-F", "new.cpio"],
            vec!["-i", "-F", "host:a.cpio"],
            vec!["-i", "-E", "patterns", "-F", "a.cpio"],
        ] {
            assert!(parse(Tool::Cpio, &args).is_none(), "{args:?}");
        }
        let l = parse(Tool::Ditto, &["-xk", "a.zip", "out"]).unwrap();
        assert_eq!(l.paths, vec![path(1, "", false)]);
        let l = parse(Tool::Ditto, &["--bom", "b", "-c", "-k", "src", "a.zip"]).unwrap();
        assert_eq!(l.archive.index, 5);
        assert_eq!(l.paths, vec![path(4, "", false)]);
        assert!(parse(Tool::Ditto, &["src", "dst"]).is_none());
        assert!(parse(Tool::Ditto, &["-x", "a.zip"]).is_none());
        assert!(host_paths_only(tool("/usr/bin/gunzip").unwrap()));
        assert!(!host_paths_only(tool("ar").unwrap()));
    }

    #[test]
    fn powershell_archive_parameters_bind_by_prefix_and_position() {
        let expand = |args: &[&str]| parse(Tool::PowerShellArchive(true), args);
        let compress = |args: &[&str]| parse(Tool::PowerShellArchive(false), args);
        assert_eq!(
            tool("Microsoft.PowerShell.Archive\\expand-ARCHIVE"),
            Some(Tool::PowerShellArchive(true))
        );
        assert_eq!(
            expand(&["a.zip", "out"]).unwrap().paths,
            vec![path(0, "", false)]
        );
        assert_eq!(
            expand(&["-Dest", "out", "a.zip"]).unwrap().paths,
            vec![path(2, "", false)]
        );
        assert_eq!(
            expand(&["-Path:a.zip", "-Force"]).unwrap().paths,
            vec![path(0, "-Path:", false)]
        );
        assert_eq!(
            expand(&["-lp", "a[1].zip"]).unwrap().paths,
            vec![path(1, "", false)]
        );
        let l = compress(&[
            "-Path",
            "notes.txt",
            "-DestinationPath",
            "new.zip",
            "-Update",
        ])
        .unwrap();
        assert_eq!(l.archive.index, 3);
        assert_eq!(l.paths, vec![path(1, "", false)]);
        for args in [
            vec!["-P", "a.zip"],
            vec!["a.zip", "out", "extra"],
            vec!["a*.zip"],
            vec!["a.zip,b.zip"],
            vec!["-Bogus", "a.zip"],
            vec!["-Force:$false", "a.zip"],
            vec!["-Path", "a.zip", "-LiteralPath", "b.zip"],
        ] {
            assert!(expand(&args).is_none(), "{args:?}");
        }
        assert!(compress(&["notes.txt"]).is_none());
    }

    #[test]
    fn jar_old_and_gnu_styles_find_the_archive_and_members() {
        let l = parse(Tool::Jar, &["tf", "app.jar", "META-INF/MANIFEST.MF"]).unwrap();
        assert_eq!((l.archive.index, l.members.clone()), (1, vec![2]));
        // Values follow in the order of their letters.
        let l = parse(Tool::Jar, &["cmf", "manifest.txt", "app.jar", "a.class"]).unwrap();
        assert_eq!(l.archive.index, 2);
        assert_eq!(l.paths, vec![path(3, "", false), path(1, "", false)]);
        let l = parse(Tool::Jar, &["-xvf", "app.jar", "a.txt"]).unwrap();
        assert_eq!((l.archive.index, l.members.clone()), (1, vec![2]));
        let l = parse(Tool::Jar, &["--list", "--file=app.jar"]).unwrap();
        assert_eq!(l.archive, path(1, "--file=", false));
        let l = parse(
            Tool::Jar,
            &["--create", "--file", "a.jar", "-C", "dir", ".", "b.txt"],
        )
        .unwrap();
        assert_eq!(l.paths, vec![path(6, "", false)]);
        for args in [
            vec!["tf"],
            vec!["-t"],
            vec!["xtf", "a.jar"],
            vec!["-tfapp.jar"],
            vec!["--list", "--file", "a.jar", "@more"],
            vec!["--generate-index=a.jar"],
        ] {
            assert!(parse(Tool::Jar, &args).is_none(), "{args:?}");
        }
    }

    #[test]
    fn zip_inputs_follow_whole_option_words() {
        let indices = |args: &[&str]| {
            parse(Tool::Zip, args).map(|l| l.paths.iter().map(|p| p.index).collect::<Vec<_>>())
        };
        assert_eq!(
            indices(&["-r", "out.zip", "a.txt", "dir"]),
            Some(vec![2, 3])
        );
        assert_eq!(indices(&["-P", "pw", "out.zip", "a.txt"]), Some(vec![3]));
        assert_eq!(
            indices(&["out.zip", "a.txt", "-x", "*.o", "b.o", "-q"]),
            Some(vec![1])
        );
        assert_eq!(indices(&["out.zip", "--", "-dash.txt"]), Some(vec![2]));
        for args in [
            vec!["-d", "out.zip", "member"],
            vec!["-ds", "10", "out.zip", "a"],
            vec!["-@", "out.zip"],
            vec!["out.zip"],
            vec!["-", "a.txt"],
            vec!["-rq", "out.zip", "a"],
        ] {
            assert!(indices(&args).is_none(), "{args:?}");
        }
    }

    #[test]
    fn extractors_and_wrappers_keep_outputs_and_selectors_apart() {
        let indices = |l: Layout| l.paths.iter().map(|p| p.index).collect::<Vec<_>>();
        assert_eq!(
            inputs(Family::Lzip, &["-d", "-n", "2", "a.lz"]),
            Some(vec![3])
        );
        assert_eq!(
            inputs(Family::Lzop, &["-x", "-p/tmp", "a.lzo"]),
            Some(vec![2])
        );
        assert_eq!(inputs(Family::Lzop, &["-o", "out", "a.lzo"]), Some(vec![2]));
        assert_eq!(inputs(Family::Pixz, &["-d", "a.pxz", "out"]), Some(vec![1]));
        assert_eq!(
            parse(
                Tool::Inputs(Family::Pixz),
                &["-d", "-i", "a.pxz", "-o", "out"]
            )
            .unwrap()
            .paths,
            vec![path(2, "", false)]
        );
        assert_eq!(
            inputs(Family::Cabextract, &["-d", "out", "a.cab", "b.cab"]),
            Some(vec![2, 3])
        );
        assert_eq!(
            inputs(Family::Dtrx, &["-p", "secret", "-o", "a.zip"]),
            Some(vec![3])
        );
        for (family, args) in [
            (Family::Pixz, vec!["-x", "member"]),
            (Family::Pixz, vec!["-i", "a.pxz", "b.pxz"]),
            (Family::Lzop, vec!["-r", "dir"]),
            (Family::Lzip, vec!["--bogus", "a.lz"]),
        ] {
            assert!(inputs(family, &args).is_none(), "{family:?} {args:?}");
        }
        let l = parse(Tool::Unar, &["-o", "out", "-p", "pw", "a.zip", "m", "-q"]).unwrap();
        assert_eq!((l.archive.index, l.members.clone()), (4, vec![5]));
        let l = parse(Tool::Lsar, &["a.zip", "--long", "m"]).unwrap();
        assert_eq!(l.members, vec![2]);
        for args in [vec!["-nr", "a.zip", "m"], vec!["-e", "cp437", "a.zip", "m"]] {
            let l = parse(Tool::Unar, &args).unwrap();
            assert!(l.members.is_empty() && !l.reads_members, "{args:?}");
        }
        assert!(parse(Tool::Unar, &["-unknown", "a.zip"]).is_none());
        assert_eq!(
            indices(parse(Tool::Ouch, &["d", "-d", "out", "a.zip", "b.7z"]).unwrap()),
            vec![3, 4]
        );
        assert_eq!(
            indices(parse(Tool::Ouch, &["-q", "compress", "a", "b", "new.zip"]).unwrap()),
            vec![2, 3]
        );
        assert_eq!(
            indices(parse(Tool::Ouch, &["list", "--format=zip", "a"]).unwrap()),
            vec![2]
        );
        for args in [
            vec!["compress", "new.zip"],
            vec!["decompres", "a.zip"],
            vec!["d", "-"],
        ] {
            assert!(parse(Tool::Ouch, &args).is_none(), "{args:?}");
        }
        let atool = |mode, args: &[&str]| parse(Tool::Atool(mode), args).map(indices);
        assert_eq!(
            atool(AtoolMode::Extract, &["a.zip", "member"]),
            Some(vec![0])
        );
        assert_eq!(
            atool(AtoolMode::Unknown, &["-x", "-e", "a.zip", "b.zip"]),
            Some(vec![2, 3])
        );
        assert_eq!(
            atool(AtoolMode::Add, &["-F", "zip", "new", "a", "b"]),
            Some(vec![3, 4])
        );
        assert_eq!(
            atool(AtoolMode::Diff, &["a.zip", "b.zip"]),
            Some(vec![0, 1])
        );
        assert_eq!(atool(AtoolMode::Repack, &["a.zip", "b.7z"]), Some(vec![0]));
        assert!(atool(AtoolMode::Unknown, &["a.zip"]).is_none());
        assert!(atool(AtoolMode::List, &["-x", "a.zip"]).is_none());
        assert!(atool(AtoolMode::Extract, &["--null"]).is_none());
        assert_eq!(
            indices(parse(Tool::Unsquashfs, &["-d", "out", "-f", "image.sqfs", "etc"]).unwrap()),
            vec![3]
        );
        assert!(parse(Tool::Unsquashfs, &["-ef", "list", "image.sqfs"]).is_none());
        assert_eq!(
            indices(parse(Tool::Mksquashfs, &["a", "b", "new.sqfs", "-comp", "xz"]).unwrap()),
            vec![0, 1]
        );
        assert!(parse(Tool::Mksquashfs, &["new.sqfs", "-noappend"]).is_none());
        let members = parse_lsar(
            br#"{"lsarContents":[{"XADFileName":"dir/a.txt"},{"XADFileName":"newline\nname"},{"XADFileName":"../escape"}]}"#,
            10,
        )
        .unwrap();
        assert_eq!(members, vec!["dir/a.txt"]);
        assert!(
            parse_lsar(
                br#"{"lsarContents":[{"XADFileName":"a"},{"XADFileName":"b"}]}"#,
                1
            )
            .is_err()
        );
        assert!(parse_lsar(b"not json", 10).is_err());
    }

    #[test]
    fn listing_names_decode_losslessly_and_reject_unsafe_selectors() {
        let members = parse_listing(Format::GnuTar, b"\"docs/report.txt\"\n\"spaces and apostrophes'.txt\"\n\"unicode-\\360\\235\\204\\236.txt\"\n\"newline\\nfile\"\n\"../outside\"\n", 10).unwrap();
        assert!(members.contains(&"unicode-𝄞.txt".into()));
        assert!(members.contains(&"spaces and apostrophes'.txt".into()));
        assert_eq!(members.len(), 3);
        assert!(contains(&members, "docs"));
        assert!(!contains(&members, "doc"));
        assert_eq!(
            parse_listing(Format::BsdTar, b"docs/report.txt\n", 10).unwrap(),
            vec!["docs/report.txt"]
        );
        for value in [
            "-option",
            "@list",
            "a*",
            "a?",
            "a[b]",
            "/etc/passwd",
            "../file",
            "a/../b",
            "a\\b",
            "a:b",
        ] {
            assert!(!literal_member(value), "{value}");
        }
        assert!(parse_listing(Format::GnuTar, b"\"a\\999\"\n", 10).is_err());
        assert!(parse_listing(Format::BsdTar, b"bad\\", 10).is_err());
        assert!(parse_listing(Format::Unzip, b"a\nb\nc\n", 2).is_err());
        assert!(parse_listing(Format::SevenZip, b"Path = a\nSize = 1\nmalformed\n", 10).is_err());
    }
}
