//! The list of executables in `$PATH` (the bins half of
//! `get_all_executables`), cached on disk.
//!
//! Listing every PATH directory costs ~1 ms (≈2000 entries on a typical
//! macOS box), so the result is cached and validated by each directory's
//! (dev, inode, mtime): a warm lookup is one `stat` per directory plus one
//! small file read. Adding or removing a binary changes its directory's
//! mtime, which invalidates the cache.

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::{env, fs, thread};

/// One PATH directory and the non-directory entries in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirListing {
    pub dir: String,
    pub names: Vec<String>,
}

/// `$PATH` split and de-duplicated; empty entries mean the current directory.
pub fn path_dirs() -> Vec<String> {
    let path = env::var_os("PATH").unwrap_or_default();
    let mut dirs: Vec<String> = Vec::new();
    for dir in env::split_paths(&path) {
        let dir = if dir.as_os_str().is_empty() {
            ".".into()
        } else {
            dir.to_string_lossy().into_owned()
        };
        if !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    dirs
}

/// Listings for every PATH directory, from the cache when it's still valid.
pub fn load() -> Vec<DirListing> {
    let path = env::var("PATH").unwrap_or_default();
    let dirs = path_dirs();
    let sigs: Vec<Option<DirSig>> = dirs.iter().map(|d| DirSig::of(Path::new(d))).collect();
    let cache = cache_file(&path);
    if let Some(cache) = &cache
        && let Some(listings) = read_cache(cache, &path, &dirs, &sigs)
    {
        return listings;
    }
    let listings = scan(&dirs, &sigs);
    if let Some(cache) = &cache {
        write_cache(cache, &path, &sigs, &listings);
    }
    listings
}

/// What invalidates a directory's cached listing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DirSig {
    dev: u64,
    ino: u64,
    mtime: i64,
    mtime_nsec: i64,
    /// Not part of the key: a scheduling hint for parallel scans.
    size: u64,
}

impl DirSig {
    fn of(dir: &Path) -> Option<DirSig> {
        let m = fs::metadata(dir).ok().filter(|m| m.is_dir())?;
        #[cfg(unix)]
        {
            Some(DirSig {
                dev: m.dev(),
                ino: m.ino(),
                mtime: m.mtime(),
                mtime_nsec: m.mtime_nsec(),
                size: m.size(),
            })
        }
        #[cfg(windows)]
        {
            let modified = m
                .modified()
                .ok()?
                .duration_since(std::time::UNIX_EPOCH)
                .ok()?;
            Some(DirSig {
                dev: 0,
                ino: 0,
                mtime: modified.as_secs() as i64,
                mtime_nsec: i64::from(modified.subsec_nanos()),
                size: m.len(),
            })
        }
    }

    fn key(sig: Option<DirSig>) -> String {
        match sig {
            Some(s) => format!("{} {} {} {}", s.dev, s.ino, s.mtime, s.mtime_nsec),
            None => "-".to_owned(),
        }
    }
}

/// Non-directory entries of `dir`. `d_type` answers for everything but
/// symlinks, which are followed (a broken link counts as a file, like
/// Python's `Path.is_dir()` returning False).
fn list_dir(dir: &str) -> Vec<String> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let names = entries
        .filter_map(Result::ok)
        .filter(|entry| match entry.file_type() {
            Ok(t) if t.is_dir() => false,
            Ok(t) if t.is_symlink() => !fs::metadata(entry.path()).is_ok_and(|m| m.is_dir()),
            Ok(_) => true,
            Err(_) => false,
        })
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| !name.contains('\n'));
    #[cfg(windows)]
    let names = names.filter_map(|name| crate::utils::windows_executable_name(&name));
    names.collect()
}

/// Lists all directories, the biggest first, on a few threads.
fn scan(dirs: &[String], sigs: &[Option<DirSig>]) -> Vec<DirListing> {
    let mut order: Vec<usize> = (0..dirs.len()).filter(|&i| sigs[i].is_some()).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(sigs[i].map_or(0, |s| s.size)));
    let threads = order.len().clamp(1, 3);
    let chunks: Vec<Vec<usize>> = (0..threads)
        .map(|t| order.iter().copied().skip(t).step_by(threads).collect())
        .collect();
    let mut names: Vec<Vec<String>> = vec![Vec::new(); dirs.len()];
    thread::scope(|s| {
        let handles: Vec<_> = chunks
            .iter()
            .map(|chunk| {
                s.spawn(move || {
                    chunk
                        .iter()
                        .map(|&i| (i, list_dir(&dirs[i])))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        for handle in handles {
            for (i, listing) in handle.join().unwrap_or_default() {
                names[i] = listing;
            }
        }
    });
    dirs.iter()
        .zip(names)
        .map(|(dir, names)| DirListing {
            dir: dir.clone(),
            names,
        })
        .collect()
}

fn cache_file(path: &str) -> Option<PathBuf> {
    if env::var_os("NOTYPO_NO_CACHE").is_some() {
        return None;
    }
    let base = match env::var("XDG_CACHE_HOME") {
        Ok(d) if !d.is_empty() => PathBuf::from(d),
        _ => crate::utils::expand_user("~/.cache"),
    };
    // FNV-1a, so different PATHs (e.g. per-project shells) get their own file.
    let hash = path.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    });
    Some(base.join("notypo").join(format!("path-{hash:016x}.idx")))
}

const MAGIC: &str = "notypo-path-index v1";

/// Cache format: magic, PATH, one signature line per directory, then
/// `<dir index>\t<name>` lines.
fn read_cache(
    file: &Path,
    path: &str,
    dirs: &[String],
    sigs: &[Option<DirSig>],
) -> Option<Vec<DirListing>> {
    let text = fs::read_to_string(file).ok()?;
    let mut lines = text.lines();
    if lines.next()? != MAGIC || lines.next()? != path {
        return None;
    }
    for sig in sigs {
        if lines.next()? != DirSig::key(*sig) {
            return None;
        }
    }
    let mut listings: Vec<DirListing> = dirs
        .iter()
        .map(|d| DirListing {
            dir: d.clone(),
            names: Vec::new(),
        })
        .collect();
    for line in lines {
        let (idx, name) = line.split_once('\t')?;
        listings
            .get_mut(idx.parse::<usize>().ok()?)?
            .names
            .push(name.to_owned());
    }
    Some(listings)
}

fn write_cache(file: &Path, path: &str, sigs: &[Option<DirSig>], listings: &[DirListing]) {
    let mut text = format!("{MAGIC}\n{path}\n");
    for sig in sigs {
        text += &DirSig::key(*sig);
        text.push('\n');
    }
    for (i, listing) in listings.iter().enumerate() {
        for name in &listing.names {
            text += &format!("{i}\t{name}\n");
        }
    }
    let Some(dir) = file.parent() else { return };
    let _ = fs::create_dir_all(dir);
    let tmp = file.with_extension(format!("tmp{}", std::process::id()));
    if fs::write(&tmp, text).is_ok() && fs::rename(&tmp, file).is_err() {
        let _ = fs::remove_file(&tmp);
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    #[cfg(unix)]
    fn scan_and_cache_roundtrip() {
        let dirs = vec![
            "/bin".to_owned(),
            "/usr/bin".to_owned(),
            "/nonexistent-dir-xyz".to_owned(),
        ];
        let sigs: Vec<_> = dirs.iter().map(|d| DirSig::of(Path::new(d))).collect();
        let listings = scan(&dirs, &sigs);
        assert!(listings[0].names.iter().any(|n| n == "sh"));
        assert!(listings[2].names.is_empty());

        let file = env::temp_dir().join(format!("notypo-test-{}.idx", std::process::id()));
        write_cache(&file, "P", &sigs, &listings);
        assert_eq!(read_cache(&file, "P", &dirs, &sigs), Some(listings));
        assert_eq!(
            read_cache(&file, "Q", &dirs, &sigs),
            None,
            "different PATH must miss"
        );
        let mut stale = sigs.clone();
        stale[0].as_mut().unwrap().mtime_nsec += 1;
        assert_eq!(
            read_cache(&file, "P", &dirs, &stale),
            None,
            "changed directory must miss"
        );
        let _ = fs::remove_file(file);
    }
}
