use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};
use mediastat_core::{
    format_bytes, format_count, set_active_language, t, AppSettings, Language, NodeId, ScanConfig,
    SizeMetric,
};
use mediastat_scanner::{
    enumerate_drives, is_process_elevated, run_scan_pipeline, scan_permissions_for_scope,
    ScanPhase, ScanProgress,
};
use mediastat_storage::{
    compute_diff, export_nodes, load_snapshot, save_snapshot, ExportFormat,
};

#[derive(Parser)]
#[command(
    name = "mediastat-cli",
    version,
    about = "High-speed WinDirStat-style disk space analyzer (NtQueryDirectoryFile / NTFS $MFT + Hardlinks + SysAdmin)"
)]
struct Cli {
    /// UI & report output language (`es` or `en`; defaults to saved GUI setting)
    #[arg(long, global = true)]
    lang: Option<String>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Clone, Copy, ValueEnum)]
enum CliExportFormat {
    Csv,
    Json,
    Jsonl,
    Html,
    PermsCsv,
    DupesCsv,
    Ps1Quarantine,
}

impl From<CliExportFormat> for ExportFormat {
    fn from(f: CliExportFormat) -> Self {
        match f {
            CliExportFormat::Csv => ExportFormat::Csv,
            CliExportFormat::Json => ExportFormat::Json,
            CliExportFormat::Jsonl => ExportFormat::Jsonl,
            CliExportFormat::Html => ExportFormat::ExecutiveHtml,
            CliExportFormat::PermsCsv => ExportFormat::PermissionsCsv,
            CliExportFormat::DupesCsv => ExportFormat::DuplicatesCsv,
            CliExportFormat::Ps1Quarantine => ExportFormat::PowerShellQuarantine,
        }
    }
}

#[derive(Subcommand)]
enum Commands {
    /// Scan one or more drives, folders, or UNC paths and save a `.mstat` snapshot
    Scan {
        #[arg(required = true)]
        targets: Vec<PathBuf>,

        #[arg(short, long, default_value = "audit.mstat")]
        out: PathBuf,

        /// Enable duplicate detection (BLAKE3) during scanning
        #[arg(long)]
        find_duplicates: bool,

        /// Use fast Sparse Hash sampling for files > 64 MiB (implies --find-duplicates)
        #[arg(long)]
        sparse_dupes: bool,

        /// Resolve NTFS/SMB file owners (DOMAIN\user) during scanning
        #[arg(long)]
        scan_owners: bool,

        /// Scan folder DACL/ACL permissions for security auditing
        #[arg(long)]
        scan_perms: bool,

        #[arg(long)]
        no_mft: bool,

        #[arg(long, default_value_t = 0)]
        threads: usize,

        #[arg(long)]
        export_csv: Option<PathBuf>,

        #[arg(long)]
        export_html: Option<PathBuf>,
    },

    /// Export a `.mstat` snapshot to CSV, JSON, JSONL, HTML, Permissions CSV, Duplicates CSV, or Quarantine PS1
    Export {
        snapshot: PathBuf,

        #[arg(short, long, value_enum, default_value_t = CliExportFormat::Csv)]
        format: CliExportFormat,

        #[arg(short, long)]
        out: PathBuf,

        /// Limit export to the Top N largest files (0 = all)
        #[arg(long, default_value_t = 0)]
        top: usize,
    },

    /// Compare two `.mstat` snapshots ("Before" vs. "After")
    Diff {
        before: PathBuf,
        after: PathBuf,
        #[arg(short, long)]
        json_out: Option<PathBuf>,
    },

    /// Print an executive summary of a `.mstat` snapshot
    Summary { snapshot: PathBuf },

    /// List detected drives, resolved UNC targets, capacity, and NTFS $MFT support
    Drives,
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .init();

    let cli = Cli::parse();
    let active_lang = cli
        .lang
        .as_deref()
        .and_then(Language::from_code)
        .unwrap_or_else(|| AppSettings::load_or_default().language);
    set_active_language(active_lang);

    match cli.command {
        Commands::Drives => {
            println!(
                "{}: {}",
                t(
                    "Privilegios Administrador (NTFS $MFT)",
                    "Administrator Privileges (NTFS $MFT)"
                ),
                if is_process_elevated() {
                    t("SÍ", "YES")
                } else {
                    "NO"
                }
            );
            println!(
                "{:<6} {:<24} {:<6} {:<12} {:>11} {:>11}  {}",
                t("Ruta", "Path"),
                t("Etiqueta", "Label"),
                "FS",
                t("Tipo", "Type"),
                t("Usado", "Used"),
                "Total",
                t("Origen UNC", "UNC Target")
            );
            for d in enumerate_drives() {
                println!(
                    "{:<6} {:<24} {:<6} {:<12} {:>11} {:>11}  {}",
                    d.path.display(),
                    d.label,
                    d.filesystem,
                    d.media_type.label(),
                    format_bytes(d.used_bytes()),
                    format_bytes(d.total_bytes),
                    d.unc_remote_path.as_deref().unwrap_or("—")
                );
            }
        }
        Commands::Scan {
            targets,
            out,
            find_duplicates,
            sparse_dupes,
            scan_owners,
            scan_perms,
            no_mft,
            threads,
            export_csv,
            export_html,
        } => {
            let config = ScanConfig {
                targets,
                use_ntfs_mft: !no_mft,
                find_duplicates: find_duplicates || sparse_dupes,
                sparse_duplicate_hash: sparse_dupes,
                resolve_owners_during_scan: scan_owners,
                scan_threads: threads,
                ..Default::default()
            };

            let progress = Arc::new(ScanProgress::new());
            let prog_monitor = Arc::clone(&progress);

            let monitor_handle = std::thread::spawn(move || loop {
                std::thread::sleep(Duration::from_millis(500));
                let phase = prog_monitor.current_phase();
                let files = prog_monitor.walker.files_scanned.load(Ordering::Relaxed);
                let dirs = prog_monitor.walker.dirs_scanned.load(Ordering::Relaxed);
                let bytes = prog_monitor.walker.bytes_scanned.load(Ordering::Relaxed);
                let ddone = prog_monitor.dup_done.load(Ordering::Relaxed);
                let dtot = prog_monitor.dup_total.load(Ordering::Relaxed);
                if dtot > 0 {
                    eprint!(
                        "\r[{}] {}: {} ({}) | {}: {} | BLAKE3: {}/{}   ",
                        phase.label(),
                        t("Archivos", "Files"),
                        format_count(files),
                        format_bytes(bytes),
                        t("Carpetas", "Folders"),
                        format_count(dirs),
                        format_count(ddone),
                        format_count(dtot),
                    );
                } else {
                    eprint!(
                        "\r[{}] {}: {} ({}) | {}: {}   ",
                        phase.label(),
                        t("Archivos", "Files"),
                        format_count(files),
                        format_bytes(bytes),
                        t("Carpetas", "Folders"),
                        format_count(dirs),
                    );
                }
                if matches!(phase, ScanPhase::Completed | ScanPhase::Cancelled) {
                    eprintln!();
                    break;
                }
            });

            let mut tree = run_scan_pipeline(&config, &progress);
            let _ = monitor_handle.join();

            if scan_perms {
                eprintln!(
                    "{}",
                    t(
                        "Escaneando permisos DACL/ACLs...",
                        "Scanning DACL/ACL permissions..."
                    )
                );
                tree.permissions = scan_permissions_for_scope(
                    &tree,
                    NodeId::ROOT,
                    false,
                    25_000,
                    &progress.cancel,
                );
                eprintln!(
                    "{}: {}",
                    t("Entradas DACL registradas", "Recorded DACL entries"),
                    tree.permissions.len()
                );
            }

            print_summary(&tree);
            eprintln!(
                "{} {}...",
                t("Guardando snapshot en", "Saving snapshot to"),
                out.display()
            );
            save_snapshot(&tree, &out)?;
            eprintln!(
                "{}",
                t(
                    "Snapshot guardado correctamente.",
                    "Snapshot saved successfully."
                )
            );

            if export_csv.is_some() || export_html.is_some() {
                let files = tree.all_files_sorted(SizeMetric::Logical);
                if let Some(csv_path) = export_csv {
                    let count = export_nodes(&tree, &files, ExportFormat::Csv, &csv_path)?;
                    eprintln!(
                        "{} ({} {}) {} {}",
                        t("CSV exportado", "CSV exported"),
                        format_count(count as u64),
                        t("ficheros", "files"),
                        t("en", "to"),
                        csv_path.display()
                    );
                }
                if let Some(html_path) = export_html {
                    export_nodes(&tree, &files, ExportFormat::ExecutiveHtml, &html_path)?;
                    eprintln!(
                        "{} {}",
                        t(
                            "Reporte ejecutivo HTML generado en",
                            "Executive HTML report generated at"
                        ),
                        html_path.display()
                    );
                }
            }
        }
        Commands::Export {
            snapshot,
            format,
            out,
            top,
        } => {
            let tree = load_snapshot(&snapshot)?;
            let nodes = if top > 0 {
                tree.largest_files(top, SizeMetric::Logical)
            } else {
                tree.all_files_sorted(SizeMetric::Logical)
            };
            let count = export_nodes(&tree, &nodes, format.into(), &out)?;
            println!(
                "{} {} {} {}",
                t("Exportados", "Exported"),
                format_count(count as u64),
                t("registros a", "records to"),
                out.display()
            );
        }
        Commands::Diff {
            before,
            after,
            json_out,
        } => {
            let t1 = load_snapshot(&before)?;
            let t2 = load_snapshot(&after)?;
            let diff = compute_diff(&t1, &t2);
            println!(
                "=== {} ===",
                t("Comparación de Snapshots", "Snapshot Comparison")
            );
            println!(
                "{}: {} -> {} (Delta: {}{})",
                t("Tamaño total", "Total size"),
                format_bytes(diff.bytes_before),
                format_bytes(diff.bytes_after),
                if diff.delta_bytes >= 0 { "+" } else { "-" },
                format_bytes(diff.delta_bytes.unsigned_abs())
            );
            println!(
                "{}: {} -> {}",
                t("Archivos", "Files"),
                format_count(diff.files_before),
                format_count(diff.files_after),
            );
            println!(
                "{}: {} | {}: {}",
                t("Nuevos archivos", "Newly added files"),
                diff.newly_added_files.len(),
                t("Archivos eliminados", "Removed files"),
                diff.removed_files.len()
            );
            if let Some(out) = json_out {
                std::fs::write(&out, serde_json::to_string_pretty(&diff)?)?;
                println!("{} {}", t("Diff guardado en", "Diff saved to"), out.display());
            }
        }
        Commands::Summary { snapshot } => {
            let tree = load_snapshot(&snapshot)?;
            print_summary(&tree);
        }
    }

    Ok(())
}

fn print_summary(tree: &mediastat_core::ScanTree) {
    let s = tree.summary();
    println!("============================================================");
    println!(
        "  MediaStat v0.3 — {} ({})",
        t("Resumen del Análisis", "Scan Summary"),
        s.engine_used
    );
    println!("============================================================");
    println!(
        "  {:<26} {:.2} s",
        t("Duración total:", "Total duration:"),
        s.scan_duration_secs
    );
    println!(
        "  {:<26} {} / {}",
        t("Archivos / Carpetas:", "Files / Folders:"),
        format_count(s.total_files),
        format_count(s.total_dirs)
    );
    println!(
        "  {:<26} {} / {}",
        t("Tamaño lógico / en disco:", "Logical / Allocated size:"),
        format_bytes(s.total_logical_bytes),
        format_bytes(s.total_allocated_bytes)
    );
    if !tree.hardlinks.is_empty() {
        let links: u64 = tree.hardlinks.iter().map(|g| g.nodes.len() as u64).sum();
        println!(
            "  {:<26} {} {} ({} {})",
            t("Hardlinks NTFS dedup.:", "NTFS Hardlinks dedup.:"),
            format_count(tree.hardlinks.len() as u64),
            t("grupos", "groups"),
            format_count(links),
            t("enlaces", "links")
        );
    }
    if s.duplicate_groups > 0 {
        println!(
            "  {:<26} {} {} ({} {})",
            t("Duplicados:", "Duplicates:"),
            format_count(s.duplicate_groups),
            t("grupos", "groups"),
            format_bytes(s.duplicate_reclaimable_bytes),
            t("recuperables", "reclaimable")
        );
    }
    println!(
        "  SysAdmin (Denied/>260/Cloud): {} / {} / {}",
        s.access_denied_count, s.long_path_count, s.cloud_offline_count
    );
    println!("============================================================");
}
