pub mod diff_view;
pub mod duplicates_view;
pub mod inspector_panel;
pub mod largest_files;
pub mod layout_popup;
pub mod options_dialog;
pub mod permissions_view;
pub mod search_view;
pub mod storage_analytics;
pub mod sysadmin_audit;
pub mod target_selector;
pub mod tree_view;
pub mod watcher_view;

/// Render a clickable sortable table column header with a crisp `▼` / `▲` indicator.
/// Returns `true` when clicked.
pub fn sort_header_button(ui: &mut egui::Ui, label: &str, active: bool, desc: bool) -> bool {
    let text = if active {
        if desc {
            format!("{label} ▼")
        } else {
            format!("{label} ▲")
        }
    } else {
        label.to_string()
    };
    ui.selectable_label(active, egui::RichText::new(text).strong())
        .on_hover_text(mediastat_core::t(
            "Clic para ordenar ascendente / descendente",
            "Click to sort ascending / descending",
        ))
        .clicked()
}

/// Compute a centered initial position within the viewport for embedded fallback `egui::Window`.
pub fn centered_window_pos(ctx: &egui::Context, width: f32, height: f32) -> egui::Pos2 {
    let screen = ctx.screen_rect();
    let x = (screen.center().x - width * 0.5).max(screen.min.x + 12.0);
    let y = (screen.center().y - height * 0.5).max(screen.min.y + 32.0);
    egui::pos2(x, y)
}

/// Request that an open native OS subwindow be brought to the front and focused immediately
/// (without locking it permanently as Always-On-Top).
pub fn bring_native_window_to_front(ctx: &egui::Context, id: &str) {
    let focus_key = egui::Id::new(("native_vp_focus_stage", id));
    ctx.data_mut(|d| d.insert_temp::<u8>(focus_key, 0));
    ctx.request_repaint();
}

/// Configuration for an independent OS-level subwindow (`ctx.show_viewport_immediate`).
/// Users can freely drag it outside MediaStat's main window perimeter and onto other monitors.
pub struct NativeWindowConfig<'a> {
    pub id: &'a str,
    pub title: &'a str,
    pub default_size: [f32; 2],
    pub min_size: [f32; 2],
    pub resizable: bool,
    pub always_on_top: bool,
}

/// Render an independent native OS subwindow using `egui` multi-viewport (`show_viewport_immediate`).
/// Automatically brings newly opened windows to the front of the main window on Windows (via a
/// brief 1-frame z-order promotion + explicit focus, then immediately releasing back to `Normal`
/// level so it stays a normal free-floating OS window). Falls back to an embedded `egui::Window`
/// during headless tests or single-viewport backends.
pub fn show_native_window<R>(
    ctx: &egui::Context,
    cfg: NativeWindowConfig<'_>,
    open: &mut bool,
    mut add_contents: impl FnMut(&egui::Context, &mut egui::Ui, &mut bool) -> R,
) -> Option<R> {
    let pos_key = egui::Id::new(("native_vp_init_pos", cfg.id));
    let focus_key = egui::Id::new(("native_vp_focus_stage", cfg.id));
    if !*open {
        ctx.data_mut(|d| {
            d.remove::<Option<egui::Pos2>>(pos_key);
            d.remove::<u8>(focus_key);
        });
        return None;
    }

    // Compute initial position ONCE per open session (without nesting ctx locks!)
    // so dragging the main window never causes `eframe`'s ViewportBuilder diff to yank a child window.
    let init_pos: Option<egui::Pos2> =
        if let Some(cached) = ctx.data(|d| d.get_temp::<Option<egui::Pos2>>(pos_key)) {
            cached
        } else {
            let computed = ctx.input(|i| i.viewport().outer_rect).map(|parent| {
                egui::pos2(
                    (parent.center().x - cfg.default_size[0] * 0.5).max(16.0),
                    (parent.center().y - cfg.default_size[1] * 0.5).max(40.0),
                )
            });
            if computed.is_some() {
                ctx.data_mut(|d| d.insert_temp(pos_key, computed));
            }
            computed
        };

    // 2-stage front promotion on Windows:
    //   stage 0: create/raise at AlwaysOnTop + Focus (guarantees z-order strictly in front of ROOT)
    //   stage 1: immediately return to Normal (unless cfg.always_on_top) + Focus
    //   stage 2: steady normal OS window (draggable anywhere, never stuck above other apps)
    let focus_stage: u8 = ctx.data(|d| d.get_temp::<u8>(focus_key)).unwrap_or(0);
    let desired_level = if cfg.always_on_top || focus_stage == 0 {
        egui::WindowLevel::AlwaysOnTop
    } else {
        egui::WindowLevel::Normal
    };

    let mut builder = egui::ViewportBuilder::default()
        .with_title(cfg.title)
        .with_inner_size(cfg.default_size)
        .with_min_inner_size(cfg.min_size)
        .with_resizable(cfg.resizable)
        .with_active(true)
        .with_minimize_button(true)
        .with_maximize_button(cfg.resizable)
        .with_window_level(desired_level);

    if let Some(pos) = init_pos {
        builder = builder.with_position(pos);
    }

    let vp_id = egui::ViewportId::from_hash_of(cfg.id);
    let mut keep_open = *open;
    let mut result: Option<R> = None;
    let mut ran_native = false;

    ctx.show_viewport_immediate(vp_id, builder, |vp_ctx, class| {
        if class == egui::ViewportClass::Embedded {
            let mut close_now = false;
            let mut win_open = keep_open;
            egui::Window::new(cfg.title)
                .id(egui::Id::new(cfg.id))
                .open(&mut win_open)
                .movable(true)
                .collapsible(true)
                .resizable(cfg.resizable)
                .constrain(true)
                .default_pos(centered_window_pos(
                    vp_ctx,
                    cfg.default_size[0],
                    cfg.default_size[1],
                ))
                .default_size(cfg.default_size)
                .min_size(cfg.min_size)
                .show(vp_ctx, |ui| {
                    result = Some(add_contents(vp_ctx, ui, &mut close_now));
                });
            keep_open = win_open && !close_now;
        } else {
            ran_native = true;
            if focus_stage < 2 {
                vp_ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                vp_ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                vp_ctx.request_repaint();
            }
            if vp_ctx.input(|i| i.viewport().close_requested()) {
                keep_open = false;
            }
            let mut close_now = false;
            egui::CentralPanel::default().show(vp_ctx, |ui| {
                result = Some(add_contents(vp_ctx, ui, &mut close_now));
            });
            if close_now {
                keep_open = false;
            }
            if vp_ctx.input(|i| i.pointer.any_released() || !i.events.is_empty()) {
                vp_ctx.request_repaint_of(egui::ViewportId::ROOT);
            }
        }
    });

    if !keep_open {
        *open = false;
        ctx.data_mut(|d| {
            d.remove::<Option<egui::Pos2>>(pos_key);
            d.remove::<u8>(focus_key);
        });
        ctx.request_repaint_of(egui::ViewportId::ROOT);
    } else if ran_native && focus_stage < 2 {
        ctx.data_mut(|d| d.insert_temp::<u8>(focus_key, focus_stage + 1));
        ctx.request_repaint();
    }
    result
}
