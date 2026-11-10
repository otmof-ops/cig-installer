// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! The same job through `tough`: verify signed metadata (roles, thresholds, expiry, version
//! floor) and then a target against it. The size added over `baseline` is the cost of TUF.

use sha2::{Digest, Sha256};
use tough::{IntoVec, RepositoryLoader, TargetName};
use tough_eval::ureq_transport::UreqTransport;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 6 {
        eprintln!(
            "usage: tuf-client <root.json> <metadata-url> <targets-url> <datastore-dir> <target>"
        );
        std::process::exit(2);
    }
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let out = rt.block_on(async {
        let root = std::fs::read(&args[1]).map_err(|e| e.to_string())?;
        let url = |s: &str| url::Url::parse(s).map_err(|e| e.to_string());
        let repo = RepositoryLoader::new(&root, url(&args[2])?, url(&args[3])?)
            .transport(UreqTransport::new(16 << 20))
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
        let bytes = stream.into_vec().await.map_err(|e| e.to_string())?;
        Ok::<_, String>(hex::encode(Sha256::digest(&bytes)))
    });
    match out {
        Ok(d) => println!("ok {d}"),
        Err(e) => {
            eprintln!("refused: {e}");
            std::process::exit(1);
        }
    }
}
