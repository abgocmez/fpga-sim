#![no_main]
// Entry point: project.toml text -> Project::from_toml -> validate.
// Same properties as crates/sim-api/tests/fuzz_project.rs (no panic, structured errors).

use libfuzzer_sys::fuzz_target;
use sim_api::Project;
use std::path::Path;

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    if let Ok(project) = Project::from_toml(&text) {
        let _ = project.validate(Path::new("."));
    }
});
