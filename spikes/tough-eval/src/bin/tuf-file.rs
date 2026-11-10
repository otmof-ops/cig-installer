// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! `tough` alone, reading a mirror from a directory: no `ureq`, no rustls of ours. It exists to
//! show how much of `tuf-client`'s size is `tough` and its crypto, and how much is the HTTP client
//! both `baseline` and `tuf-client` already carry.

use tough::{IntoVec, RepositoryLoader, TargetName};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 6 {
        eprintln!(
            "usage: tuf-file <root.json> <metadata-dir> <targets-dir> <datastore-dir> <target>"
        );
        std::process::exit(2);
    }
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let out = rt.block_on(async {
        let root = std::fs::read(&args[1]).map_err(|e| e.to_string())?;
        let dir =
            |s: &str| url::Url::from_directory_path(s).map_err(|()| "not absolute".to_string());
        let repo = RepositoryLoader::new(&root, dir(&args[2])?, dir(&args[3])?)
            .datastore(&args[4])
            .load()
            .await
            .map_err(|e| e.to_string())?;
        let name = TargetName::new(&args[5]).map_err(|e| e.to_string())?;
        let stream = repo
            .read_target(&name)
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "not listed in signed targets metadata".to_string())?;
        stream.into_vec().await.map_err(|e| e.to_string())
    });
    match out {
        Ok(b) => println!("ok {} bytes", b.len()),
        Err(e) => {
            eprintln!("refused: {e}");
            std::process::exit(1);
        }
    }
}
