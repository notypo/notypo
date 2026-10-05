//! Identifying Go programs from the module information the Go toolchain
//! embeds in every binary (`go version -m` reads the same data): the main
//! package path names the app, and the dependency list names the completion
//! library it was built with. Two programs sharing a basename (Hetzner's and
//! Huawei's `hcloud`) have different main packages.

use super::super::cache::{CompletionCache, fingerprint};
use super::CompletionItem;
use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;
use std::path::Path;

/// Delimiters around the module information (from `runtime/debug`).
const START: &[u8] = b"\x30\x77\xaf\x0c\x92\x74\x08\x02\x41\xe1\xc1\x07\xe6\xd6\x18\xe6";
const END: &[u8] = b"\xf9\x32\x43\x31\x86\x18\x20\x72\x00\x82\x42\x10\x41\x16\xd8\xf2";
const MAX_INFO: usize = 512 * 1024;
const MAX_FILE: u64 = 512 * 1024 * 1024;

/// The completion libraries notypo can talk to.
const LIBRARIES: &[&str] = &[
    "github.com/spf13/cobra",
    "github.com/posener/complete",
    "github.com/urfave/cli",
    "github.com/urfave/cli/v2",
    "github.com/urfave/cli/v3",
    "gopkg.in/alecthomas/kingpin.v2",
    "github.com/alecthomas/kingpin/v2",
    "github.com/choria-io/fisk",
    "github.com/jessevdk/go-flags",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GoModule {
    /// The main package, such as `k8s.io/kubernetes/cmd/kubectl`.
    pub path: String,
    /// Dependencies among [`LIBRARIES`].
    pub libraries: Vec<String>,
    /// Each library's linked release. A replaced module has none: the
    /// substitute's behavior is not that release's.
    pub versions: BTreeMap<String, String>,
    /// go-flags uses POSIX option syntax on Windows with this build tag.
    pub force_posix: bool,
}

impl GoModule {
    pub fn uses(&self, library: &str) -> bool {
        self.libraries.iter().any(|l| l == library)
    }

    pub fn version(&self, library: &str) -> Option<&str> {
        self.versions.get(library).map(String::as_str)
    }
}

/// The module information of `binary`, cached by the file's fingerprint
/// (binaries are large; the scan runs once per installed version).
pub fn module(binary: &Path) -> Option<GoModule> {
    let key = fingerprint(&[binary.to_owned()], &["go-module"]);
    // Bump with LIBRARIES: older entries omit libraries added since (v2
    // added urfave/cli and versions, v3 kingpin and fisk, v4 go-flags and
    // its forceposix build tag).
    let mut cache = CompletionCache::open("go-modules", "v4");
    if let Some(hit) = cache.as_ref().and_then(|c| c.get(&key)) {
        return decode(hit);
    }
    let found = scan(binary);
    if let Some(cache) = cache.as_mut() {
        cache.put(key, encode(found.as_ref()));
        cache.save();
    }
    found
}

fn encode(module: Option<&GoModule>) -> Vec<CompletionItem> {
    module
        .map(|m| {
            std::iter::once(format!("path\t{}", m.path))
                .chain(m.libraries.iter().map(|l| format!("dep\t{l}")))
                .chain(m.versions.iter().map(|(l, v)| format!("ver\t{l}\t{v}")))
                .chain(m.force_posix.then(|| "tag\tforceposix".to_owned()))
                .map(|value| CompletionItem {
                    value,
                    takes_value: None,
                    description: None,
                })
                .collect()
        })
        .unwrap_or_default()
}

fn decode(items: &[CompletionItem]) -> Option<GoModule> {
    let path = items
        .iter()
        .find_map(|i| i.value.strip_prefix("path\t"))?
        .to_owned();
    let libraries = items
        .iter()
        .filter_map(|i| i.value.strip_prefix("dep\t"))
        .map(str::to_owned)
        .collect();
    let versions = items
        .iter()
        .filter_map(|i| i.value.strip_prefix("ver\t")?.split_once('\t'))
        .map(|(l, v)| (l.to_owned(), v.to_owned()))
        .collect();
    Some(GoModule {
        path,
        libraries,
        versions,
        force_posix: items.iter().any(|item| item.value == "tag\tforceposix"),
    })
}

/// Reads `binary` in chunks until the module information is found.
fn scan(binary: &Path) -> Option<GoModule> {
    let file = File::open(binary).ok()?;
    if file.metadata().ok()?.len() > MAX_FILE {
        return None;
    }
    let mut reader = file.take(MAX_FILE);
    let start = regex::bytes::Regex::new(&format!("(?-u){}", escape(START))).ok()?;
    let mut window: Vec<u8> = Vec::new();
    let mut chunk = vec![0u8; 8 * 1024 * 1024];
    loop {
        let n = reader.read(&mut chunk).ok()?;
        if n == 0 {
            return None;
        }
        window.extend_from_slice(&chunk[..n]);
        if let Some(m) = start.find(&window) {
            let mut info = window[m.end()..].to_vec();
            while !contains(&info, END) && info.len() < MAX_INFO {
                let n = reader.read(&mut chunk).ok()?;
                if n == 0 {
                    break;
                }
                info.extend_from_slice(&chunk[..n]);
            }
            let end = find(&info, END)?;
            return parse(&String::from_utf8_lossy(&info[..end]));
        }
        // Keep a tail in case the marker spans two chunks.
        let keep = window.len().saturating_sub(START.len());
        window.drain(..keep);
    }
}

fn escape(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("\\x{b:02x}")).collect()
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    find(haystack, needle).is_some()
}

fn parse(info: &str) -> Option<GoModule> {
    let mut path = None;
    let mut libraries = Vec::new();
    let mut versions = BTreeMap::new();
    let mut previous: Option<&str> = None;
    let mut force_posix = false;
    for line in info.lines() {
        let mut fields = line.split('\t');
        match (fields.next(), fields.next(), fields.next()) {
            (Some("path"), Some(p), _) => path = Some(p.to_owned()),
            (Some("dep"), Some(d), version) if LIBRARIES.contains(&d) => {
                libraries.push(d.to_owned());
                if let Some(version) = version.filter(|v| !v.is_empty()) {
                    versions.insert(d.to_owned(), version.to_owned());
                }
                previous = Some(d);
                continue;
            }
            // `=>` follows a dependency that a replace directive substituted.
            (Some("=>"), ..) => {
                if let Some(replaced) = previous {
                    versions.remove(replaced);
                }
            }
            (Some("build"), Some(setting), _) => {
                if let Some(tags) = setting.strip_prefix("-tags=") {
                    force_posix = tags
                        .trim_matches('"')
                        .split([',', ' '])
                        .any(|tag| tag == "forceposix");
                }
            }
            _ => {}
        }
        previous = None;
    }
    Some(GoModule {
        path: path?,
        libraries,
        versions,
        force_posix,
    })
}

/// A file carrying Go module information, for tests of identification.
#[cfg(all(test, unix))]
pub(crate) fn fake_binary(path: &str, dependencies: &[&str]) -> Vec<u8> {
    let lines: String = dependencies
        .iter()
        .map(|dependency| format!("dep\t{dependency}\tv1.0.0\th1:x\n"))
        .collect();
    fake_binary_info(path, &lines)
}

/// Module information with raw `dep`/`=>` lines after the main module.
#[cfg(all(test, unix))]
pub(crate) fn fake_binary_info(path: &str, lines: &str) -> Vec<u8> {
    let mut data = vec![0u8; 64];
    data.extend_from_slice(START);
    data.extend_from_slice(format!("path\t{path}\nmod\t{path}\t(devel)\t\n{lines}").as_bytes());
    data.extend_from_slice(END);
    data
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn go_flags_dependency_and_forceposix_build_tag_survive_the_identity_cache() {
        for (tags, expected) in [
            ("forceposix", true),
            ("other,forceposix", true),
            ("forceposix_extra", false),
        ] {
            let module = parse(&format!("path\texample.com/fixture\ndep\tgithub.com/jessevdk/go-flags\tv1.6.1\th1:x\nbuild\t-tags={tags}\n")).unwrap();
            assert!(module.uses("github.com/jessevdk/go-flags"));
            assert_eq!(module.force_posix, expected);
            assert_eq!(decode(&encode(Some(&module))), Some(module));
        }
    }

    #[test]
    fn finds_module_information_across_chunk_boundaries() {
        let dir = std::env::temp_dir().join(format!("notypo-go-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let binary = dir.join("tool");
        let mut data = vec![0u8; 8 * 1024 * 1024 - 5];
        data.extend_from_slice(START);
        data.extend_from_slice(
            b"path\tgithub.com/hetznercloud/cli/cmd/hcloud\nmod\tgithub.com/hetznercloud/cli\t(devel)\t\ndep\tgithub.com/spf13/cobra\tv1.9.1\th1:x\ndep\tgolang.org/x/net\tv0.1.0\th1:y\n",
        );
        data.extend_from_slice(END);
        data.extend_from_slice(&[0u8; 1000]);
        std::fs::write(&binary, &data).unwrap();
        let module = scan(&binary).unwrap();
        assert_eq!(module.path, "github.com/hetznercloud/cli/cmd/hcloud");
        assert!(module.uses("github.com/spf13/cobra"));
        assert!(
            !module.uses("golang.org/x/net"),
            "only completion libraries are kept"
        );
        assert_eq!(module.version("github.com/spf13/cobra"), Some("v1.9.1"));
        assert_eq!(decode(&encode(Some(&module))), Some(module));
        std::fs::write(&binary, b"#!/bin/sh\necho not go\n").unwrap();
        assert_eq!(scan(&binary), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn replaced_libraries_keep_their_path_but_lose_the_release_version() {
        let module = parse(
            "path\tgithub.com/cloudflare/cloudflared/cmd/cloudflared\n\
             dep\tgithub.com/urfave/cli/v2\tv2.3.0\n\
             =>\tgithub.com/ipostelnik/cli/v2\tv2.3.1-0.20210324024421-b6ea8234fe3d\th1:x\n\
             dep\tgithub.com/urfave/cli/v3\tv3.14.0\th1:y\n",
        )
        .unwrap();
        assert!(module.uses("github.com/urfave/cli/v2"));
        assert_eq!(module.version("github.com/urfave/cli/v2"), None);
        assert_eq!(module.version("github.com/urfave/cli/v3"), Some("v3.14.0"));
        assert_eq!(decode(&encode(Some(&module))), Some(module));
    }
}
