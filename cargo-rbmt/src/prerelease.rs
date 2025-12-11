//! Pre-release readiness checks.

use crate::environment::{get_crate_dirs, quiet_println, CONFIG_FILE_PATH};
use crate::quiet_cmd;
use serde::Deserialize;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::Path;
use xshell::Shell;

/// Pre-release configuration loaded from rbmt.toml.
#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct Config {
    prerelease: PrereleaseConfig,
}

/// Pre-release-specific configuration.
#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct PrereleaseConfig {
    /// If true, opt-out of pre-release checks for this package.
    skip: bool,
}

impl PrereleaseConfig {
    /// Load pre-release configuration from a crate directory.
    fn load(crate_dir: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let config_path = crate_dir.join(CONFIG_FILE_PATH);

        if !config_path.exists() {
            // Return default config (skip = false) if file doesn't exist.
            return Ok(PrereleaseConfig { skip: false });
        }

        let contents = std::fs::read_to_string(&config_path)?;
        let config: Config = toml::from_str(&contents)?;
        Ok(config.prerelease)
    }
}

/// Run pre-release readiness checks for all packages.
pub fn run(sh: &Shell, packages: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let crate_dirs = get_crate_dirs(sh, packages)?;
    quiet_println(&format!(
        "Running pre-release checks on {} crates",
        crate_dirs.len()
    ));

    let mut skipped = Vec::new();

    for crate_dir in &crate_dirs {
        let config = PrereleaseConfig::load(Path::new(crate_dir))?;

        if config.skip {
            skipped.push(crate_dir.as_str());
            quiet_println(&format!("Skipping crate: {} (marked as skip)", crate_dir));
            continue;
        }

        quiet_println(&format!("Checking crate: {}", crate_dir));

        let _dir = sh.push_dir(crate_dir);

        // Run all pre-release checks. Return immediately on first failure.
        if let Err(e) = check_todos(sh) {
            eprintln!("Pre-release check failed for {}: {}", crate_dir, e);
            return Err(e);
        }

        if let Err(e) = check_crate(sh) {
            eprintln!("Pre-release check failed for {}: {}", crate_dir, e);
            return Err(e);
        }
    }

    quiet_println("All pre-release checks passed");
    Ok(())
}

/// Check for TODO comments in source files.
fn check_todos(sh: &Shell) -> Result<(), Box<dyn std::error::Error>> {
    quiet_println("Checking for TODO comments...");

    let mut todos = Vec::new();
    let src_dir = sh.current_dir().join("src");

    // Recursively walk the src/ directory.
    let mut dirs_to_visit = vec![src_dir];
    while let Some(dir) = dirs_to_visit.pop() {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.is_dir() {
                dirs_to_visit.push(path);
            } else if path.extension().and_then(|s| s.to_str()) == Some("rs") {
                // Check Rust source files for TODO comments.
                let file = fs::File::open(&path)?;
                let reader = BufReader::new(file);

                for (line_num, line) in reader.lines().enumerate() {
                    let line = line?;
                    if line.contains("// TODO") || line.contains("/* TODO") {
                        todos.push((path.clone(), line_num + 1, line));
                    }
                }
            }
        }
    }

    if !todos.is_empty() {
        eprintln!("\nFound {} TODO comment(s):", todos.len());
        for (file, line_num, line) in &todos {
            eprintln!("{}:{}:{}", file.display(), line_num, line.trim());
        }
        return Err(format!("Found {} TODO comments", todos.len()).into());
    }

    quiet_println("No TODO comments found");
    Ok(())
}

/// Check that the crate can be packaged and published.
///
/// A crate may work with local path dependencies, but fail when published
/// because the version specifications don't match the published versions
/// or don't resolve correctly.
fn check_crate(sh: &Shell) -> Result<(), Box<dyn std::error::Error>> {
    quiet_println("Running cargo package...");
    quiet_cmd!(sh, "cargo package").run()?;
    quiet_println("cargo package succeeded");

    let (package_name, version) = get_package_info(sh)?;
    let package_dir = format!("target/package/{}-{}", package_name, version);
    quiet_println(&format!("Testing packaged crate in {}...", package_dir));

    let _dir = sh.push_dir(&package_dir);
    // Broad test to try and weed out any dependency issues.
    quiet_cmd!(sh, "cargo test --all-features --all-targets").run()?;
    quiet_println("Packaged crate tests passed");

    Ok(())
}

/// Get the current package name and version from cargo metadata.
fn get_package_info(sh: &Shell) -> Result<(String, String), Box<dyn std::error::Error>> {
    let metadata = xshell::cmd!(sh, "cargo metadata --no-deps --format-version 1").read()?;
    let json: serde_json::Value = serde_json::from_str(&metadata)?;

    // Find the package that matches the current directory.
    let current_dir = sh.current_dir();
    let current_manifest = current_dir.join("Cargo.toml");

    let packages = json["packages"]
        .as_array()
        .ok_or("Missing 'packages' field in cargo metadata")?;

    for package in packages {
        let manifest_path = package["manifest_path"]
            .as_str()
            .ok_or("Missing manifest_path in package")?;

        if manifest_path == current_manifest.to_str().ok_or("Invalid path")? {
            let name = package["name"]
                .as_str()
                .ok_or("Missing name in package")?
                .to_string();

            let version = package["version"]
                .as_str()
                .ok_or("Missing version in package")?
                .to_string();

            return Ok((name, version));
        }
    }

    Err("Could not find current package in cargo metadata".into())
}
