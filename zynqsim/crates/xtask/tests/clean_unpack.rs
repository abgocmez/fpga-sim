//! The clean-machine check of the release archive: unpack it into a fresh
//! directory and run the programs there with a PATH that holds no toolchain.
//!
//! `cargo xtask dist` runs the same check after it builds the archive. This
//! test repeats it on an archive that already exists, for example one that a
//! CI job downloaded:
//!
//! ```text
//! cargo xtask dist
//! cargo test -p xtask -- --ignored
//! ```
//!
//! `ZYNQSIM_DIST_ARCHIVE` picks an archive; otherwise the newest one in `dist/`.

use std::path::PathBuf;
use xtask::dist;

#[test]
#[ignore = "needs an archive in dist/: run `cargo xtask dist` first"]
fn the_archive_unpacks_and_runs_from_a_clean_directory() {
    let archive = std::env::var_os("ZYNQSIM_DIST_ARCHIVE")
        .map(PathBuf::from)
        .or_else(|| dist::newest_archive(&dist::workspace_root().join("dist")))
        .expect("no zynqsim-* archive in dist/: run `cargo xtask dist` first");
    if let Err(e) = dist::verify_archive(&archive) {
        panic!("{e}");
    }
}
