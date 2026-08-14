//! Places the nRF52840 memory map where the Cortex-M linker can find it.

use std::{env, fs, path::PathBuf, process::Command};

fn git_output(manifest_dir: &PathBuf, args: &[&str]) -> Option<String> {
    match Command::new("git")
        .args(args)
        .current_dir(manifest_dir)
        .output()
    {
        Ok(output) if output.status.success() => String::from_utf8(output.stdout)
            .ok()
            .map(|value| value.trim().to_owned()),
        Ok(_) | Err(_) => None,
    }
}

fn main() {
    let manifest_dir = PathBuf::from(
        env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set by Cargo"),
    );
    let output = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR is set by Cargo"));
    fs::copy("memory.x", output.join("memory.x")).expect("copy memory.x");
    println!("cargo:rustc-link-search={}", output.display());
    println!("cargo:rerun-if-changed=memory.x");

    let head_path = git_output(&manifest_dir, &["rev-parse", "--git-path", "HEAD"]);
    if let Some(path) = head_path.as_deref() {
        println!("cargo:rerun-if-changed={path}");
    }
    if let Some(branch) = git_output(
        &manifest_dir,
        &["symbolic-ref", "--quiet", "--short", "HEAD"],
    ) && let Some(path) = git_output(
        &manifest_dir,
        &["rev-parse", "--git-path", &format!("refs/heads/{branch}")],
    ) {
        println!("cargo:rerun-if-changed={path}");
    }
    if let Some(path) = git_output(&manifest_dir, &["rev-parse", "--git-path", "index"]) {
        println!("cargo:rerun-if-changed={path}");
    }

    // A missing/failed git command is expected in source archives and must not
    // make the firmware unbuildable; zero explicitly means the build ID is
    // unavailable rather than silently pretending that a hash was found.
    let build_id = git_output(&manifest_dir, &["rev-parse", "--short=8", "HEAD"])
        .filter(|hash| hash.len() == 8 && hash.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .and_then(|hash| u32::from_str_radix(&hash, 16).ok())
        .unwrap_or(0);
    let dirty = git_output(&manifest_dir, &["status", "--porcelain"])
        .map(|status| !status.is_empty())
        .unwrap_or(false);
    println!("cargo:rustc-env=UKF_BUILD_ID={build_id:08x}");
    println!("cargo:rustc-env=UKF_BUILD_DIRTY={}", u8::from(dirty));
}
