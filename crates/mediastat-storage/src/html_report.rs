//! Self-contained Executive HTML Report generator (zero external CDNs, inline CSS/charts).

use std::fmt::Write as _;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use ahash::AHashMap;
use anyhow::Result;
use mediastat_core::{
    active_language, format_bytes, format_count, format_timestamp, t, FileCategory, NodeFlags,
    NodeId, ScanTree, StorageTierConfig,
};

const TIB: f64 = 1_099_511_627_776.0;

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Generate a standalone Executive Storage Audit HTML Dashboard with top folders, top extensions,
/// storage tier cost breakdown, duplicates, top owners, and top exported / largest files.
pub fn write_executive_html_report(
    tree: &ScanTree,
    nodes: &[NodeId],
    tiers: &[StorageTierConfig],
    cost_per_tib: f64,
    currency: &str,
    output_path: &Path,
) -> Result<usize> {
    let summary = tree.summary();
    let total_bytes = summary.total_logical_bytes.max(1);
    let dup_reclaim = summary.duplicate_reclaimable_bytes;
    let dup_monthly_savings = (dup_reclaim as f64 / TIB) * cost_per_tib;

    // Top 12 folders by logical size
    let mut top_dirs: Vec<(String, u64, u64)> = (1..tree.len())
        .filter(|&i| {
            tree.flags[i].contains(NodeFlags::IS_DIR) && !tree.flags[i].contains(NodeFlags::IS_DELETED)
        })
        .map(|i| {
            let nid = NodeId(i as u32);
            let files = tree
                .dir_stats
                .get(&nid)
                .map(|d| d.total_files)
                .unwrap_or(0);
            (
                tree.resolve_path(nid).to_string_lossy().into_owned(),
                tree.logical_sizes[i],
                files,
            )
        })
        .collect();
    top_dirs.sort_unstable_by_key(|d| std::cmp::Reverse(d.1));
    top_dirs.truncate(12);

    // Top 12 extensions
    let mut top_exts: Vec<_> = tree
        .extensions
        .entries()
        .iter()
        .filter(|e| e.file_count > 0)
        .collect();
    top_exts.sort_unstable_by_key(|e| std::cmp::Reverse(e.total_logical_bytes));
    top_exts.truncate(12);

    // Compute Age Tier breakdown
    let now = chrono::Utc::now().timestamp();
    let mut tier_bytes = vec![0u64; tiers.len()];
    let mut tier_files = vec![0u64; tiers.len()];
    let mut sorted_tier_idx: Vec<usize> = (0..tiers.len()).collect();
    sorted_tier_idx.sort_by_key(|&i| std::cmp::Reverse(tiers[i].min_age_days));

    for i in 1..tree.len() {
        let f = tree.flags[i];
        if f.contains(NodeFlags::IS_DIR) || f.contains(NodeFlags::IS_DELETED) {
            continue;
        }
        let age_days = if tree.mtimes[i] > 0 {
            ((now - tree.mtimes[i]).max(0) / 86_400) as u32
        } else {
            0
        };
        if let Some(&ti) = sorted_tier_idx
            .iter()
            .find(|&&ti| age_days >= tiers[ti].min_age_days)
        {
            tier_bytes[ti] = tier_bytes[ti].saturating_add(tree.logical_sizes[i]);
            tier_files[ti] += 1;
        }
    }

    let total_monthly_cost: f64 = tiers
        .iter()
        .enumerate()
        .map(|(i, tier)| (tier_bytes[i] as f64 / TIB) * tier.cost_per_tib_monthly)
        .sum();

    // Owner rollup if owners were resolved
    let mut by_owner: AHashMap<&str, (u64, u64)> = AHashMap::new();
    for i in 1..tree.len() {
        let f = tree.flags[i];
        if f.contains(NodeFlags::IS_DIR) || f.contains(NodeFlags::IS_DELETED) {
            continue;
        }
        let own = tree.owner(NodeId(i as u32));
        if !own.is_empty() {
            let e = by_owner.entry(own).or_default();
            e.0 += 1;
            e.1 = e.1.saturating_add(tree.logical_sizes[i]);
        }
    }
    let mut top_owners: Vec<(&str, u64, u64)> = by_owner
        .into_iter()
        .map(|(k, (c, b))| (k, c, b))
        .collect();
    top_owners.sort_unstable_by_key(|o| std::cmp::Reverse(o.2));
    top_owners.truncate(12);

    let mut html = String::with_capacity(64 * 1024);
    let _ = writeln!(
        html,
        r#"<!DOCTYPE html>
<html lang="{}">
<head>
<meta charset="utf-8">
<title>{}</title>
<style>
  :root {{ --bg:#0f131a; --card:#171d27; --border:#273142; --text:#e6edf5; --muted:#8c9bb0; --accent:#4da6ff; --green:#4cd280; --orange:#f5824b; --red:#e63946; }}
  * {{ box-sizing:border-box; }}
  body {{ font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',Roboto,sans-serif; background:var(--bg); color:var(--text); margin:0; padding:24px; line-height:1.45; }}
  h1,h2 {{ margin:0 0 12px 0; }}
  .sub {{ color:var(--muted); font-size:13px; margin-bottom:20px; }}
  .grid {{ display:grid; grid-template-columns:repeat(auto-fit,minmax(200px,1fr)); gap:12px; margin-bottom:22px; }}
  .card {{ background:var(--card); border:1px solid var(--border); border-radius:10px; padding:14px 16px; }}
  .kpi-lbl {{ font-size:12px; color:var(--muted); text-transform:uppercase; letter-spacing:0.5px; }}
  .kpi-val {{ font-size:22px; font-weight:700; margin-top:4px; }}
  .kpi-sub {{ font-size:12px; color:var(--muted); margin-top:2px; }}
  .row {{ display:grid; grid-template-columns:1fr 1fr; gap:16px; margin-bottom:20px; }}
  @media (max-width:960px) {{ .row {{ grid-template-columns:1fr; }} }}
  table {{ width:100%; border-collapse:collapse; font-size:12.5px; }}
  th,td {{ padding:7px 8px; border-bottom:1px solid var(--border); text-align:left; }}
  th {{ color:var(--muted); font-weight:600; position:sticky; top:0; background:var(--card); }}
  .mono {{ font-family:Consolas,monospace; font-size:11.5px; word-break:break-all; }}
  .pill {{ display:inline-block; padding:2px 7px; border-radius:4px; font-size:11px; font-weight:700; color:#fff; }}
  .bar-bg {{ background:#222a38; height:12px; border-radius:6px; overflow:hidden; width:100%; }}
  .bar-fg {{ height:100%; border-radius:6px; }}
</style>
</head>
<body>
<h1>{}</h1>
<div class="sub">{}: {} · {}: {} · {}: {:.2}s · {}: {}</div>
"#,
        active_language().code(),
        t(
            "MediaStat — Reporte Ejecutivo de Almacenamiento",
            "MediaStat — Executive Storage Report"
        ),
        t(
            "📊 MediaStat — Reporte Ejecutivo de Espacio en Disco",
            "📊 MediaStat — Executive Disk Space Report"
        ),
        t("Generado", "Generated"),
        esc(&format_timestamp(tree.scanned_at_unix)),
        t("Motor", "Engine"),
        esc(&summary.engine_used),
        t("Duración", "Duration"),
        summary.scan_duration_secs,
        t("Orígenes", "Targets"),
        esc(&tree
            .root_targets
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(" | "))
    );

    // KPI Grid
    let _ = write!(
        html,
        r#"<div class="grid">
  <div class="card"><div class="kpi-lbl">{}</div><div class="kpi-val" style="color:var(--accent)">{}</div><div class="kpi-sub">{}: {} · {} {} · {} {}</div></div>
  <div class="card"><div class="kpi-lbl">{}</div><div class="kpi-val" style="color:var(--green)">{}</div><div class="kpi-sub">{} {} (~{:.2} {}/{})</div></div>
  <div class="card"><div class="kpi-lbl">{}</div><div class="kpi-val" style="color:var(--orange)">{:.2} {}/{}</div><div class="kpi-sub">{:.2} {}/{}</div></div>
  <div class="card"><div class="kpi-lbl">{}</div><div class="kpi-val" style="color:var(--red)">{} {}</div><div class="kpi-sub">Access Denied: {} · &gt;260: {} · Cloud: {}</div></div>
</div>"#,
        t("Espacio Total", "Total Space"),
        esc(&format_bytes(summary.total_logical_bytes)),
        t("En disco", "On disk"),
        esc(&format_bytes(summary.total_allocated_bytes)),
        esc(&format_count(summary.total_files)),
        t("archivos", "files"),
        esc(&format_count(summary.total_dirs)),
        t("carpetas", "folders"),
        t("Duplicados Recuperables", "Reclaimable Duplicates"),
        esc(&format_bytes(dup_reclaim)),
        esc(&format_count(summary.duplicate_groups)),
        t("grupos", "groups"),
        dup_monthly_savings,
        esc(currency),
        t("mes", "mo"),
        t("Coste Estimado (Tiers)", "Est. Storage Cost (Tiers)"),
        total_monthly_cost,
        esc(currency),
        t("mes", "mo"),
        total_monthly_cost * 12.0,
        esc(currency),
        t("año", "yr"),
        t("Auditoría SysAdmin", "SysAdmin Audit"),
        summary.access_denied_count + summary.long_path_count + summary.cloud_offline_count,
        t("incidencias", "issues"),
        summary.access_denied_count,
        summary.long_path_count,
        summary.cloud_offline_count,
    );

    // Row 1: Top Folders + Top Extensions
    let _ = writeln!(
        html,
        r#"<div class="row"><div class="card"><h2>{}</h2><table><tr><th>{}</th><th>{}</th><th>{}</th></tr>"#,
        t("📁 Top Carpetas por Tamaño", "📁 Top Folders by Size"),
        t("Carpeta", "Folder"),
        t("Tamaño", "Size"),
        t("Ficheros", "Files")
    );
    for (p, sz, files) in &top_dirs {
        let pct = (*sz as f64 / total_bytes as f64 * 100.0).clamp(1.0, 100.0);
        let _ = writeln!(
            html,
            r#"<tr><td class="mono">{}<div class="bar-bg"><div class="bar-fg" style="width:{:.1}%;background:var(--accent)"></div></div></td><td>{}</td><td>{}</td></tr>"#,
            esc(p),
            pct,
            esc(&format_bytes(*sz)),
            esc(&format_count(*files))
        );
    }
    let _ = writeln!(
        html,
        r#"</table></div><div class="card"><h2>{}</h2><table><tr><th>{}</th><th>{}</th><th>{}</th><th>%</th></tr>"#,
        t("🎨 Top Extensiones", "🎨 Top Extensions"),
        t("Extensión", "Extension"),
        t("Ficheros", "Files"),
        t("Tamaño", "Size")
    );
    for e in &top_exts {
        let [r, g, b] = e.color;
        let pct = (e.total_logical_bytes as f64 / total_bytes as f64 * 100.0).clamp(0.0, 100.0);
        let ext_lbl = if e.ext.is_empty() {
            t("(sin ext)", "(no ext)").to_string()
        } else {
            format!(".{}", e.ext)
        };
        let _ = writeln!(
            html,
            r#"<tr><td><span class="pill" style="background:rgb({r},{g},{b})">{}</span><div class="bar-bg" style="margin-top:4px"><div class="bar-fg" style="width:{:.1}%;background:rgb({r},{g},{b})"></div></div></td><td>{}</td><td>{}</td><td>{:.1}%</td></tr>"#,
            esc(&ext_lbl),
            pct.max(1.0),
            esc(&format_count(e.file_count)),
            esc(&format_bytes(e.total_logical_bytes)),
            pct
        );
    }
    let _ = writeln!(html, "</table></div></div>");

    // Row 2: Age Tiers & Cost + Top Owners
    let _ = writeln!(
        html,
        r#"<div class="row"><div class="card"><h2>{}</h2><table><tr><th>Tier</th><th>{}</th><th>{}</th><th>{}</th></tr>"#,
        t("🌡 Tiers de Antigüedad y Coste Mensual", "🌡 Age Tiers & Monthly Cost"),
        t("Archivos", "Files"),
        t("Tamaño", "Size"),
        t("Coste Est./mes", "Est. Cost/mo")
    );
    for (i, tier) in tiers.iter().enumerate() {
        let [r, g, b] = tier.accent_rgb;
        let cost = (tier_bytes[i] as f64 / TIB) * tier.cost_per_tib_monthly;
        let _ = writeln!(
            html,
            r#"<tr><td><span class="pill" style="background:rgb({r},{g},{b})">{}</span></td><td>{}</td><td>{}</td><td>{:.2} {}</td></tr>"#,
            esc(&tier.name),
            esc(&format_count(tier_files[i])),
            esc(&format_bytes(tier_bytes[i])),
            cost,
            esc(currency)
        );
    }
    let _ = writeln!(
        html,
        r#"</table></div><div class="card"><h2>{}</h2>"#,
        t("👤 Top Propietarios (DOMINIO\\usuario)", "👤 Top Owners (DOMAIN\\user)")
    );
    if top_owners.is_empty() {
        let _ = writeln!(
            html,
            r#"<p class="sub">{}</p>"#,
            t(
                "No se resolvieron propietarios durante este escaneo. Usa la pestaña <b>Permisos / Propietarios</b> o <code>--scan-owners</code> para incluir el desglose por usuario.",
                "No owners were resolved during this scan. Use the <b>Permissions / Owners</b> tab or <code>--scan-owners</code> to include a per-user breakdown."
            )
        );
    } else {
        let _ = writeln!(
            html,
            "<table><tr><th>{}</th><th>{}</th><th>{}</th><th>%</th></tr>",
            t("Propietario", "Owner"),
            t("Archivos", "Files"),
            t("Tamaño", "Size")
        );
        for (own, cnt, sz) in &top_owners {
            let pct = (*sz as f64 / total_bytes as f64) * 100.0;
            let _ = writeln!(
                html,
                r#"<tr><td class="mono">{}</td><td>{}</td><td>{}</td><td>{:.1}%</td></tr>"#,
                esc(own),
                esc(&format_count(*cnt)),
                esc(&format_bytes(*sz)),
                pct
            );
        }
        let _ = writeln!(html, "</table>");
    }
    let _ = writeln!(html, "</div></div>");

    // Table: Top 200 Largest / Exported Files
    let exported = nodes.len().min(200);
    let _ = writeln!(
        html,
        r#"<div class="card" style="margin-bottom:20px"><h2>🏆 Top {} {} ({} {})</h2><table><tr><th>{}</th><th>{}</th><th>{}</th><th>{}</th><th>{}</th><th>{}</th></tr>"#,
        exported,
        t("Archivos Más Grandes / Exportados", "Largest / Exported Files"),
        t("de", "of"),
        format_count(nodes.len() as u64),
        t("Categoría", "Category"),
        t("Tamaño", "Size"),
        t("En Disco", "On Disk"),
        t("Propietario", "Owner"),
        t("Modificado", "Modified"),
        t("Ruta Completa", "Full Path")
    );
    for &nid in nodes.iter().take(200) {
        let idx = nid.index();
        let is_dir = tree.flags[idx].contains(NodeFlags::IS_DIR);
        let (cat_lbl, [r, g, b]) = if is_dir {
            (t("Carpeta", "Folder"), [76, 136, 210])
        } else {
            let c = FileCategory::from_extension(tree.ext(nid));
            (c.short_label(), c.accent_rgb())
        };
        let own = tree.owner(nid);
        let _ = writeln!(
            html,
            r#"<tr><td><span class="pill" style="background:rgb({r},{g},{b})">{}</span></td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td class="mono">{}</td></tr>"#,
            esc(cat_lbl),
            esc(&format_bytes(tree.logical_sizes[idx])),
            esc(&format_bytes(tree.allocated_sizes[idx])),
            if own.is_empty() { "—".into() } else { esc(own) },
            esc(&format_timestamp(tree.mtimes[idx])),
            esc(&tree.resolve_path(nid).to_string_lossy())
        );
    }
    let _ = writeln!(html, "</table></div></body></html>");

    let file = File::create(output_path)?;
    let mut writer = BufWriter::new(file);
    writer.write_all(html.as_bytes())?;
    writer.flush()?;

    Ok(nodes.len())
}
