//! Identifying Go programs from the module information the Go toolchain
//! embeds in every binary (`go version -m` reads the same data): the main
//! package path names the app, and the dependency list names the completion
//! library it was built with. Two programs sharing a basename (Hetzner's and
//! Huawei's `hcloud`) have different main packages.

use super::super::cache::{CompletionCache, fingerprint};
use super::CompletionItem;
use std::fs::File;
use std::io::Read;
use std::path::Path;

/// Delimiters around the module information (from `runtime/debug`).
const START: &[u8] = b"\x30\x77\xaf\x0c\x92\x74\x08\x02\x41\xe1\xc1\x07\xe6\xd6\x18\xe6";
const END: &[u8] = b"\xf9\x32\x43\x31\x86\x18\x20\x72\x00\x82\x42\x10\x41\x16\xd8\xf2";
const MAX_INFO: usize = 512 * 1024;
const MAX_FILE: u64 = 512 * 1024 * 1024;

/// The completion libraries notypo can talk to.
const LIBRARIES: &[&str] = &["github.com/spf13/cobra", "github.com/posener/complete"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GoModule {
    /// The main package, such as `k8s.io/kubernetes/cmd/kubectl`.
    pub path: String,
    /// Dependencies among [`LIBRARIES`].
    pub libraries: Vec<String>,
}

impl GoModule {
    pub fn uses(&self, library: &str) -> bool {
        self.libraries.iter().any(|l| l == library)
    }
}

/// The module information of `binary`, cached by the file's fingerprint
/// (binaries are large; the scan runs once per installed version).
pub fn module(binary: &Path) -> Option<GoModule> {
    let key = fingerprint(&[binary.to_owned()], &["go-module"]);
    let mut cache = CompletionCache::open("go-modules", "v1");
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
                .map(|value| CompletionItem {
                    value,
                    takes_value: None,
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
    Some(GoModule { path, libraries })
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
    for line in info.lines() {
        let mut fields = line.split('\t');
        match (fields.next(), fields.next()) {
            (Some("path"), Some(p)) => path = Some(p.to_owned()),
            (Some("dep"), Some(d)) if LIBRARIES.contains(&d) => libraries.push(d.to_owned()),
            _ => {}
        }
    }
    Some(GoModule {
        path: path?,
        libraries,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(decode(&encode(Some(&module))), Some(module));
        std::fs::write(&binary, b"#!/bin/sh\necho not go\n").unwrap();
        assert_eq!(scan(&binary), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
