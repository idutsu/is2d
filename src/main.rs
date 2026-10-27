mod canvas_tool;
mod panels;
mod timeline;
mod tool_paint;
mod tool_select;
mod tool_transform;
mod vector_engine;

use canvas_tool::{
    HoverTarget, SelectDragState, SelectSubMode, Tool, TransformDragState, TransformHoverTarget,
};
use eframe::egui::{self, Color32, CursorIcon, Key, Pos2, Sense, Stroke, Vec2};
use kurbo::Point;
use lyon_tessellation::FillTessellator;
use panels::PropertiesPanel;
use timeline::Timeline;
use vector_engine::FillShape;

const MAX_UNDO_HISTORY: usize = 50;

fn main() -> eframe::Result<()> {
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Clay Canvas — Flash Brush & Merge Vector Engine")
            .with_inner_size([1280.0, 820.0])
            .with_min_inner_size([800.0, 520.0]),
        ..Default::default()
    };

    eframe::run_native(
        "Clay Canvas",
        native_options,
        Box::new(|cc| {
            configure_japanese_fonts(&cc.egui_ctx);
            configure_sharp_light_theme(&cc.egui_ctx);
            Ok(Box::new(FlashBrushApp::default()))
        }),
    )
}

/// `fonts/` フォルダ内の日本語フォント（NotoSansJP）とアイコンフォント（MaterialIcons）を読み込む
fn configure_japanese_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();

    fonts.font_data.insert(
        "noto_sans_jp_regular".to_owned(),
        egui::FontData::from_static(include_bytes!("../fonts/NotoSansJP-Regular.ttf")),
    );
    fonts.font_data.insert(
        "noto_sans_jp_bold".to_owned(),
        egui::FontData::from_static(include_bytes!("../fonts/NotoSansJP-Bold.ttf")),
    );
    fonts.font_data.insert(
        "material_icons".to_owned(),
        egui::FontData::from_static(include_bytes!("../fonts/MaterialIcons-Regular.ttf")),
    );

    if let Some(proportional) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
        proportional.insert(0, "noto_sans_jp_regular".to_owned());
        proportional.insert(1, "noto_sans_jp_bold".to_owned());
        proportional.insert(2, "material_icons".to_owned());
    }
    if let Some(monospace) = fonts.families.get_mut(&egui::FontFamily::Monospace) {
        monospace.push("noto_sans_jp_regular".to_owned());
        monospace.push("material_icons".to_owned());
    }

    ctx.set_fonts(fonts);
}

/// アプリ全体のUIをライトテーマ＋角丸なし（すべて直角のシャープな矩形）に設定する
fn configure_sharp_light_theme(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::light();
    visuals.window_rounding = egui::Rounding::ZERO;
    visuals.menu_rounding = egui::Rounding::ZERO;
    visuals.widgets.noninteractive.rounding = egui::Rounding::ZERO;
    visuals.widgets.inactive.rounding = egui::Rounding::ZERO;
    visuals.widgets.hovered.rounding = egui::Rounding::ZERO;
    visuals.widgets.active.rounding = egui::Rounding::ZERO;
    visuals.widgets.open.rounding = egui::Rounding::ZERO;
    ctx.set_visuals(visuals);
}

// ============================================================================
// メインアプリケーション状態 & ショートカット制御
// ============================================================================

pub(crate) struct FlashBrushApp {
    pub(crate) tool: Tool,
    pub(crate) properties_panel: PropertiesPanel,
    pub(crate) brush_radius: f32,
    pub(crate) eraser_radius: f32,
    pub(crate) brush_color: Color32,
    pub(crate) show_wireframe: bool,

    /// キャンバスのズーム倍率（1.0 = 100%, 範囲 0.15..=16.0）
    pub(crate) view_zoom: f32,
    /// キャンバスのパンオフセット（スクリーンピクセル単位）
    pub(crate) view_pan: Vec2,
    /// スペースドラッグ / 中ボタンドラッグによるパン操作中の直前スクリーン座標
    pub(crate) pan_last_screen_pos: Option<Pos2>,

    /// タイムライン・レイヤー・キーフレーム・セル構造
    pub(crate) timeline: Timeline,

    pub(crate) shapes: Vec<FillShape>,
    pub(crate) undo_stack: Vec<(Timeline, Vec<FillShape>)>,
    pub(crate) redo_stack: Vec<(Timeline, Vec<FillShape>)>,

    pub(crate) active_stroke: Vec<Point>,
    pub(crate) active_raw_outline: Vec<Point>,

    pub(crate) selected_shapes: Vec<usize>,
    pub(crate) unsliced_canvas: Option<Vec<FillShape>>,
    pub(crate) selection_modified: bool,
    pub(crate) select_sub_mode: SelectSubMode,
    pub(crate) hover_target: HoverTarget,
    pub(crate) select_drag: SelectDragState,

    /// 現在インプレイス編集で潜っているグループ階層のパス（空 `[]` ならルートシーン）
    pub(crate) editing_group_path: Vec<u64>,
    /// 新規グループ化時に採番する一意のグループIDカウンタ
    pub(crate) next_group_id: u64,

    /// コピー / カットされたシェイプ群（`group_path` はコピー時の編集スコープからの相対パス）
    pub(crate) clipboard: Vec<FillShape>,
    /// `Cmd+V` による通常ペーストの連続回数（少しずつずらして配置するためのカウンタ）
    pub(crate) paste_count: usize,

    pub(crate) transform_pivot: Option<Point>,
    pub(crate) transform_hover: TransformHoverTarget,
    pub(crate) transform_drag: TransformDragState,

    pub(crate) tessellator: FillTessellator,
}

impl Default for FlashBrushApp {
    fn default() -> Self {
        Self {
            tool: Tool::Brush,
            properties_panel: PropertiesPanel::default(),
            brush_radius: 8.0,
            eraser_radius: 22.0,
            brush_color: Color32::from_rgb(40, 45, 55),
            show_wireframe: false,
            view_zoom: 1.0,
            view_pan: Vec2::ZERO,
            pan_last_screen_pos: None,
            timeline: Timeline::default(),
            shapes: Vec::new(),
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            active_stroke: Vec::new(),
            active_raw_outline: Vec::new(),
            selected_shapes: Vec::new(),
            unsliced_canvas: None,
            selection_modified: false,
            select_sub_mode: SelectSubMode::Reshape,
            hover_target: HoverTarget::None,
            select_drag: SelectDragState::None,
            editing_group_path: Vec::new(),
            next_group_id: 1,
            clipboard: Vec::new(),
            paste_count: 0,
            transform_pivot: None,
            transform_hover: TransformHoverTarget::None,
            transform_drag: TransformDragState::None,
            tessellator: FillTessellator::new(),
        }
    }
}

impl FlashBrushApp {
    // ------------------------------------------------------------------------
    // 1. Undo / Redo 履歴管理
    // ------------------------------------------------------------------------

    fn snapshot_timeline(&self) -> Timeline {
        let mut tl = self.timeline.clone();
        tl.is_playing = false;
        tl.last_frame_advance_time = None;
        tl
    }

    fn restore_state_snapshot(&mut self, tl: Timeline, shapes: Vec<FillShape>) {
        self.timeline = tl;
        self.shapes = shapes;
        self.select_drag = SelectDragState::None;
        self.transform_drag = TransformDragState::None;
        self.sanitize_editing_group_path();
    }

    pub(crate) fn record_undo_snapshot(&mut self, snapshot: Vec<FillShape>) {
        let mut tl_snap = self.snapshot_timeline();
        tl_snap.write_shapes_to_editable_cels(&snapshot);
        self.undo_stack.push((tl_snap, snapshot));
        if self.undo_stack.len() > MAX_UNDO_HISTORY {
            self.undo_stack.remove(0);
        }
        self.redo_stack.clear();
    }

    pub(crate) fn push_undo(&mut self) {
        self.record_undo_snapshot(self.shapes.clone());
    }

    pub(crate) fn undo(&mut self) {
        self.clear_selection();
        self.sync_shapes_to_timeline();
        if let Some((prev_tl, prev_shapes)) = self.undo_stack.pop() {
            let curr = (self.snapshot_timeline(), self.shapes.clone());
            self.redo_stack.push(curr);
            self.restore_state_snapshot(prev_tl, prev_shapes);
        }
    }

    pub(crate) fn redo(&mut self) {
        self.clear_selection();
        self.sync_shapes_to_timeline();
        if let Some((next_tl, next_shapes)) = self.redo_stack.pop() {
            let curr = (self.snapshot_timeline(), self.shapes.clone());
            self.undo_stack.push(curr);
            self.restore_state_snapshot(next_tl, next_shapes);
        }
    }

    // ------------------------------------------------------------------------
    // 2. キーボードショートカット制御
    // ------------------------------------------------------------------------

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.wants_keyboard_input() {
            return;
        }
        let mut copied_to_clipboard = false;
        ctx.input(|i| {
            let cmd = i.modifiers.command || i.modifiers.ctrl;
            let copy_event = i.events.iter().any(|ev| matches!(ev, egui::Event::Copy));
            let cut_event = i.events.iter().any(|ev| matches!(ev, egui::Event::Cut));
            let paste_event = i
                .events
                .iter()
                .any(|ev| matches!(ev, egui::Event::Paste(_)));

            if !cmd && !copy_event && !cut_event && !paste_event {
                self.handle_single_key_shortcuts(i, i.modifiers.shift);
            }

            if i.key_pressed(Key::Delete) || i.key_pressed(Key::Backspace) {
                self.delete_selected_shapes();
            }

            copied_to_clipboard =
                self.handle_command_shortcuts(i, cmd, copy_event, cut_event, paste_event);
        });
        if copied_to_clipboard {
            ctx.copy_text(String::from("clay_canvas"));
        }
    }

    fn handle_single_key_shortcuts(&mut self, i: &egui::InputState, shift: bool) {
        if i.key_pressed(Key::Enter) {
            self.toggle_playback();
        } else if i.key_pressed(Key::Comma) || i.key_pressed(Key::ArrowLeft) {
            self.step_frame(-1);
        } else if i.key_pressed(Key::Period) || i.key_pressed(Key::ArrowRight) {
            self.step_frame(1);
        } else if i.key_pressed(Key::Home) {
            self.switch_to_layer_and_frame(self.timeline.active_layer, 0);
        } else if i.key_pressed(Key::End) {
            let last = self.timeline.total_frames.saturating_sub(1);
            self.switch_to_layer_and_frame(self.timeline.active_layer, last);
        } else if i.key_pressed(Key::F6) {
            if shift {
                self.timeline_delete_keyframe();
            } else {
                self.timeline_insert_keyframe(true);
            }
        } else if i.key_pressed(Key::F7) {
            self.timeline_insert_keyframe(false);
        } else if i.key_pressed(Key::F5) {
            if shift {
                self.timeline_remove_frame();
            } else {
                self.timeline_extend_frame();
            }
        } else if i.key_pressed(Key::O) {
            self.timeline.onion_skin_enabled = !self.timeline.onion_skin_enabled;
        } else if let Some(tool) = Tool::from_shortcut_input(i) {
            self.set_tool(tool);
        } else if i.key_pressed(Key::Escape) {
            if !self.selected_shapes.is_empty() {
                self.clear_selection();
            } else {
                self.exit_current_group();
            }
            self.set_tool(Tool::Select);
        } else if self.tool == Tool::Select {
            let plus_pressed = i.key_pressed(Key::Plus)
                || i.key_pressed(Key::Equals)
                || i.key_pressed(Key::Semicolon)
                || i.events.iter().any(|ev| {
                    matches!(ev, egui::Event::Text(t) if t == "+" || t == "=" || t == ";" || t == "＋")
                });
            let minus_pressed = i.key_pressed(Key::Minus)
                || i.events.iter().any(|ev| {
                    matches!(ev, egui::Event::Text(t) if t == "-" || t == "ー" || t == "－")
                });

            if plus_pressed {
                self.toggle_select_sub_mode(SelectSubMode::AddAnchor);
            } else if minus_pressed {
                self.toggle_select_sub_mode(SelectSubMode::DeleteAnchor);
            }
        }
    }

    fn handle_command_shortcuts(
        &mut self,
        i: &egui::InputState,
        cmd: bool,
        copy_event: bool,
        cut_event: bool,
        paste_event: bool,
    ) -> bool {
        let mut copied = false;
        if copy_event || (cmd && !i.modifiers.shift && i.key_pressed(Key::C)) {
            copied = self.copy_selected_shapes();
        } else if cut_event || (cmd && !i.modifiers.shift && i.key_pressed(Key::X)) {
            copied = self.cut_selected_shapes();
        } else if paste_event || (cmd && i.key_pressed(Key::V)) {
            self.paste_clipboard(i.modifiers.shift);
        } else if cmd && i.key_pressed(Key::Num0) {
            self.reset_view();
        } else if cmd
            && (i.key_pressed(Key::Plus)
                || i.key_pressed(Key::Equals)
                || i.key_pressed(Key::Semicolon))
        {
            let center = i.pointer.hover_pos().unwrap_or(Pos2::new(640.0, 400.0));
            self.zoom_around_screen_pos(center, 1.25);
        } else if cmd && i.key_pressed(Key::Minus) {
            let center = i.pointer.hover_pos().unwrap_or(Pos2::new(640.0, 400.0));
            self.zoom_around_screen_pos(center, 0.8);
        } else if cmd && i.key_pressed(Key::G) {
            if i.modifiers.shift {
                self.ungroup_selected_shapes();
            } else {
                self.group_selected_shapes();
            }
        } else if cmd && i.key_pressed(Key::B) {
            self.ungroup_selected_shapes();
        } else if cmd && i.key_pressed(Key::Z) {
            if i.modifiers.shift {
                self.redo();
            } else {
                self.undo();
            }
        } else if cmd && i.key_pressed(Key::Y) {
            self.redo();
        }
        copied
    }

    // ------------------------------------------------------------------------
    // 3. 中央キャンバスのツール入力ディスパッチ・バッジ・カーソル更新
    // ------------------------------------------------------------------------

    fn is_current_tool_allowed(&self) -> bool {
        match self.tool {
            Tool::Eyedropper => true,
            Tool::Brush => self.timeline.is_active_layer_editable(),
            Tool::Eraser | Tool::Bucket | Tool::Select | Tool::Transform => {
                self.timeline.has_any_editable_layer()
            }
        }
    }

    fn handle_canvas_tool_input(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        mouse_pt: Option<Point>,
        tool_allowed: bool,
    ) {
        if self.timeline.is_playing {
            let canvas_interacted = (response.hovered()
                && ui.input(|i| i.pointer.primary_pressed()))
                || response.drag_started()
                || response.clicked();
            if canvas_interacted {
                self.stop_playback();
            } else {
                return;
            }
        }
        if self.tool == Tool::Eyedropper {
            let sampling = (response.hovered() && ui.input(|i| i.pointer.primary_pressed()))
                || response.dragged()
                || response.clicked();
            if sampling {
                if let Some(pt) = mouse_pt {
                    self.pick_color_at(pt);
                }
            }
        } else if tool_allowed {
            match self.tool {
                Tool::Brush | Tool::Eraser => {
                    self.handle_brush_or_eraser_input(response, mouse_pt);
                }
                Tool::Bucket => {
                    let just_pressed =
                        response.hovered() && ui.input(|i| i.pointer.primary_pressed());
                    if just_pressed {
                        if let Some(pt) = mouse_pt {
                            self.bucket_fill_at(pt);
                        }
                    }
                }
                Tool::Eyedropper => {}
                Tool::Select => {
                    self.handle_select_tool_input(ui, response, mouse_pt);
                }
                Tool::Transform => {
                    self.handle_transform_tool_input(ui, response, mouse_pt);
                }
            }
        }
    }

    fn draw_locked_layer_badge(&self, painter: &egui::Painter, canvas_rect: egui::Rect) {
        let badge_rect = egui::Rect::from_min_size(
            canvas_rect.left_top() + Vec2::new(12.0, 12.0),
            Vec2::new(340.0, 26.0),
        );
        painter.rect_filled(
            badge_rect,
            0.0,
            Color32::from_rgba_unmultiplied(255, 248, 225, 240),
        );
        painter.rect_stroke(
            badge_rect,
            0.0,
            Stroke::new(1.0, Color32::from_rgb(215, 165, 55)),
        );
        painter.text(
            badge_rect.center(),
            egui::Align2::CENTER_CENTER,
            "\u{e897} 現在のレイヤーはロック中または非表示のため編集できません",
            egui::FontId::proportional(12.0),
            Color32::from_rgb(145, 90, 10),
        );
    }

    fn update_canvas_cursor(
        &self,
        ctx: &egui::Context,
        painter: &egui::Painter,
        canvas_rect: egui::Rect,
        pointer_pos: Option<Pos2>,
        pan_state: (bool, bool),
        tool_allowed: bool,
    ) {
        let Some(pos) = pointer_pos else {
            return;
        };
        if !canvas_rect.contains(pos) {
            return;
        }
        let (is_panning, pan_gesture_active) = pan_state;
        if is_panning {
            ctx.set_cursor_icon(if pan_gesture_active {
                CursorIcon::Grabbing
            } else {
                CursorIcon::Grab
            });
        } else if !tool_allowed {
            ctx.set_cursor_icon(CursorIcon::NotAllowed);
        } else {
            self.draw_tool_cursor_and_feedback(ctx, painter, pos);
        }
    }
}

impl eframe::App for FlashBrushApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.update_playback(ctx);
        self.handle_shortcuts(ctx);
        self.draw_left_toolbar(ctx);
        self.draw_right_properties_panel(ctx);
        self.draw_timeline_panel(ctx);
        self.draw_stage_header_bar(ctx);

        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(Color32::from_rgb(244, 245, 248)))
            .show(ctx, |ui| {
                let (canvas_rect, response) =
                    ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
                let painter = ui.painter_at(canvas_rect);
                painter.rect_filled(canvas_rect, 0.0, Color32::from_rgb(250, 250, 252));

                let pointer_pos = response
                    .interact_pointer_pos()
                    .or_else(|| ctx.pointer_hover_pos());

                // 0. ビューポートのズーム・パン移動操作ハンドリング
                let pan_state @ (is_panning, _) =
                    self.handle_viewport_navigation(ui, canvas_rect, pointer_pos);

                let mouse_pt = pointer_pos.map(|p| self.screen_to_world(p));
                let tool_allowed = self.is_current_tool_allowed();

                // 1. ツール別のマウス入力ハンドリング（パン操作中はスキップ）
                if !is_panning {
                    self.handle_canvas_tool_input(ui, &response, mouse_pt, tool_allowed);
                }

                // キャンバス編集結果を現在フレームの各レイヤーセルへ即時同期
                self.sync_shapes_to_timeline();

                // 2. キャンバス上のシェイプ・オニオンスキン・選択枠・プレビューの描画
                self.draw_canvas_contents(&painter);

                if !tool_allowed {
                    self.draw_locked_layer_badge(&painter, canvas_rect);
                }

                // 3. カーソル表示とホバー・ドラッグ視覚フィードバック
                self.update_canvas_cursor(
                    ctx,
                    &painter,
                    canvas_rect,
                    pointer_pos,
                    pan_state,
                    tool_allowed,
                );
            });
    }
}

#[cfg(test)]
#[cfg(test)]
mod tests;
