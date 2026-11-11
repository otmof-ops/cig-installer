// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2

//! Does the machine layer (#17) say anything about `carton`? The shim asks before it trusts its
//! own fast path: a carton the machine locks outranks a project's `.cig-carton`, and only the
//! installer's resolver knows how to apply a lock, so a machine that mentions `carton` sends
//! every run down the slow path. The question is cheap (one failed open on a machine with no
//! layer) and errs toward the slow path: unreadable, oversized or odd is "yes".
//!
//! The stores and their names are `cigi-home`'s `machine` module's; the constants live here so
//! the two cannot drift. Nothing here parses a store.

use std::path::Path;

/// The macOS preference domain (a bundle identifier).
pub const MACOS_DOMAIN: &str = "io.github.otmof-ops.cigscript";
/// Where a configuration profile's preferences land on macOS.
pub const MACOS_MANAGED_DIR: &str = "/Library/Managed Preferences";
/// The file store on Linux and the BSDs.
pub const FILE_PATH: &str = "/etc/cigscript/config.toml";
/// The Windows policy key under `HKEY_LOCAL_MACHINE`.
pub const REGISTRY_SUBKEY: &str = r"SOFTWARE\Policies\otmof-ops\CigScript";

/// The most of a store the shim reads: more than that is the installer's to read.
const PEEK: u64 = 256 * 1024;

/// Does this store's text contain the word `carton`? A byte search, so it holds for TOML, an
/// XML property list and a binary one alike; a false "yes" only costs the slow path.
pub fn bytes_mention_carton(bytes: &[u8]) -> bool {
    const W: &[u8] = b"carton";
    bytes.windows(W.len()).any(|w| w.eq_ignore_ascii_case(W))
}

/// Does the store file at `path` mention `carton`? Absent is no; anything else that is not a
/// clean read of a small file is yes.
pub fn file_mentions_carton(path: &Path) -> bool {
    use std::io::Read;
    let f = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return false,
        Err(_) => return true,
    };
    let mut buf = Vec::new();
    match f.take(PEEK + 1).read_to_end(&mut buf) {
        Ok(n) if (n as u64) <= PEEK => bytes_mention_carton(&buf),
        _ => true,
    }
}

/// The variable the `test-machine-root` feature reads (see `cigi-home`'s `machine` module).
#[cfg(feature = "test-machine-root")]
pub const TEST_ROOT_SEAM: &str = "CIGI_TEST_MACHINE_ROOT";

/// Does this system's machine layer mention `carton`?
pub fn machine_layer_mentions_carton() -> bool {
    #[cfg(feature = "test-machine-root")]
    if let Some(root) = std::env::var_os(TEST_ROOT_SEAM).filter(|v| !v.is_empty()) {
        return file_mentions_carton(&Path::new(&root).join("config.toml"));
    }
    system_mentions_carton()
}

#[cfg(windows)]
fn system_mentions_carton() -> bool {
    match cigi_sys::policy::read_values(REGISTRY_SUBKEY) {
        Ok(None) => false,
        Ok(Some(vals)) => vals.iter().any(|(n, _)| n.eq_ignore_ascii_case("carton")),
        Err(_) => true,
    }
}

#[cfg(target_os = "macos")]
fn system_mentions_carton() -> bool {
    file_mentions_carton(&Path::new(MACOS_MANAGED_DIR).join(format!("{MACOS_DOMAIN}.plist")))
}

#[cfg(not(any(windows, target_os = "macos")))]
fn system_mentions_carton() -> bool {
    file_mentions_carton(Path::new(FILE_PATH))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_word_is_found_in_text_and_in_a_binary_list() {
        assert!(bytes_mention_carton(b"carton = \"1.1\"\n"));
        assert!(bytes_mention_carton(b"locked = [\"Carton\"]"));
        assert!(bytes_mention_carton(b"bplist00\xd1\x01\x02Scarton\x08"));
        assert!(!bytes_mention_carton(
            b"mirror = \"https://m/\"\nreports = \"never\"\n"
        ));
        assert!(!bytes_mention_carton(b""));
    }

    #[test]
    fn a_missing_store_is_no_and_an_unreadable_or_big_one_is_yes() {
        let tmp = std::env::temp_dir().join(format!("cigi-shim-machine-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        assert!(!file_mentions_carton(&tmp.join("absent.toml")));
        let small = tmp.join("small.toml");
        std::fs::write(&small, "reports = \"never\"\n").unwrap();
        assert!(!file_mentions_carton(&small));
        std::fs::write(&small, "carton = \"1\"\n").unwrap();
        assert!(file_mentions_carton(&small));
        let big = tmp.join("big.toml");
        std::fs::write(&big, vec![b'#'; (PEEK as usize) + 10]).unwrap();
        assert!(file_mentions_carton(&big));
        // a directory cannot be read as a file: the installer decides
        assert!(file_mentions_carton(&tmp));
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
