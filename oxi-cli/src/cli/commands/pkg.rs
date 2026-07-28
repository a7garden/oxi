//! Package subcommand handler.

use crate::cli::PkgCommands;
use crate::storage::packages::PackageManager;
use anyhow::Result;

/// Handle `oxi pkg …` subcommands.
pub fn handle_pkg(action: &PkgCommands) -> Result<()> {
    let mut mgr = PackageManager::new()?;

    match action {
        PkgCommands::Install { source } => {
            if source.starts_with("npm:") {
                let name = source
                    .strip_prefix("npm:")
                    .ok_or_else(|| anyhow::anyhow!("Invalid npm source format: {}", source))?;
                let manifest = mgr.install_npm(name)?;
                let counts = mgr.resource_counts(&manifest.name).unwrap_or_default();
                println!(
                    "Installed {} v{} ({})",
                    manifest.name, manifest.version, counts
                );
            } else {
                let manifest = mgr.install(source)?;
                let counts = mgr.resource_counts(&manifest.name).unwrap_or_default();
                println!(
                    "Installed {} v{} ({})",
                    manifest.name, manifest.version, counts
                );
            }
        }
        PkgCommands::List => {
            let packages = mgr.list();
            if packages.is_empty() {
                println!("No packages installed.");
            } else {
                println!(
                    "{:<30} {:<10} {:<15} INSTALL DIR",
                    "NAME", "VERSION", "RESOURCES"
                );
                println!("{:-<30} {:-<10} {:-<15} {:-<40}", "", "", "", "");
                for pkg in packages {
                    let counts = mgr.resource_counts(&pkg.name).unwrap_or_default();
                    let install_dir = mgr
                        .get_install_dir(&pkg.name)
                        .map(|d| d.display().to_string())
                        .unwrap_or_else(|| "-".to_string());
                    println!(
                        "{:<30} {:<10} {:<15} {}",
                        pkg.name, pkg.version, counts, install_dir
                    );

                    // Show discovered resources
                    if let Ok(resources) = mgr.discover_resources(&pkg.name) {
                        for r in &resources {
                            println!("    {} {}", r.kind, r.relative_path);
                        }
                    }
                }
            }
        }
        PkgCommands::Uninstall { name } => {
            mgr.uninstall(name)?;
            println!("Uninstalled {}", name);
        }
        PkgCommands::Update { name } => match name {
            Some(pkg_name) => {
                let manifest = mgr.update(pkg_name)?;
                println!("Updated {} to v{}", manifest.name, manifest.version);
            }
            None => {
                let packages: Vec<String> = mgr.list().iter().map(|p| p.name.clone()).collect();
                if packages.is_empty() {
                    println!("No packages to update.");
                } else {
                    for pkg_name in &packages {
                        match mgr.update(pkg_name) {
                            Ok(manifest) => {
                                println!("Updated {} to v{}", manifest.name, manifest.version);
                            }
                            Err(e) => {
                                eprintln!(
                                    "{}",
                                    crate::print_mode::format_error(&format!(
                                        "Failed to update {}: {}",
                                        pkg_name, e
                                    ))
                                );
                            }
                        }
                    }
                }
            }
        },
    }

    Ok(())
}
