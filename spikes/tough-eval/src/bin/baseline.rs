// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! What the installer's trust does today, at its smallest: GET a file over HTTPS and compare its
//! SHA-256 to a digest given on the command line. This is the size baseline for `tuf-client`.

use sha2::{Digest, Sha256};
use std::io::Read;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 {
        eprintln!("usage: baseline <url> <sha256>");
        std::process::exit(2);
    }
    let mut body = Vec::new();
    let got = ureq::get(&args[1])
        .call()
        .and_then(|r| {
            r.into_reader()
                .take(1 << 30)
                .read_to_end(&mut body)
                .map_err(ureq::Error::from)
        })
        .map(|_| hex::encode(Sha256::digest(&body)));
    match got {
        Ok(d) if d.eq_ignore_ascii_case(&args[2]) => println!("ok {d}"),
        Ok(d) => {
            eprintln!("refused: digest {d} is not {}", args[2]);
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("failed: {e}");
            std::process::exit(1);
        }
    }
}
