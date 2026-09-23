use std::{
    io::{self, Write},
    path::PathBuf,
};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use rcc_core::pack::{
    create_pack, extract_pack, inspect_pack, verify_pack, PackInspection,
    PackOptions,
};

#[derive(Debug, Parser)]
#[command(
    name = "rcc-pack",
    version,
    about = "Create and inspect deterministic RCC native-toolchain packs"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Create a deterministic .rccpack from a directory tree.
    Create {
        /// Directory whose regular files become pack entries.
        source: PathBuf,

        /// New .rccpack path. Existing files are never overwritten.
        output: PathBuf,

        /// Stable lowercase pack identifier recorded in the manifest.
        #[arg(long)]
        pack_id: String,

        /// Stable lowercase build/source revision recorded in the manifest.
        #[arg(long)]
        revision: String,

        /// Host triple that can execute the tools in this pack.
        #[arg(long)]
        host: String,

        /// Profile ID supported by this payload. Repeat for multiple profiles.
        #[arg(long = "profile", required = true)]
        profiles: Vec<String>,
    },

    /// Validate the header/payload digest and list manifest entries.
    List {
        pack: PathBuf,

        /// Emit the schema manifest as JSON.
        #[arg(long)]
        json: bool,
    },

    /// Decompress all entries and verify every file digest.
    Verify {
        pack: PathBuf,

        /// Emit the verified manifest as JSON.
        #[arg(long)]
        json: bool,

        /// List every verified file after the summary.
        #[arg(long)]
        verbose: bool,
    },

    /// Verify and atomically extract into a new directory.
    Extract { pack: PathBuf, output: PathBuf },
}

fn main() {
    if let Err(error) = run(Cli::parse()) {
        eprintln!("rcc-pack: {error:#}");
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Create {
            source,
            output,
            pack_id,
            revision,
            host,
            profiles,
        } => {
            let options = PackOptions::new(pack_id, revision, host, profiles);
            let inspection = create_pack(&source, &output, &options)
                .with_context(|| {
                    format!(
                        "failed to create {} from {}",
                        output.display(),
                        source.display()
                    )
                })?;
            println!(
                "created {} ({} files, {} bytes, sha256 {})",
                output.display(),
                inspection.manifest.files.len(),
                inspection.pack_size,
                inspection.sha256
            );
        }
        Command::List { pack, json } => {
            let inspection = inspect_pack(&pack).with_context(|| {
                format!("failed to inspect {}", pack.display())
            })?;
            print_inspection(&inspection, json, false, true)?;
        }
        Command::Verify {
            pack,
            json,
            verbose,
        } => {
            let inspection = verify_pack(&pack).with_context(|| {
                format!("failed to verify {}", pack.display())
            })?;
            print_inspection(&inspection, json, true, verbose)?;
        }
        Command::Extract { pack, output } => {
            let inspection =
                extract_pack(&pack, &output).with_context(|| {
                    format!(
                        "failed to extract {} into {}",
                        pack.display(),
                        output.display()
                    )
                })?;
            println!(
                "extracted {} files into {} (pack sha256 {})",
                inspection.manifest.files.len(),
                output.display(),
                inspection.sha256
            );
        }
    }
    Ok(())
}

fn print_inspection(
    inspection: &PackInspection,
    json: bool,
    verified: bool,
    include_files: bool,
) -> Result<()> {
    if json {
        serde_json::to_writer_pretty(io::stdout().lock(), &inspection.manifest)
            .context("failed to write manifest JSON")?;
        println!();
        return Ok(());
    }

    let mut output = io::BufWriter::new(io::stdout().lock());
    writeln!(
        output,
        "{} {}@{} host={} files={} size={} sha256={}",
        if verified { "verified" } else { "pack" },
        inspection.manifest.pack_id,
        inspection.manifest.revision,
        inspection.manifest.host,
        inspection.manifest.files.len(),
        inspection.pack_size,
        inspection.sha256
    )?;
    writeln!(
        output,
        "profiles={}",
        inspection
            .manifest
            .profiles
            .iter()
            .cloned()
            .collect::<Vec<_>>()
            .join(",")
    )?;
    if !include_files {
        output.flush()?;
        return Ok(());
    }
    writeln!(output, "MODE\tORIGINAL\tCOMPRESSED\tSHA256\tPATH")?;
    for file in &inspection.manifest.files {
        writeln!(
            output,
            "{}\t{}\t{}\t{}\t{}",
            if file.executable { "x" } else { "-" },
            file.original_len,
            file.compressed_len,
            file.sha256,
            file.path
        )?;
    }
    output.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_all_subcommands() {
        Cli::try_parse_from([
            "rcc-pack",
            "create",
            "payload",
            "payload.rccpack",
            "--pack-id",
            "native-core",
            "--revision",
            "r1",
            "--host",
            "aarch64-apple-darwin",
            "--profile",
            "macos-aarch64",
        ])
        .unwrap();
        Cli::try_parse_from(["rcc-pack", "list", "payload.rccpack", "--json"])
            .unwrap();
        Cli::try_parse_from(["rcc-pack", "verify", "payload.rccpack"]).unwrap();
        Cli::try_parse_from([
            "rcc-pack",
            "extract",
            "payload.rccpack",
            "extracted",
        ])
        .unwrap();
    }
}
