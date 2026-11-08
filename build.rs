// .exe-yə Windows manifest yerləşdirir: admin hüququ istəyir, Win7–11 dəstəyini elan edir.
use embed_manifest::manifest::ExecutionLevel;
use embed_manifest::{embed_manifest, new_manifest};

fn main() {
    if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        embed_manifest(
            new_manifest("WinTool")
                .requested_execution_level(ExecutionLevel::RequireAdministrator),
        )
        .expect("manifest yerləşdirilə bilmədi");
    }
    println!("cargo:rerun-if-changed=build.rs");
}
