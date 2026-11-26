use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use anyhow::Result;
use mediastat_core::{
    format_timestamp, t, FileCategory, NodeFlags, NodeId, ScanTree, StorageTierConfig,
};
use serde::Serialize;

use crate::html_report::write_executive_html_report;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    Csv,
    Json,
    Jsonl,
    ExecutiveHtml,
    PermissionsCsv,
    DuplicatesCsv,
    PowerShellQuarantine,
}

impl ExportFormat {
    pub fn label(self) -> &'static str {
        match self {
            Self::Csv => t("CSV para Excel (.csv)", "Excel-ready CSV (.csv)"),
            Self::Json => t("JSON estructurado (.json)", "Structured JSON (.json)"),
            Self::Jsonl => t("JSON Lines streaming (.jsonl)", "Streaming JSON Lines (.jsonl)"),
            Self::ExecutiveHtml => t(
                "Reporte Ejecutivo HTML autocontenido (.html)",
                "Self-Contained Executive HTML Report (.html)",
            ),
            Self::PermissionsCsv => {
                t("Auditoría de Permisos DACL (.csv)", "DACL Permissions Audit (.csv)")
            }
            Self::DuplicatesCsv => t("Grupos de Duplicados (.csv)", "Duplicate Groups (.csv)"),
            Self::PowerShellQuarantine => t(
                "Script PowerShell de Cuarentena (.ps1)",
                "PowerShell Quarantine Script (.ps1)",
            ),
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Self::Csv | Self::PermissionsCsv | Self::DuplicatesCsv => "csv",
            Self::Json => "json",
            Self::Jsonl => "jsonl",
            Self::ExecutiveHtml => "html",
            Self::PowerShellQuarantine => "ps1",
        }
    }
}

#[derive(Debug, Serialize)]
struct ExportRow {
    full_path: String,
    filename: String,
    is_dir: bool,
    extension: String,
    category: String,
    attributes: String,
    owner: String,
    logical_size_bytes: u64,
    allocated_size_bytes: u64,
    mtime: String,
}

fn build_row(tree: &ScanTree, nid: NodeId) -> ExportRow {
    let idx = nid.index();
    let is_dir = tree.flags[idx].contains(NodeFlags::IS_DIR);
    let ext = tree.ext(nid);
    let category = if is_dir {
        t("Carpeta", "Folder").to_string()
    } else {
        FileCategory::from_extension(ext).label().to_string()
    };

    ExportRow {
        full_path: tree.resolve_path(nid).to_string_lossy().into_owned(),
        filename: tree.name(nid).to_string(),
        is_dir,
        extension: ext.to_string(),
        category,
        attributes: tree.win_attrs[idx].to_compact_string(),
        owner: tree.owner(nid).to_string(),
        logical_size_bytes: tree.logical_sizes[idx],
        allocated_size_bytes: tree.allocated_sizes[idx],
        mtime: format_timestamp(tree.mtimes[idx]),
    }
}

pub fn export_nodes(
    tree: &ScanTree,
    nodes: &[NodeId],
    format: ExportFormat,
    output_path: &Path,
) -> Result<usize> {
    export_nodes_with_cost(
        tree,
        nodes,
        format,
        &StorageTierConfig::default_tiers(),
        18.0,
        "€",
        output_path,
    )
}

pub fn export_nodes_with_cost(
    tree: &ScanTree,
    nodes: &[NodeId],
    format: ExportFormat,
    tiers: &[StorageTierConfig],
    cost_per_tib: f64,
    currency: &str,
    output_path: &Path,
) -> Result<usize> {
    if format == ExportFormat::ExecutiveHtml {
        return write_executive_html_report(tree, nodes, tiers, cost_per_tib, currency, output_path);
    }

    let file = File::create(output_path)?;
    let mut writer = BufWriter::new(file);

    match format {
        ExportFormat::ExecutiveHtml => unreachable!(),
        ExportFormat::Csv => {
            writer.write_all(&[0xEF, 0xBB, 0xBF])?;
            let mut csv_w = csv::Writer::from_writer(writer);
            for &nid in nodes {
                csv_w.serialize(build_row(tree, nid))?;
            }
            csv_w.flush()?;
            Ok(nodes.len())
        }
        ExportFormat::PermissionsCsv => {
            #[derive(Serialize)]
            struct PermRow<'a> {
                path: String,
                is_dir: bool,
                owner: &'a str,
                identity: &'a str,
                access_type: &'a str,
                rights: &'a str,
                inherited: bool,
            }
            writer.write_all(&[0xEF, 0xBB, 0xBF])?;
            let mut csv_w = csv::Writer::from_writer(writer);
            for p in &tree.permissions {
                csv_w.serialize(PermRow {
                    path: p.path.to_string_lossy().into_owned(),
                    is_dir: p.is_dir,
                    owner: &p.owner,
                    identity: &p.identity,
                    access_type: &p.access_type,
                    rights: &p.rights,
                    inherited: p.inherited,
                })?;
            }
            csv_w.flush()?;
            Ok(tree.permissions.len())
        }
        ExportFormat::DuplicatesCsv => {
            #[derive(Serialize)]
            struct DupRow<'a> {
                group_index: usize,
                blake3: &'a str,
                is_probable_sparse: bool,
                file_size_bytes: u64,
                reclaimable_bytes: u64,
                full_path: String,
            }
            writer.write_all(&[0xEF, 0xBB, 0xBF])?;
            let mut csv_w = csv::Writer::from_writer(writer);
            let mut total = 0usize;
            for (gi, g) in tree.duplicates.iter().enumerate() {
                let rec = g.reclaimable_bytes();
                for &nid in &g.nodes {
                    csv_w.serialize(DupRow {
                        group_index: gi + 1,
                        blake3: &g.blake3_hex,
                        is_probable_sparse: g.is_probable_sparse,
                        file_size_bytes: g.file_size,
                        reclaimable_bytes: rec,
                        full_path: tree.resolve_path(nid).to_string_lossy().into_owned(),
                    })?;
                    total += 1;
                }
            }
            csv_w.flush()?;
            Ok(total)
        }
        ExportFormat::Json => {
            #[derive(Serialize)]
            struct JsonPayload<'a> {
                summary: mediastat_core::ScanSummary,
                exported_count: usize,
                targets: &'a [std::path::PathBuf],
                files: Vec<ExportRow>,
            }
            let rows: Vec<ExportRow> = nodes.iter().map(|&nid| build_row(tree, nid)).collect();
            let payload = JsonPayload {
                summary: tree.summary(),
                exported_count: rows.len(),
                targets: &tree.root_targets,
                files: rows,
            };
            serde_json::to_writer_pretty(&mut writer, &payload)?;
            writer.flush()?;
            Ok(nodes.len())
        }
        ExportFormat::Jsonl => {
            for &nid in nodes {
                let row = build_row(tree, nid);
                serde_json::to_writer(&mut writer, &row)?;
                writer.write_all(b"\n")?;
            }
            writer.flush()?;
            Ok(nodes.len())
        }
        ExportFormat::PowerShellQuarantine => {
            writer.write_all(&[0xEF, 0xBB, 0xBF])?;
            writeln!(
                writer,
                "# ============================================================================"
            )?;
            writeln!(
                writer,
                "# MediaStat — Script PowerShell de Cuarentena / Revisión (Solo revisión manual)"
            )?;
            writeln!(
                writer,
                "# Ejecuta con -WhatIf primero: .\\script.ps1 -QuarantineRoot 'D:\\Cuarentena' -WhatIf"
            )?;
            writeln!(
                writer,
                "# ============================================================================"
            )?;
            writeln!(writer, "[CmdletBinding(SupportsShouldProcess = $true)]")?;
            writeln!(writer, "param(")?;
            writeln!(
                writer,
                "    [Parameter(Mandatory = $true)][string]$QuarantineRoot,"
            )?;
            writeln!(writer, "    [switch]$MoveInsteadOfCopy")?;
            writeln!(writer, ")\n")?;
            writeln!(writer, "$Files = @(")?;
            for &nid in nodes {
                let p = tree
                    .resolve_path(nid)
                    .to_string_lossy()
                    .replace('\'', "''");
                writeln!(writer, "    '{p}'")?;
            }
            writeln!(writer, ")\n")?;
            writeln!(writer, "foreach ($src in $Files) {{")?;
            writeln!(
                writer,
                "    if (-not (Test-Path -LiteralPath $src)) {{ Write-Warning \"No existe: $src\"; continue }}"
            )?;
            writeln!(
                writer,
                "    $rel = $src -replace '^([A-Za-z]):\\\\', '$1\\' -replace '^\\\\\\\\', 'UNC\\'"
            )?;
            writeln!(writer, "    $dst = Join-Path $QuarantineRoot $rel")?;
            writeln!(writer, "    $dstDir = Split-Path -Parent $dst")?;
            writeln!(
                writer,
                "    if ($PSCmdlet.ShouldProcess($src, \"Copiar/Mover a $dst\")) {{"
            )?;
            writeln!(
                writer,
                "        New-Item -ItemType Directory -Force -Path $dstDir | Out-Null"
            )?;
            writeln!(
                writer,
                "        if ($MoveInsteadOfCopy) {{ Move-Item -LiteralPath $src -Destination $dst }}"
            )?;
            writeln!(
                writer,
                "        else {{ Copy-Item -LiteralPath $src -Destination $dst }}"
            )?;
            writeln!(writer, "    }}")?;
            writeln!(writer, "}}")?;
            writer.flush()?;
            Ok(nodes.len())
        }
    }
}
