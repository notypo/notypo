//! Identifying Go programs from the module information the Go toolchain
//! embeds in every binary (`go version -m` reads the same data): the main
//! package path names the app, and the dependency list names the completion
//! library it was built with. Two programs sharing a basename (Hetzner's and
//! Huawei's `hcloud`) have different main packages.

use super::super::cache::{CompletionCache, fingerprint};
use super::CompletionItem;
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
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
    // Not a completion library: it builds the command line of xcaddy's
    // generically named `caddy` modules (see GO_COMMAND_LIBRARIES).
    "github.com/caddyserver/caddy/v2",
    // Not completion libraries: mkcert's package init starts `brew` on
    // macOS, as a program or as bep/mclib (see STARTUP_HELPERS).
    "filippo.io/mkcert",
    "github.com/bep/mclib",
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
    // its forceposix build tag, v5 binaries found through their headers
    // beyond the scan's size limit, v6 the Caddy library, v7 mkcert).
    let mut cache = CompletionCache::open("go-modules", "v7");
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

/// Reads `binary`'s module information: from the build information section
/// the headers locate, at any file size (vault's executable exceeds
/// [`MAX_FILE`]), else by scanning the file in chunks. Toolchains before Go
/// 1.18 kept the information outside that section; the scan finds it.
fn scan(binary: &Path) -> Option<GoModule> {
    let mut file = File::open(binary).ok()?;
    if let Some((offset, size)) = section(&mut file)
        && file.seek(SeekFrom::Start(offset)).is_ok()
        && let Some(module) = scan_reader((&mut file).take(size.min(SECTION_LIMIT)))
    {
        return Some(module);
    }
    if file.metadata().ok()?.len() > MAX_FILE {
        return None;
    }
    file.seek(SeekFrom::Start(0)).ok()?;
    scan_reader(file.take(MAX_FILE))
}

/// How much of a build information section is read: Go 1.18+ stores the
/// version and module strings inline, right after the section's header.
const SECTION_LIMIT: u64 = MAX_INFO as u64 + 64 * 1024;
/// The most header or section-name data read to find that section.
const MAX_HEADERS: u64 = 4 * 1024 * 1024;

/// The file offset and size of the section `debug/buildinfo` reads: ELF's
/// `.go.buildinfo`, Mach-O's `__go_buildinfo` (in any slice of a universal
/// binary), or PE's first writable data section.
fn section(file: &mut File) -> Option<(u64, u64)> {
    let head = read_at(file, 0, 64)?;
    match head.get(..4)? {
        b"\x7fELF" => elf(file, &head),
        [0xca, 0xfe, 0xba, 0xbe | 0xbf] => {
            let wide = head[3] == 0xbf;
            let fields = Fields::new(&head, true);
            // Java class files share the magic; their version fields read
            // as an implausible slice count.
            let count = fields.u32(4)?;
            if count == 0 || count > 32 {
                return None;
            }
            let entry = if wide { 32 } else { 20 };
            let table = read_at(file, 8, count * entry)?;
            let table = Fields::new(&table, true);
            (0..count).find_map(|i| {
                let at = (i * entry) as usize;
                let offset = if wide {
                    table.u64(at + 8)?
                } else {
                    table.u32(at + 8)?
                };
                macho(file, offset)
            })
        }
        [0xcf | 0xce, 0xfa, 0xed, 0xfe] | [0xfe, 0xed, 0xfa, 0xcf | 0xce] => macho(file, 0),
        [b'M', b'Z', ..] => pe(file, &head),
        _ => None,
    }
}

fn macho(file: &mut File, base: u64) -> Option<(u64, u64)> {
    let head = read_at(file, base, 32)?;
    let (wide, big) = match head.get(..4)? {
        [0xcf, 0xfa, 0xed, 0xfe] => (true, false),
        [0xce, 0xfa, 0xed, 0xfe] => (false, false),
        [0xfe, 0xed, 0xfa, 0xcf] => (true, true),
        [0xfe, 0xed, 0xfa, 0xce] => (false, true),
        _ => return None,
    };
    let fields = Fields::new(&head, big);
    let count = fields.u32(16)?;
    let commands = read_at(file, base + if wide { 32 } else { 28 }, fields.u32(20)?)?;
    let fields = Fields::new(&commands, big);
    // LC_SEGMENT_64 or LC_SEGMENT, the offset of its section count, and
    // the size of its header and of each section entry.
    let (segment, sections_at, header, entry) = if wide {
        (0x19, 64, 72, 80)
    } else {
        (0x1, 48, 56, 68)
    };
    let mut at = 0usize;
    for _ in 0..count {
        let size = fields.u32(at + 4)? as usize;
        if size < 8 {
            return None;
        }
        if fields.u32(at)? == segment {
            for i in 0..fields.u32(at + sections_at)? as usize {
                let start = at.checked_add(header + i * entry)?;
                if commands.get(start..start + 16)? == b"__go_buildinfo\0\0" {
                    let (size, offset) = if wide {
                        (fields.u64(start + 40)?, fields.u32(start + 48)?)
                    } else {
                        (fields.u32(start + 36)?, fields.u32(start + 40)?)
                    };
                    return Some((base + offset, size));
                }
            }
        }
        at = at.checked_add(size)?;
    }
    None
}

fn elf(file: &mut File, head: &[u8]) -> Option<(u64, u64)> {
    let wide = match head[4] {
        1 => false,
        2 => true,
        _ => return None,
    };
    let fields = Fields::new(head, head[5] == 2);
    let (table_at, entry, count, names) = if wide {
        (
            fields.u64(0x28)?,
            fields.u16(0x3a)?,
            fields.u16(0x3c)?,
            fields.u16(0x3e)?,
        )
    } else {
        (
            fields.u32(0x20)?,
            fields.u16(0x2e)?,
            fields.u16(0x30)?,
            fields.u16(0x32)?,
        )
    };
    if entry < if wide { 64 } else { 40 } {
        return None;
    }
    let table = read_at(file, table_at, entry * count)?;
    let table = Fields::new(&table, head[5] == 2);
    // A section's name offset, file offset, and size.
    let header = |i: u64| -> Option<(u64, u64, u64)> {
        let at = (i * entry) as usize;
        Some(if wide {
            (table.u32(at)?, table.u64(at + 24)?, table.u64(at + 32)?)
        } else {
            (table.u32(at)?, table.u32(at + 16)?, table.u32(at + 20)?)
        })
    };
    let (_, names_at, names_size) = header(names)?;
    let names = read_at(file, names_at, names_size)?;
    (0..count).find_map(|i| {
        let (name, offset, size) = header(i)?;
        names
            .get(name as usize..)?
            .starts_with(b".go.buildinfo\0")
            .then_some((offset, size))
    })
}

fn pe(file: &mut File, head: &[u8]) -> Option<(u64, u64)> {
    let signature = Fields::new(head, false).u32(0x3c)?;
    let coff = read_at(file, signature, 24)?;
    if coff.get(..4)? != b"PE\0\0" {
        return None;
    }
    let coff = Fields::new(&coff, false);
    let count = coff.u16(6)?;
    let table = read_at(file, signature + 24 + coff.u16(20)?, count * 40)?;
    let table = Fields::new(&table, false);
    // Initialized, readable, writable data, whatever its alignment.
    const DATA: u64 = 0x40 | 0x4000_0000 | 0x8000_0000;
    (0..count as usize).find_map(|i| {
        let at = i * 40;
        let flags = table.u32(at + 36)? & !0x60_0000;
        let size = table.u32(at + 16)?;
        (table.u32(at + 12)? != 0 && size != 0 && flags == DATA)
            .then_some((table.u32(at + 20)?, size))
    })
}

fn read_at(file: &mut File, offset: u64, len: u64) -> Option<Vec<u8>> {
    if len > MAX_HEADERS {
        return None;
    }
    file.seek(SeekFrom::Start(offset)).ok()?;
    let mut data = vec![0; len as usize];
    file.read_exact(&mut data).ok()?;
    Some(data)
}

/// Fixed-width integers at offsets of a header, in its byte order.
struct Fields<'a> {
    bytes: &'a [u8],
    big: bool,
}

impl<'a> Fields<'a> {
    fn new(bytes: &'a [u8], big: bool) -> Self {
        Fields { bytes, big }
    }

    fn int<const N: usize>(&self, at: usize) -> Option<u64> {
        let bytes: [u8; N] = self.bytes.get(at..at.checked_add(N)?)?.try_into().ok()?;
        let fold = |value: u64, byte: &u8| value << 8 | u64::from(*byte);
        Some(if self.big {
            bytes.iter().fold(0, fold)
        } else {
            bytes.iter().rev().fold(0, fold)
        })
    }

    fn u16(&self, at: usize) -> Option<u64> {
        self.int::<2>(at)
    }

    fn u32(&self, at: usize) -> Option<u64> {
        self.int::<4>(at)
    }

    fn u64(&self, at: usize) -> Option<u64> {
        self.int::<8>(at)
    }
}

/// Reads until the module information is found.
fn scan_reader(mut reader: impl Read) -> Option<GoModule> {
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

    /// Writes `value`'s low `width` bytes at `at`, growing `data` as needed.
    fn put(data: &mut Vec<u8>, at: usize, width: usize, value: u64, big: bool) {
        if data.len() < at + width {
            data.resize(at + width, 0);
        }
        for i in 0..width {
            let shift = 8 * if big { width - 1 - i } else { i };
            data[at + i] = (value >> shift) as u8;
        }
    }

    fn info(path: &str) -> Vec<u8> {
        let mut info = START.to_vec();
        info.extend_from_slice(
            format!("path\t{path}\nmod\t{path}\t(devel)\t\ndep\tgithub.com/posener/complete\tv1.2.3\th1:x\n").as_bytes(),
        );
        info.extend_from_slice(END);
        info
    }

    /// A 64-bit Mach-O header whose one segment holds `__go_buildinfo`.
    fn macho64(offset: u64, size: u64) -> Vec<u8> {
        let mut data = vec![0xcf, 0xfa, 0xed, 0xfe];
        data.resize(32 + 16 + 72 + 80, 0);
        put(&mut data, 4, 4, 0x0100_000c, false);
        put(&mut data, 12, 4, 2, false);
        put(&mut data, 16, 4, 2, false);
        put(&mut data, 20, 4, 16 + 72 + 80, false);
        // A command of another kind comes first.
        put(&mut data, 32, 4, 0x32, false);
        put(&mut data, 36, 4, 16, false);
        let segment = 48;
        put(&mut data, segment, 4, 0x19, false);
        put(&mut data, segment + 4, 4, 72 + 80, false);
        data[segment + 8..segment + 14].copy_from_slice(b"__DATA");
        put(&mut data, segment + 64, 4, 1, false);
        let section = segment + 72;
        data[section..section + 14].copy_from_slice(b"__go_buildinfo");
        data[section + 16..section + 22].copy_from_slice(b"__DATA");
        put(&mut data, section + 40, 8, size, false);
        put(&mut data, section + 48, 4, offset, false);
        data
    }

    /// An ELF header with `.go.buildinfo` as its third section.
    fn elf(wide: bool, big: bool, offset: u64, size: u64) -> Vec<u8> {
        let mut data = vec![0x7f, b'E', b'L', b'F', if wide { 2 } else { 1 }];
        data.push(if big { 2 } else { 1 });
        let names = b"\0.shstrtab\0.go.buildinfo\0";
        let (names_at, table_at) = (0x100u64, 0x200u64);
        data.resize(names_at as usize, 0);
        data.extend_from_slice(names);
        let entry = if wide { 64 } else { 40 };
        if wide {
            put(&mut data, 0x28, 8, table_at, big);
            put(&mut data, 0x3a, 2, entry, big);
            put(&mut data, 0x3c, 2, 3, big);
            put(&mut data, 0x3e, 2, 1, big);
        } else {
            put(&mut data, 0x20, 4, table_at, big);
            put(&mut data, 0x2e, 2, entry, big);
            put(&mut data, 0x30, 2, 3, big);
            put(&mut data, 0x32, 2, 1, big);
        }
        for (i, (name, at, len)) in [(1, names_at, names.len() as u64), (11, offset, size)]
            .into_iter()
            .enumerate()
        {
            let header = (table_at + (i as u64 + 1) * entry) as usize;
            put(&mut data, header, 4, name, big);
            if wide {
                put(&mut data, header + 24, 8, at, big);
                put(&mut data, header + 32, 8, len, big);
            } else {
                put(&mut data, header + 16, 4, at, big);
                put(&mut data, header + 20, 4, len, big);
            }
        }
        data
    }

    /// A PE header with a code section, then a writable data section.
    fn pe(offset: u64, size: u64) -> Vec<u8> {
        let mut data = b"MZ".to_vec();
        put(&mut data, 0x3c, 4, 0x80, false);
        data.resize(0x80, 0);
        data.extend_from_slice(b"PE\0\0");
        put(&mut data, 0x84, 2, 0x8664, false);
        put(&mut data, 0x86, 2, 2, false);
        put(&mut data, 0x94, 2, 0xf0, false);
        let table = 0x80 + 24 + 0xf0;
        for (i, flags, at) in [(0, 0x6000_0020u64, 0x400u64), (1, 0xc060_0040, offset)] {
            let header = table + i * 40;
            put(&mut data, header + 12, 4, 0x1000 * (i as u64 + 1), false);
            put(&mut data, header + 16, 4, size, false);
            put(&mut data, header + 20, 4, at, false);
            put(&mut data, header + 36, 4, flags, false);
        }
        data
    }

    #[test]
    fn executable_headers_locate_the_build_information_section() {
        let dir = std::env::temp_dir().join(format!("notypo-go-headers-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let binary = dir.join("tool");
        let real = info("github.com/hashicorp/vault");
        let size = real.len() as u64 + 32;
        let at = 0x4000;
        // A sequential scan would take the earlier record; `go version -m`
        // and notypo read the section the headers name.
        let with = |mut data: Vec<u8>, at: usize| {
            data.resize(data.len().max(0x1000), 0);
            data.extend_from_slice(&info("example.com/decoy"));
            data.resize(at + 32, 0);
            data.extend_from_slice(&real);
            data.extend_from_slice(&[0; 64]);
            data
        };
        let mut fat = vec![0xca, 0xfe, 0xba, 0xbe];
        put(&mut fat, 4, 4, 2, true);
        // The first slice has no Go section; the second one does.
        put(&mut fat, 8 + 8, 4, 0x200, true);
        put(&mut fat, 28 + 8, 4, 0x2000, true);
        fat.resize(0x200, 0);
        fat.extend_from_slice(&[0xcf, 0xfa, 0xed, 0xfe]);
        fat.resize(0x2000, 0);
        fat.extend_from_slice(&macho64(at as u64 - 0x2000, size));
        for data in [
            macho64(at as u64, size),
            fat,
            elf(true, false, at as u64, size),
            elf(false, true, at as u64, size),
            pe(at as u64, size),
        ] {
            std::fs::write(&binary, with(data, at)).unwrap();
            let module = scan(&binary).unwrap();
            assert_eq!(module.path, "github.com/hashicorp/vault");
            assert!(module.uses("github.com/posener/complete"));
        }
        // Without such a section (or with one past the end of the file),
        // the bounded scan still finds the first record.
        let mut truncated = macho64(0xffff_0000, size);
        truncated.resize(0x1000, 0);
        truncated.extend_from_slice(&real);
        std::fs::write(&binary, &truncated).unwrap();
        assert_eq!(scan(&binary).unwrap().path, "github.com/hashicorp/vault");
        // Malformed headers cannot point a read anywhere: oversized tables
        // and a zero-sized load command are rejected.
        let mut looping = macho64(at as u64, size);
        put(&mut looping, 36, 4, 0, false);
        assert_eq!(
            section(&mut write_open(&binary, &looping)),
            None,
            "a zero command size would never advance"
        );
        let mut huge = elf(true, false, at as u64, size);
        put(&mut huge, 0x3c, 2, 0xffff, false);
        put(&mut huge, 0x3a, 2, 0xffff, false);
        assert_eq!(section(&mut write_open(&binary, &huge)), None);
        assert_eq!(
            section(&mut write_open(
                &binary,
                &[0xca, 0xfe, 0xba, 0xbe, 0, 0, 0, 52]
            )),
            None,
            "a Java class file is not a universal binary"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn write_open(path: &Path, data: &[u8]) -> File {
        let mut data = data.to_vec();
        data.resize(data.len().max(64), 0);
        std::fs::write(path, &data).unwrap();
        File::open(path).unwrap()
    }

    /// vault 2.1.1's macOS executable is larger than the scan's limit.
    #[cfg(unix)]
    #[test]
    fn executables_beyond_the_scan_limit_are_identified_through_their_headers() {
        let dir = std::env::temp_dir().join(format!("notypo-go-large-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let binary = dir.join("vault");
        let real = info("github.com/hashicorp/vault");
        let at = MAX_FILE - 4096;
        {
            use std::io::Write;
            let mut file = File::create(&binary).unwrap();
            // Sparse: the gap between the headers and the section is a hole.
            file.write_all(&macho64(at, real.len() as u64 + 32))
                .unwrap();
            file.seek(SeekFrom::Start(at + 32)).unwrap();
            file.write_all(&real).unwrap();
            file.set_len(MAX_FILE + 4096).unwrap();
        }
        assert_eq!(scan(&binary).unwrap().path, "github.com/hashicorp/vault");
        // A file that large without the section is still not read through.
        std::fs::write(&binary, b"").unwrap();
        File::options()
            .write(true)
            .open(&binary)
            .unwrap()
            .set_len(MAX_FILE + 1)
            .unwrap();
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
