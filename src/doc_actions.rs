use std::collections::HashSet;

use eframe::egui::{Color32, Vec2};
use kurbo::{Affine, ParamCurve, Point, QuadBez, Vec2 as KVec2};

use crate::canvas_tool::{
    active_scope_layer_id, cancel_active_drag, clear_selection, commit_floating,
    editable_layer_ids, focus_active_layer_by_id,
    focus_active_layer_to_shape_by_id, reset_session_after_timeline_change,
    select_shapes_for_timeline_selection, set_tool, show_notice, toggle_select_sub_mode,
    update_instances, Clipboard, CoalesceKey, HoverTarget, MapOwner, NoticeKind,
    SelectSubMode, StatusToast, Tab, Tool, ToolSettings, UndoEntry,
};
use crate::clipboard_ops::{
    content_xform, import_clipboard_symbols, pasteable_content, prepare_copy_selected_shapes,
    prepare_placement, put_placements, remap_timeline_ids, selected_content,
};
use crate::planar_map::{AnchorEdit, MapRule, ReshapeEdit};
use crate::select_ops::{
    apply_marquee_selection, compute_hover_target, enter_group_at, exit_current_group,
    prepare_arrange_selected, prepare_clear_active_frame_shapes, prepare_delete_selected_shapes,
    prepare_group_selected_shapes, prepare_ungroup_selected_shapes,
    retain_selection_and_flush_deselected, select_single_shape_at, toggle_shape_selection,
    ArrangeMode,
};
use crate::symbol::{ColorEffect, Document, SymbolInstanceBinding, SymbolLoopMode, TweenRotate};
use crate::symbol_ops::{
    enter_symbol_from_library, enter_symbol_timeline, exit_symbol_timeline,
    prepare_convert_selection_to_symbol, prepare_place_symbol_instance,
    prepare_swap_selected_instances, selected_single_symbol_info,
};
use crate::timeline::{Layer, Timeline, TimelineCellSelection};
use crate::vector_engine::{
    affine_rotate_around, affine_scale_around, rects_overlap, FillShape, LayerId,
};
use crate::vector_stroke::{input_step, stroke_shapes};

// ============================================================================
// 1. ActionHost / Action の定義
// ============================================================================

/// `apply` の対象となるホスト（`Is2dApp`）を抽象化するトレイト
pub(crate) trait ActionHost {
    fn tab(&self) -> &Tab;
    fn tab_mut(&mut self) -> &mut Tab;
    fn tool(&self) -> &ToolSettings;
    fn tool_mut(&mut self) -> &mut ToolSettings;
    fn clipboard_mut(&mut self) -> &mut Clipboard;
    fn toast_mut(&mut self) -> &mut Option<StatusToast>;
    fn parts_mut(
        &mut self,
    ) -> (
        &mut Tab,
        &mut ToolSettings,
        &mut Clipboard,
        &mut Option<StatusToast>,
    );
    fn stop_playback(&mut self);

    fn notify_error(&mut self, message: impl Into<String>) {
        show_notice(self.toast_mut(), NoticeKind::Error, message);
    }

    fn notify_info(&mut self, message: impl Into<String>) {
        show_notice(self.toast_mut(), NoticeKind::Info, message);
    }

    fn notify_warning(&mut self, message: impl Into<String>) {
        show_notice(self.toast_mut(), NoticeKind::Warning, message);
    }
}

/// ドキュメント状態または編集セッションを変更するすべての操作を表す列挙型
#[derive(Clone, Debug)]
pub(crate) enum Action {
    // 描画・塗り
    PaintStroke {
        points: Vec<Point>,
        radius: f32,
        color: Color32,
    },
    EraseStroke {
        points: Vec<Point>,
        radius: f32,
    },
    /// テスト用：閉じた折れ線 `raw`（NonZero で塗る）を、そのまま直線の輪郭の形にして、今のツールで塗る・消す
    #[cfg(test)]
    CommitRawOutline {
        raw: Vec<Point>,
    },
    BucketFill {
        pt: Point,
        color: Color32,
    },
    /// 鉛筆：マウスの点 `points`（ステージの座標）から作った線（`pencil_curves`）を、線 `stroke` で引く
    PencilStroke {
        points: Vec<Point>,
        stroke: crate::planar_map::EdgeStroke,
    },
    /// テスト用：線の区間 `segs`（ステージの座標）を、作業中のレイヤーの編集中の階層に引く（鉛筆と同じ道）
    #[cfg(test)]
    DrawLines {
        segs: Vec<crate::planar_map::LineSeg>,
    },
    /// インクボトル：点の線か、点を含む面の輪郭に、線 `stroke` を乗せる
    InkBottle {
        pt: Point,
        stroke: crate::planar_map::EdgeStroke,
    },

    // 選択・移動・グループ階層
    SelectSingleAt {
        pt: Point,
    },
    ToggleSelectionAt {
        scene_idx: usize,
    },
    RetainSelection {
        keep_indices: Vec<usize>,
    },
    SelectMarquee {
        p0: Point,
        p1: Point,
        additive: bool,
    },
    #[cfg(test)]
    SelectAll,
    Deselect,
    #[cfg(test)]
    CommitFloating,
    #[cfg(test)]
    MoveSelected {
        delta: KVec2,
    },
    NudgeSelection {
        delta: KVec2,
    },
    FinishMoveSelectionDrag {
        total_delta: KVec2,
        alt_duplicate: bool,
    },
    DeleteSelection,
    ClearActiveFrame,
    Group,
    Ungroup,
    Arrange {
        mode: ArrangeMode,
    },
    EnterGroupAt {
        pt: Point,
    },
    /// ダブルクリック：点の生の図形と、つながったものを選ぶ（線なら、つながった同じ線。塗りなら、輪郭の線も）。`add`（Shift）
    /// なら、今の選択に足す
    SelectConnectedAt {
        pt: Point,
        add: bool,
    },
    ExitGroupSteps {
        steps: usize,
    },

    // 輪郭変形・アンカー編集
    /// 境目の線で持つ地図（レイヤー `layer_id` の、編集中の階層の生の図形）で、形が `orig_seg` の辺の `grab_t` の所を
    /// `target_pos` へ引っぱる（形と位置は文書の座標。シンボルの中ではシンボルの座標）
    ReshapeEdge {
        layer_id: LayerId,
        grab_t: f64,
        target_pos: Point,
        orig_seg: QuadBez,
        /// まわりの辺もついてくる引っぱりか（`ReshapeEdit::PullEdge`）
        follow: bool,
    },
    /// 境目の線で持つ地図（レイヤー `layer_id` の、編集中の階層の生の図形）で、`orig_vertex` にある頂点を `target_pos` へ
    /// 動かす（位置は文書の座標）
    MoveVertex {
        layer_id: LayerId,
        orig_vertex: Point,
        target_pos: Point,
        /// まわりがついてくる引っぱりか（`ReshapeEdit::vertex`）
        follow: bool,
    },
    #[cfg(test)]
    PullEdgeFromTo {
        from: Point,
        to: Point,
    },
    #[cfg(test)]
    MoveVertexFromTo {
        from: Point,
        to: Point,
    },
    AddAnchor {
        pt: Point,
    },
    DeleteAnchor {
        pt: Point,
    },

    // 自由変形・ピボット
    #[cfg(test)]
    ScaleSelection {
        sx: f64,
        sy: f64,
    },
    FlipSelection {
        flip_x: bool,
        flip_y: bool,
    },
    RotateSelection {
        angle_rad: f64,
    },
    #[cfg(test)]
    SetTransformPivot {
        world_pivot: Option<Point>,
    },
    ResetTransformPivot,
    CommitTransformDrag {
        affine: Affine,
        orig_pivot: Point,
        had_custom_pivot: bool,
        changed: bool,
    },
    CommitPivotDrag {
        new_pivot: Point,
        orig_pivot: Point,
        had_custom_pivot: bool,
        changed: bool,
    },

    // クリップボード
    Copy,
    Cut,
    Paste {
        in_place: bool,
    },

    // シンボル・クラシックトゥイーン
    ConvertToSymbol,
    PlaceSymbol {
        sym_id: u64,
    },
    /// 今のコマの、編集中の階層のインスタンス `instance_gid` のシンボルに入る
    EnterSymbolTimeline {
        instance_gid: u64,
    },
    EnterSymbolFromLibrary {
        sym_id: u64,
    },
    #[cfg(test)]
    ExitSymbolTimeline,
    #[cfg(test)]
    ExitSymbolToDepth {
        depth: usize,
    },
    RenameSymbol {
        sym_id: u64,
        new_name: String,
    },
    DeleteSymbol {
        sym_id: u64,
    },
    DuplicateSymbol {
        sym_id: u64,
    },
    SwapSymbol {
        sym_id: u64,
    },
    SetSymbolPlayback {
        loop_mode: Option<SymbolLoopMode>,
        first_frame: Option<usize>,
    },
    SetSymbolColorEffect {
        effect: ColorEffect,
    },
    ToggleTween,
    SetTweenRotate {
        rotate: TweenRotate,
    },

    // タイムライン・レイヤー・ステージ設定
    SwitchLayerAndFrame {
        layer_idx: usize,
        frame: usize,
    },
    ScrubRulerToFrame {
        frame: usize,
    },
    SelectTimelineCells {
        l0: usize,
        f0: usize,
        l1: usize,
        f1: usize,
    },
    ClearTimelineCells,
    /// 音（名前 `name`・WAV か MP3 のバイト `bytes`）を音のライブラリに入れ、今のコマの音を付けられるキーフレーム
    /// （`Layer::sound_slot_at`）に、頭から終わりまで付ける
    ImportSound {
        name: String,
        bytes: std::sync::Arc<[u8]>,
    },
    /// 今のコマの音を付けられるキーフレームに、音のライブラリの番号 `sound` の音を頭から終わりまで付ける（付け替える）。
    /// `None` は音を外す
    SetKeyframeSound {
        sound: Option<u64>,
    },
    /// 音のライブラリの音 `sound_id` の名前を変える（続けて打った文字は1つの取り消しにまとめる）
    RenameSound {
        sound_id: u64,
        new_name: String,
    },
    /// 音のライブラリから音 `sound_id` を消す。その音を付けたキーフレーム（ルートと全シンボル）は空白に戻る
    DeleteSound {
        sound_id: u64,
    },
    /// 作業中のタイムラインのレイヤー `layer` の、コマ `kf_start` に始まるキーフレームの音の区間の端 `edge` を、コマの
    /// 境目 `frame` へ動かす（`Layer::trim_sound`。タイムラインの波形の端のドラッグを離したとき）
    TrimSound {
        layer: usize,
        kf_start: usize,
        edge: crate::sound::SoundEdge,
        frame: usize,
    },
    InsertKeyframe {
        copy_prev: bool,
    },
    DeleteKeyframe,
    ExtendFrame,
    RemoveFrame,
    MoveOrCopyFrames {
        sel: TimelineCellSelection,
        layer_delta: isize,
        frame_delta: isize,
        copy_mode: bool,
    },
    AddLayer,
    DeleteActiveLayer,
    ReorderLayer {
        src_idx: usize,
        dst_idx: usize,
    },
    SetLayerVisible {
        layer_idx: usize,
        visible: bool,
    },
    SetLayerLocked {
        layer_idx: usize,
        locked: bool,
    },
    /// 今のタイムラインのレイヤー `layer_id` の名前を変える（続けて打った文字は1つの取り消しにまとめる）
    RenameLayer {
        layer_id: u64,
        new_name: String,
    },
    SetFps {
        fps: u32,
    },
    SetStageSize {
        size: Vec2,
    },

    // ツール・サブモード・Undo/Redo・Coalesce区切り
    SetTool {
        tool: Tool,
    },
    ToggleSelectSubMode {
        mode: SelectSubMode,
    },
    Undo,
    Redo,
    EndCoalesce {
        key: Option<CoalesceKey>,
    },
}

impl Action {
    /// 連続する同一種の変更を1つのUndoステップにまとめるための `CoalesceKey` を返す
    pub(crate) fn coalesce_key(&self, tab: &Tab) -> Option<CoalesceKey> {
        match self {
            Self::SetStageSize { .. } => Some(CoalesceKey::StageSize),
            Self::SetFps { .. } => Some(CoalesceKey::Fps),
            Self::RenameSymbol { sym_id, .. } => Some(CoalesceKey::RenameSymbol(*sym_id)),
            Self::RenameSound { sound_id, .. } => Some(CoalesceKey::RenameSound(*sound_id)),
            Self::RenameLayer { layer_id, .. } => Some(CoalesceKey::RenameLayer(*layer_id)),
            Self::SetSymbolPlayback {
                loop_mode,
                first_frame: Some(_),
            } => selected_single_symbol_info(tab).and_then(|(b, _, _)| {
                if loop_mode.is_none_or(|m| m == b.loop_mode) {
                    Some(CoalesceKey::SymbolPlayback(b.instance_gid))
                } else {
                    None
                }
            }),
            Self::SetSymbolColorEffect { .. } => selected_single_symbol_info(tab)
                .map(|(b, _, _)| CoalesceKey::SymbolColorEffect(b.instance_gid)),
            Self::NudgeSelection { .. } => Some(CoalesceKey::NudgeSelection),
            _ => None,
        }
    }
}

// ============================================================================
// 2. Undo 記録・Dirty 更新（このモジュール内でのみ呼び出し可能）
// ============================================================================

/// 今の状態の控え（絵・浮いた片・選択・階層など。中身は文書と共有するので軽い）
fn undo_snapshot(tab: &Tab) -> UndoEntry {
    let floating = tab.history.snapshot_floating(tab.session.floating());
    UndoEntry {
        timeline: tab.doc.root.clone(),
        // ライブラリは文書と共有する（書く所だけ `Arc::make_mut` で写す）
        library: tab.doc.library.clone(),
        // 音のライブラリも文書と共有する
        sounds: tab.doc.sounds.clone(),
        floating,
        selection: tab.session.selection.clone(),
        edit_path: tab.session.edit_path.clone(),
        editing_group_path: tab.session.editing_group_path.clone(),
        stage_size: tab.doc.stage_size,
        fps: tab.doc.fps,
        symbols_revision: tab.doc.revision,
        commit_id: tab.history.current_commit_id,
    }
}

/// 控えをしまう（`apply` の末尾で、操作ができたときだけ積む）。しまうのは、操作を始める前の状態（`UndoHistory::before`。
/// `apply` の外（テストが中の関数を直接呼ぶとき）は、今の状態）
fn push_undo_entry_from_doc(tab: &mut Tab, coalesce: Option<CoalesceKey>) {
    if tab.history.pending.is_some() {
        return;
    }
    let entry = match tab.history.before.take() {
        Some(entry) => entry,
        None => undo_snapshot(tab),
    };
    tab.history.pending = Some((entry, coalesce));
}

fn push_undo(tab: &mut Tab, coalesce: Option<CoalesceKey>) {
    if let Some(key) = coalesce {
        if tab.history.last_coalesce_key == Some(key) {
            return;
        }
    }
    push_undo_entry_from_doc(tab, coalesce);
}

/// 音を読み込んで、今のコマの音を付けられるキーフレーム（`Timeline::active_sound_slot`）に頭から終わりまで付ける。
/// 付けられる所が無ければ断る。波形にできないバイト・長さが 0 の音は、知らせて断る（文書は変えない）
fn apply_import_sound(
    tab: &mut Tab,
    toast: &mut Option<StatusToast>,
    name: String,
    bytes: std::sync::Arc<[u8]>,
) -> bool {
    if tab.active_timeline().active_sound_slot().is_none() {
        return false;
    }
    let sound = match crate::sound::Sound::new(tab.doc.next_sound_id, name, bytes) {
        Ok(s) => s,
        Err(e) => {
            show_notice(toast, NoticeKind::Error, e);
            return false;
        }
    };
    let Some(clip) = crate::sound::SoundClip::whole(&sound) else {
        show_notice(toast, NoticeKind::Error, "音の長さが 0 です");
        return false;
    };
    set_active_keyframe_sound(tab, Some(clip), Some(sound))
}

/// 今のコマの音を付けられるキーフレーム（`Timeline::active_sound_slot`）に、音のライブラリの番号 `sound` の音を頭から
/// 終わりまで付ける（付いていれば付け替える）。`None` なら音を外す（キーフレームは空白に戻る）。付けられる所が無い・
/// 無い番号・変わらないなら断る
fn apply_set_keyframe_sound(tab: &mut Tab, sound: Option<u64>) -> bool {
    let clip = match sound {
        Some(id) => {
            let Some(s) = tab.doc.sounds.iter().find(|s| s.id == id) else {
                return false;
            };
            let Some(clip) = crate::sound::SoundClip::whole(s) else {
                return false;
            };
            Some(clip)
        }
        None => None,
    };
    set_active_keyframe_sound(tab, clip, None)
}

/// 今のコマの音を付けられるキーフレームの音を `clip` にする（取り消しの1歩）。新しく読み込んだ音 `added` は、同じ1歩で
/// 音のライブラリに足す（付く番号は次の音の番号 `next_sound_id`：`clip` はその番号で作ってある）。付けられる所が無い・
/// 変わらないなら断る
fn set_active_keyframe_sound(
    tab: &mut Tab,
    clip: Option<crate::sound::SoundClip>,
    added: Option<crate::sound::Sound>,
) -> bool {
    mutate_timeline_with_undo(tab, |tl, doc| {
        let Some((l, k)) = tl.active_sound_slot() else {
            return false;
        };
        if tl.layers[l].keyframes[k].sound == clip {
            return false;
        }
        tl.layers[l].keyframe_mut(k).sound = clip;
        if let Some(s) = added {
            doc.add_sound(s);
        }
        true
    })
}

fn mutate_timeline_with_undo(
    tab: &mut Tab,
    mutate: impl FnOnce(&mut Timeline, &mut Document) -> bool,
) -> bool {
    if !commit_floating(tab) {
        return false;
    }
    push_undo(tab, None);
    let changed = tab.with_active_timeline_mut(mutate);
    if changed {
        reset_session_after_timeline_change(tab);
    }
    changed
}

// ============================================================================
// 3. 単一エントリポイント `apply`
// ============================================================================

/// すべての文書変更・編集操作の唯一の入口。
/// 再生停止・Undo記録・連続操作のCoalesce・タイムライン同期・Dirty更新を一元管理する。
pub(crate) fn apply(host: &mut impl ActionHost, action: Action) -> bool {
    debug_assert!(host.tab().history.pending.is_none());
    if !matches!(action, Action::SetFps { .. } | Action::EndCoalesce { .. }) {
        host.stop_playback();
        // 画面の土台を最新にする（ここ1か所。`apply` の中で土台を読む所（浮いた片の確定 `plan_commit` など）は作り直さ
        // ない。文書を変えた所は、後始末（`finish_cel_writes`・`with_active_timeline_mut` など）でその場で作り直す）
        host.tab_mut().refresh_scene_base();
    }
    // 操作を始める前の状態（控えにしまうのはこれ：`UndoHistory::before`）
    let before = undo_snapshot(host.tab());
    host.tab_mut().history.before = Some(before);
    let check_symbol_nest = matches!(
        action,
        Action::ConvertToSymbol
            | Action::Paste { .. }
            | Action::PlaceSymbol { .. }
            | Action::SwapSymbol { .. }
    );
    let prev_ids = (host.tab().doc.next_symbol_id, host.tab().doc.next_group_id);
    let prev_imported_map =
        matches!(action, Action::Paste { .. }).then(|| host.parts_mut().2.imported_map.clone());
    // 操作の途中で止まったら（panic）、その操作を無かったことにする（`restore_after_panic`）
    let mut res = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| dispatch(host, action))) {
        Ok(res) => res,
        Err(_) => {
            restore_after_panic(host);
            false
        }
    };
    let (tab, _, clipboard, toast) = host.parts_mut();
    // 断った操作は、何も変えない（全部か無しか）：途中まで書いていても、操作の前に戻す（`restore_state_before_action`。
    // 地図にできない形を作る編集は、ふつう文書を変える前に断ってあり、そのときは戻しても同じ）。前は、断る所が先に書いた
    // ものを各操作が自分で戻す約束だったので、グループの変形・削除で、断られた所だけが黙って残った（レビュー de92ceb の
    // N-4001・設計審査 de92ceb の D-5）
    if let Some(refusal) = tab.session.refusal.take() {
        if !res {
            restore_state_before_action(tab);
        }
        show_notice(toast, NoticeKind::Warning, refusal.message());
    }
    tab.history.before = None;
    if check_symbol_nest && res && tab.doc.symbol_nest_exceeded() {
        restore_state_before_action(tab);
        tab.ensure_valid_library_selection();
        show_notice(toast, NoticeKind::Warning, "シンボルの入れ子は8段までです");
        res = false;
    }
    if res {
        if let Some((entry, coalesce)) = tab.history.pending.take() {
            tab.history.push(entry);
            tab.history.last_coalesce_key = coalesce;
        }
    } else {
        tab.history.pending = None;
        (tab.doc.next_symbol_id, tab.doc.next_group_id) = prev_ids;
        if let Some(map) = prev_imported_map {
            clipboard.imported_map = map;
        }
    }
    res
}

/// 操作を無かったことにする（戻し方はこれ1つ：断った操作・入れ子の深すぎるシンボル・途中で止まった操作）：文書・選択・
/// 浮いた片を、操作を始める前の状態にする（控えにしまう前なら `UndoHistory::before`、しまったあとなら控えの候補
/// `pending`。どちらも `apply` が操作の前に1回だけ写したもの。取り消しと同じ戻し方 `restore_state_snapshot`）。控えは
/// 積まない。戻す途中で記された知らせ（戻した浮いた片の確定など）は捨てる（次の操作の知らせにしない）
fn restore_state_before_action(tab: &mut Tab) {
    let before = tab.history.before.take().or_else(|| tab.history.pending.take().map(|(e, _)| e));
    tab.history.pending = None;
    if let Some(before) = before {
        tab.restore_state_snapshot(before);
    }
    tab.refresh_scene_base();
    tab.session.refusal = None;
}

/// 操作の途中で止まった（panic：計算の前提が破れた）ときの最後の砦（設計審査 de92ceb の D-2）：途中まで作り直したかも
/// しれない画面の土台を捨て、操作の前の状態に戻し（`restore_state_before_action`）、知らせる。別のやり方へ逃げるのでは
/// なく、その操作を無かったことにするだけ。文書は
/// 共有の写しで、操作は書き終えた値を置き換えるだけなので、戻した文書は操作の前のまま。前は止まるとアプリが落ち、全部の
/// タブの保存していない作業が消えた
fn restore_after_panic(host: &mut impl ActionHost) {
    let (tab, _, _, toast) = host.parts_mut();
    tab.session.base_cache = Default::default();
    restore_state_before_action(tab);
    show_notice(
        toast,
        NoticeKind::Error,
        "内部の不具合でこの操作を続けられなかったので、操作の前に戻しました",
    );
}

fn dispatch(host: &mut impl ActionHost, action: Action) -> bool {
    let (tab, tool, clipboard, toast) = host.parts_mut();
    let coalesce = action.coalesce_key(tab);

    match action {
        Action::PaintStroke {
            points,
            radius,
            color,
        } => {
            if points.is_empty() {
                return false;
            }
            let shapes = stroke_shapes_in_view(tab, &points, radius, color);
            apply_stroke_shapes(tab, Tool::Brush, shapes)
        }
        Action::EraseStroke { points, radius } => {
            if points.is_empty() {
                return false;
            }
            let shapes = stroke_shapes_in_view(tab, &points, radius, tool.brush_color);
            apply_stroke_shapes(tab, Tool::Eraser, shapes)
        }
        #[cfg(test)]
        Action::CommitRawOutline { raw } => {
            let shapes = crate::vector_stroke::polygon_shapes(&raw, tool.brush_color);
            apply_stroke_shapes(tab, tool.tool, shapes)
        }
        Action::BucketFill { pt, color } => apply_bucket_fill(tab, color, pt),
        Action::InkBottle { pt, stroke } => apply_ink_bottle(tab, stroke, pt),
        #[cfg(test)]
        Action::DrawLines { segs } => apply_draw_lines(tab, segs),
        Action::PencilStroke { points, stroke } => {
            let zoom = tab.view.zoom() as f64;
            let step = input_step(zoom, tab.view.pixels_per_point as f64);
            let segs = crate::vector_stroke::pencil_curves(&points, step, zoom)
                .into_iter()
                .map(|q| (q, stroke))
                .collect();
            apply_draw_lines(tab, segs)
        }

        Action::SelectSingleAt { pt } => select_single_shape_at(tab, tool.select_sub_mode, pt),
        Action::ToggleSelectionAt { scene_idx } => {
            toggle_shape_selection(tab, scene_idx);
            true
        }
        Action::RetainSelection { keep_indices } => {
            retain_selection_and_flush_deselected(tab, &keep_indices);
            true
        }
        Action::SelectMarquee { p0, p1, additive } => {
            apply_marquee_selection(tab, p0, p1, additive);
            !tab.session.selection.is_empty()
        }
        #[cfg(test)]
        Action::SelectAll => {
            if !clear_selection(tab) {
                return false;
            }
            let editable_lids: std::collections::HashSet<u64> =
                editable_layer_ids(tab).into_iter().collect();
            tab.session.select_shapes_in_layers(&editable_lids);
            !tab.session.selection.is_empty()
        }
        Action::Deselect => clear_selection(tab),
        #[cfg(test)]
        Action::CommitFloating => commit_floating(tab),
        #[cfg(test)]
        Action::MoveSelected { delta } => apply_move_selected(tab, delta, None),
        Action::NudgeSelection { delta } => apply_move_selected(tab, delta, coalesce),
        Action::FinishMoveSelectionDrag {
            total_delta,
            alt_duplicate,
        } => {
            let done = apply_finish_move_selection_drag(tab, total_delta, alt_duplicate);
            tab.session.drop_ahead_done();
            done
        }
        Action::DeleteSelection => prepare_delete_selected_shapes(tab, |tab| push_undo(tab, None)),
        Action::ClearActiveFrame => {
            prepare_clear_active_frame_shapes(tab, |tab| push_undo(tab, None))
        }
        Action::Group => prepare_group_selected_shapes(tab, |tab| push_undo(tab, None)),
        Action::Ungroup => prepare_ungroup_selected_shapes(tab, |tab| push_undo(tab, None)),
        Action::Arrange { mode } => prepare_arrange_selected(tab, mode, |tab| push_undo(tab, None)),
        Action::EnterGroupAt { pt } => enter_group_at(tab, pt),
        Action::SelectConnectedAt { pt, add } => crate::select_ops::select_connected_at(tab, pt, add),
        Action::ExitGroupSteps { steps } => {
            let mut exited = false;
            for _ in 0..steps {
                if !exit_current_group(tab) {
                    break;
                }
                exited = true;
            }
            exited
        }

        Action::ReshapeEdge {
            layer_id,
            grab_t,
            target_pos,
            orig_seg,
            follow,
        } => apply_map_reshape(
            tab,
            layer_id,
            (orig_seg.eval(grab_t), target_pos),
            ReshapeEdit::PullEdge {
                orig_seg,
                grab_t,
                target: target_pos,
                follow,
            },
        ),
        Action::MoveVertex {
            layer_id,
            orig_vertex,
            target_pos,
            follow,
        } => apply_map_reshape(
            tab,
            layer_id,
            (orig_vertex, target_pos),
            ReshapeEdit::vertex(orig_vertex, target_pos, follow),
        ),
        #[cfg(test)]
        Action::PullEdgeFromTo { from, to } => apply_pull_edge_from_to(tab, from, to),
        #[cfg(test)]
        Action::MoveVertexFromTo { from, to } => apply_move_vertex_from_to(tab, from, to),
        Action::AddAnchor { pt } => apply_anchor_edit_at(tab, pt, true),
        Action::DeleteAnchor { pt } => apply_anchor_edit_at(tab, pt, false),

        #[cfg(test)]
        Action::ScaleSelection { sx, sy } => {
            let Some((_, pivot)) = tab.session.selection_bbox_and_pivot(tab.view.zoom()) else {
                return false;
            };
            apply_affine_to_selected_inner(tab, affine_scale_around(pivot, sx, sy))
        }
        Action::FlipSelection { flip_x, flip_y } => {
            let Some((_, pivot)) = tab.session.selection_bbox_and_pivot(tab.view.zoom()) else {
                return false;
            };
            let sx = if flip_x { -1.0 } else { 1.0 };
            let sy = if flip_y { -1.0 } else { 1.0 };
            apply_affine_to_selected_inner(tab, affine_scale_around(pivot, sx, sy))
        }
        Action::RotateSelection { angle_rad } => {
            let Some((_, pivot)) = tab.session.selection_bbox_and_pivot(tab.view.zoom()) else {
                return false;
            };
            apply_affine_to_selected_inner(tab, affine_rotate_around(pivot, angle_rad))
        }
        #[cfg(test)]
        Action::SetTransformPivot { world_pivot } => {
            apply_set_selected_symbol_world_pivot(tab, world_pivot)
        }
        Action::ResetTransformPivot => {
            let mut changed = tab.session.transform_pivot.take().is_some();
            if apply_set_selected_symbol_world_pivot(tab, None) {
                changed = true;
            }
            changed
        }
        Action::CommitTransformDrag {
            affine,
            orig_pivot,
            had_custom_pivot,
            changed,
        } => {
            tab.session.clear_overlay();
            tab.session.transform_pivot = had_custom_pivot.then_some(orig_pivot);
            let done = if changed && !tab.session.selection.is_empty() {
                let applied = apply_affine_to_selected_inner(tab, affine);
                if applied && had_custom_pivot && tab.session.transform_pivot.is_some() {
                    tab.session.transform_pivot = Some(affine * orig_pivot);
                }
                applied
            } else {
                false
            };
            tab.session.drop_ahead_done();
            done
        }
        Action::CommitPivotDrag {
            new_pivot,
            orig_pivot,
            had_custom_pivot,
            changed,
        } => {
            tab.session.clear_overlay();
            if changed {
                if apply_set_selected_symbol_world_pivot(tab, Some(new_pivot)) {
                    tab.session.transform_pivot = None;
                } else {
                    tab.session.transform_pivot = Some(new_pivot);
                }
                true
            } else {
                tab.session.transform_pivot = had_custom_pivot.then_some(orig_pivot);
                false
            }
        }

        Action::Copy => prepare_copy_selected_shapes(tab, clipboard),
        Action::Cut => {
            if !prepare_copy_selected_shapes(tab, clipboard) {
                return false;
            }
            prepare_delete_selected_shapes(tab, |tab| push_undo(tab, None))
        }
        Action::Paste { in_place } => apply_paste_clipboard(tab, tool, clipboard, toast, in_place),

        Action::ConvertToSymbol => {
            if let Some((sym_id, target_lid)) =
                prepare_convert_selection_to_symbol(tab, |tab| push_undo(tab, None))
            {
                tab.view.selected_library_symbol = Some(sym_id);
                if let Some(l_idx) = tab.active_timeline().layer_index_by_id(target_lid) {
                    tab.set_active_layer(l_idx);
                }
                tab.refresh_scene_base();
                tab.session.set_floating(None);
                tab.session.transform_pivot = None;
                tab.view.inspect_timeline_frame = false;
                true
            } else {
                false
            }
        }
        Action::PlaceSymbol { sym_id } => {
            if prepare_place_symbol_instance(tab, sym_id, |tab| push_undo(tab, None)) {
                tool_for_placed(tab, tool);
                true
            } else {
                false
            }
        }
        Action::EnterSymbolTimeline { instance_gid } => enter_symbol_timeline(tab, instance_gid),
        Action::EnterSymbolFromLibrary { sym_id } => enter_symbol_from_library(tab, sym_id),
        #[cfg(test)]
        Action::ExitSymbolTimeline => exit_symbol_timeline(tab),
        #[cfg(test)]
        Action::ExitSymbolToDepth { depth } => {
            let mut exited = false;
            while tab.session.edit_path.len() > depth {
                if !exit_symbol_timeline(tab) {
                    break;
                }
                exited = true;
            }
            exited
        }
        Action::RenameSymbol { sym_id, new_name } => {
            push_undo(tab, coalesce);
            tab.doc.rename_symbol(sym_id, new_name)
        }
        Action::DeleteSymbol { sym_id } => apply_delete_symbol(tab, sym_id),
        Action::DuplicateSymbol { sym_id } => apply_duplicate_symbol(tab, sym_id),
        Action::SwapSymbol { sym_id } => {
            prepare_swap_selected_instances(tab, sym_id, |tab| push_undo(tab, None))
        }
        Action::SetSymbolPlayback {
            loop_mode,
            first_frame,
        } => apply_set_symbol_playback(tab, loop_mode, first_frame, coalesce),
        Action::SetSymbolColorEffect { effect } => {
            update_selected_symbol_binding(tab, coalesce, |b| b.color_effect = effect)
        }
        Action::ToggleTween => apply_timeline_toggle_tween(tab, toast),
        Action::SetTweenRotate { rotate } => apply_set_tween_rotate(tab, rotate),

        Action::SwitchLayerAndFrame { layer_idx, frame } => {
            apply_switch_to_layer_and_frame(tab, layer_idx, frame);
            true
        }
        Action::ScrubRulerToFrame { frame } => {
            apply_scrub_ruler_to_frame(tab, frame);
            true
        }
        Action::SelectTimelineCells { l0, f0, l1, f1 } => {
            apply_select_timeline_cells(tab, l0, f0, l1, f1);
            true
        }
        Action::ClearTimelineCells => {
            tab.set_active_cell_selection(None);
            true
        }
        Action::ImportSound { name, bytes } => apply_import_sound(tab, toast, name, bytes),
        Action::SetKeyframeSound { sound } => apply_set_keyframe_sound(tab, sound),
        Action::RenameSound { sound_id, new_name } => {
            push_undo(tab, coalesce);
            tab.doc.rename_sound(sound_id, new_name)
        }
        Action::DeleteSound { sound_id } => {
            push_undo(tab, None);
            tab.doc.remove_sound(sound_id)
        }
        Action::TrimSound {
            layer,
            kf_start,
            edge,
            frame,
        } => mutate_timeline_with_undo(tab, |tl, doc| {
            let Some(l) = tl.layers.get_mut(layer).filter(|l| l.accepts_frame_ops()) else {
                return false;
            };
            let Some(k) = l.keyframe_index_starting_at(kf_start) else {
                return false;
            };
            l.trim_sound(k, edge, frame, &doc.sounds, doc.fps)
        }),
        Action::InsertKeyframe { copy_prev } => apply_timeline_insert_keyframe(tab, copy_prev),
        Action::DeleteKeyframe => apply_timeline_delete_keyframe(tab),
        Action::ExtendFrame => apply_timeline_extend_frame(tab),
        Action::RemoveFrame => apply_timeline_remove_frame(tab),
        Action::MoveOrCopyFrames {
            sel,
            layer_delta,
            frame_delta,
            copy_mode,
        } => apply_timeline_move_or_copy_frames(tab, sel, layer_delta, frame_delta, copy_mode),
        Action::AddLayer => apply_timeline_add_layer(tab),
        Action::DeleteActiveLayer => apply_timeline_delete_active_layer(tab),
        Action::ReorderLayer { src_idx, dst_idx } => {
            apply_timeline_reorder_layer(tab, src_idx, dst_idx)
        }
        Action::SetLayerVisible { layer_idx, visible } => {
            apply_set_layer_flag(tab, layer_idx, visible, |l| l.visible, |l| &mut l.visible)
        }
        Action::SetLayerLocked { layer_idx, locked } => {
            apply_set_layer_flag(tab, layer_idx, locked, |l| l.locked, |l| &mut l.locked)
        }
        Action::RenameLayer { layer_id, new_name } => {
            push_undo(tab, coalesce);
            tab.with_active_timeline_mut(|tl, _| tl.rename_layer(layer_id, new_name))
        }
        Action::SetFps { fps } => apply_set_fps(tab, fps, coalesce),
        Action::SetStageSize { size } => apply_set_stage_size(tab, size, coalesce),

        Action::SetTool { tool: target_tool } => {
            set_tool(tab, tool, target_tool);
            true
        }
        Action::ToggleSelectSubMode { mode } => {
            toggle_select_sub_mode(tab, tool, mode);
            true
        }
        Action::Undo => tab.undo(),
        Action::Redo => tab.redo(),
        Action::EndCoalesce { key } => {
            if key.is_none_or(|k| tab.history.last_coalesce_key == Some(k)) {
                tab.history.end_coalesce();
            }
            true
        }
    }
}

// ============================================================================
// 4. 各アクションの内部実装
// ============================================================================

/// ブラシ・消しゴムの点 `points`（ステージの座標）と半径から、線の形を作る。丸めの刻みと当てはめの許容は、今の表示の
/// 倍率と画面の倍率で決める（描いている間の形と同じ：`tool_paint::handle_brush_or_eraser_input`）
fn stroke_shapes_in_view(
    tab: &Tab,
    points: &[Point],
    radius: f32,
    color: Color32,
) -> Vec<FillShape> {
    let zoom = tab.view.zoom() as f64;
    let step = input_step(zoom, tab.view.pixels_per_point as f64);
    stroke_shapes(points, radius as f64, step, zoom, color)
}

/// ブラシ・消しゴムの線の形 `shapes` を、今のレイヤー・階層の地図に塗る・消す
fn apply_stroke_shapes(tab: &mut Tab, active_tool: Tool, shapes: Vec<FillShape>) -> bool {
    let target_lid = active_scope_layer_id(&tab.session, tab.active_timeline());
    if !tab.active_timeline().is_layer_editable_by_id(target_lid) {
        return false;
    }
    let brush_color = shapes.first().map_or(Color32::BLACK, |s| s.color);
    let stroke_shapes: Vec<FillShape> = shapes
        .into_iter()
        .map(|s| s.with_scope(target_lid, &tab.session.editing_group_path))
        .collect();
    if stroke_shapes.is_empty() {
        return false;
    }
    let is_brush = active_tool == Tool::Brush;
    // 消しゴムは、消す線がどの図形にもかからなければ何もしない
    if !is_brush
        && !tab.scene().iter().any(|s| {
            s.layer_id == target_lid
                && s.is_raw_at_scope(&tab.session.editing_group_path)
                && stroke_shapes
                    .iter()
                    .any(|er| rects_overlap(s.bounding_box(), er.bounding_box()))
        })
    {
        return false;
    }
    if !clear_selection(tab) {
        return false;
    }
    // 絵が変わらなければ（同じ色で塗られた所の中をなぞった・何も消えないなど）write_local が false を返し、
    // apply が Undo の控えを捨てる
    push_undo(tab, None);
    let rule = MapRule::Paint(is_brush.then_some(brush_color));
    // 地図は文書の座標なので、線の形も文書の座標へ写す。格子に置けない所（±65,536px の外）の線は描かない
    let world = tab.session.edit_path.world_xform();
    let strokes: Vec<FillShape> = stroke_shapes
        .iter()
        .map(|s| s.to_doc(world).into_owned())
        .collect();
    let at = editing_target(tab, target_lid);
    write_local(tab, &at, |base, scope| {
        base.edit_local(&base.paint_change(&strokes, rule), scope)
    })
}

/// 地図の編集を、書く所 `targets`（レイヤー, 階層）のそれぞれに、触った所だけで書く（1本の道：`LayerBase::edit_local`・
/// `Cel::edit_local`。設計：claude/触った所だけを計算する地図の設計_22fbbbe.md の段2b の 8.）。地図を書く入口はこれ1つ
/// （道具は今のレイヤーの編集中の階層1つ、グループの変形は選んだグループの階層みな）。`f` は、計算に使う写しと階層から、
/// 道具が作った重ね合わせに渡すもので編集した答えを返す。書く所を順に書き（同じレイヤーの階層は、そのセル画に順に。階層
/// ごとの地図は別なので、順に書いても答えは同じ）、後始末は1回（`write_cels`）。格子に置けないときは `Refusal::OffGrid`、
/// 正しい地図にならなければ `Refusal::Unwritable` を知らせて、そこで止めて `false`（それまでに書いた所は、断った操作を
/// `apply` が操作の前に戻す：全部か無しか。前はグループの変形だけがこの入口の中身を写した別の道で書き、断った理由を捨て、
/// 断られたグループだけが黙って動かずに残った：レビュー de92ceb の N-4001。設計：claude/地図を書く入口を1つにする設計_904de4a.md）
pub(crate) fn write_local(
    tab: &mut Tab,
    targets: &[(LayerId, Vec<u64>)],
    f: impl Fn(&crate::planar_map::LayerBase, &[u64]) -> crate::planar_map::LocalEdit,
) -> bool {
    let mut layer_ids: Vec<LayerId> = Vec::new();
    for (lid, _) in targets {
        if !layer_ids.contains(lid) {
            layer_ids.push(*lid);
        }
    }
    crate::canvas_tool::write_cels(tab, &layer_ids, |lid, cel, held| {
        let mut changed = false;
        for (_, scope) in targets.iter().filter(|t| t.0 == lid) {
            if let Some(w) = cel.edit_local(scope, |base| f(base, scope))? {
                held.extend(w.0);
                changed = true;
            }
        }
        Ok(changed)
    })
}

/// 今のレイヤー `layer_id` の、編集中の階層（道具が書く所。`write_local` に渡す1つの書く所）
fn editing_target(tab: &Tab, layer_id: LayerId) -> [(LayerId, Vec<u64>); 1] {
    [(layer_id, tab.session.editing_group_path.clone())]
}

/// 点で見えている、いちばん上のもの（`topmost_visible_at`）
#[derive(Clone, Copy, Debug)]
pub(crate) struct Seen {
    /// レイヤーの番号
    pub(crate) layer: usize,
    /// 図形の色（線なら線の色）
    pub(crate) color: Color32,
    /// 線なら、その線（色と太さ）
    pub(crate) line: Option<crate::planar_map::EdgeStroke>,
}

/// 点 `pt`（ステージの座標）で見えている、いちばん上のもの（バケツとスポイト用。点でいちばん手前のものの判定はこれ1つ）。
/// 画面に描くのと同じ写しの図形を、上から順に参照でたどる：編集できるレイヤーは画面の図形（`Scene`）、ロック中の
/// レイヤーはセル画の結合メッシュの写しが持つ図形（`Tab::cached_cel_seen_at`）。点の内外はどちらも
/// `FillShape::contains_point`。図形は複製しない
pub(crate) fn topmost_visible_at(tab: &Tab, pt: Point) -> Option<Seen> {
    let tl = tab.active_timeline();
    let hit = tab
        .scene()
        .iter()
        .rev()
        .find(|s| s.contains_point(pt))
        .and_then(|s| {
            Some(Seen { layer: tl.layer_index_by_id(s.layer_id)?, color: s.color, line: s.line })
        });
    // 当たった図形より上のロック中のレイヤー（上から順）
    tl.layers
        .iter()
        .enumerate()
        .rev()
        .take_while(|(i, _)| hit.is_none_or(|h| *i > h.layer))
        .filter(|(_, l)| l.visible && l.locked)
        .find_map(|(i, _)| {
            let (color, line) = tab.cached_cel_seen_at(i, tl.current_frame, pt)?;
            Some(Seen { layer: i, color, line })
        })
        .or(hit)
}

fn apply_bucket_fill(tab: &mut Tab, fill_color: Color32, pt: Point) -> bool {
    let target_lid = active_scope_layer_id(&tab.session, tab.active_timeline());
    if !tab.active_timeline().is_layer_editable_by_id(target_lid) {
        return false;
    }

    // 0. 選択領域が存在する場合：選択中の「生シェイプ」のみにバケツで色を流し込む（グループは無効）
    if !tab.session.selection.is_empty() {
        let tl = tab.active_timeline();
        let clicked_selected_raw = tab.scene().iter().any(|s| {
            tab.session.selection.contains_shape(s)
                && tl.is_layer_editable_by_id(s.layer_id)
                && s.is_raw_at_scope(&tab.session.editing_group_path)
                && s.contains_point(pt)
        });
        if clicked_selected_raw {
            let any_color_diff = tab.scene().iter().any(|s| {
                tab.session.selection.contains_shape(s)
                    && tl.is_layer_editable_by_id(s.layer_id)
                    && s.is_raw_at_scope(&tab.session.editing_group_path)
                    && s.color != fill_color
            });
            if !any_color_diff {
                return false;
            }
            push_undo(tab, None);
            let editable_lids: HashSet<u64> = editable_layer_ids(tab).into_iter().collect();
            if !tab.session.lift_selected_raw_shapes_to_floating() {
                tab.session.refusal = Some(crate::canvas_tool::Refusal::OffGrid);
                return false;
            }
            let ids = tab.session.selection.shapes.clone();
            let mut recolored = Ok(false);
            tab.session.update_floating(|fl_opt| {
                if let Some(fl) = fl_opt {
                    // 動かした扱いにするのは、持ち上げた所と、色を変えたときの `recolor` だけ
                    recolored = fl.recolor(&ids, fill_color, &editable_lids);
                }
            });
            if recolored.is_err() {
                tab.session.refusal = Some(crate::canvas_tool::Refusal::Unplaceable);
                return false;
            }
            // 同じ色でとなり合う片は1つになる（新しい番号。選び直しは `Session::sync_floating`）
            return true;
        }
        return false;
    }

    let target_layer_ord = tab
        .active_timeline()
        .layer_index_by_id(target_lid)
        .unwrap_or(0);

    let scene = tab.scene();
    if let Some(hit_idx) = (0..scene.len()).rev().find(|&idx| {
        let Some(s) = scene.get(idx) else {
            return false;
        };
        s.layer_id == target_lid
            && s.is_in_editing_scope(&tab.session.editing_group_path)
            && s.contains_point(pt)
    }) {
        let occluded_above = topmost_visible_at(tab, pt).is_some_and(|h| h.layer > target_layer_ord);

        let Some(hit_shape) = scene.get(hit_idx).cloned() else {
            return false;
        };
        // 前面のレイヤーに覆われている所・グループやシンボルの上は塗らない
        if occluded_above || !hit_shape.is_raw_at_scope(&tab.session.editing_group_path) {
            return false;
        }
        focus_active_layer_to_shape_by_id(tab, hit_shape.shape_id);
    }
    fill_face_at(tab, target_lid, fill_color, pt)
}

/// バケツ：レイヤー `target_lid`・編集中の階層の生の図形を地図にして、点 `pt` を含む面（色のある面、または
/// 同じレイヤー・同じ階層の線で囲まれた何も無い所）を `fill_color` にする（Animate と同じく、ほかのレイヤーや
/// グループの図形は壁にしない）
fn fill_face_at(tab: &mut Tab, target_lid: u64, fill_color: Color32, pt: Point) -> bool {
    // 塗っても絵が変わらないとき（同じ色・囲まれていない所）と、格子に置けない所（±65,536px の外）では、write_local
    // が false を返し、apply が Undo の控えを捨てる
    push_undo(tab, None);
    // 地図は文書の座標なので、点も文書の座標へ写す
    let doc_pt = tab.session.edit_path.world_xform().inverse() * pt;
    let at = editing_target(tab, target_lid);
    write_local(tab, &at, |base, scope| {
        base.fill_local(doc_pt, fill_color, scope)
    })
}

/// 線を引く（鉛筆）：線の区間 `segs`（ステージの座標）を、作業中のレイヤー・編集中の階層の地図に引く
/// （`LayerBase::line_change`・`MapRule::Line`。塗りの色は変えず、塗りの上なら塗りが線で分かれる）
fn apply_draw_lines(tab: &mut Tab, segs: Vec<crate::planar_map::LineSeg>) -> bool {
    let target_lid = active_scope_layer_id(&tab.session, tab.active_timeline());
    if segs.is_empty() || !tab.active_timeline().is_layer_editable_by_id(target_lid) {
        return false;
    }
    if !clear_selection(tab) {
        return false;
    }
    push_undo(tab, None);
    // 地図は文書の座標なので、線も文書の座標へ写す
    let to_doc = tab.session.edit_path.world_xform().inverse();
    let segs: Vec<crate::planar_map::LineSeg> = segs
        .into_iter()
        .map(|(q, stroke)| (to_doc * q, stroke))
        .collect();
    // 引いた線は、新しい線のラベルを持つ（`line_label`）
    let label = crate::vector_engine::next_shape_id();
    let at = editing_target(tab, target_lid);
    write_local(tab, &at, |base, scope| {
        base.edit_local(&base.line_change(&segs, label), scope)
    })
}

/// インクボトル：作業中のレイヤー・編集中の階層の地図の、点 `pt` の上か近くの辺1本か、点を含む面の輪郭に、線 `stroke` を乗せる
/// （`LayerBase::stroke_local`）。前面のレイヤーに覆われている所には乗せない（バケツと同じ）
fn apply_ink_bottle(tab: &mut Tab, stroke: crate::planar_map::EdgeStroke, pt: Point) -> bool {
    let target_lid = active_scope_layer_id(&tab.session, tab.active_timeline());
    let tl = tab.active_timeline();
    let Some(target_ord) = tl.layer_index_by_id(target_lid) else {
        return false;
    };
    if !tl.is_layer_editable_by_id(target_lid)
        || topmost_visible_at(tab, pt).is_some_and(|h| h.layer > target_ord)
    {
        return false;
    }
    // 線が替わらないときは、write_local が false を返し、apply が Undo の控えを捨てる
    push_undo(tab, None);
    let to_stage = tab.session.edit_path.world_xform();
    let doc_pt = to_stage.inverse() * pt;
    // 辺に合う距離は、選択ツールと同じ画面の px（文書の座標の長さにする）
    let px = 1.0 / (tab.view.zoom() as f64 * crate::vector_engine::max_scale(to_stage));
    let reach = crate::planar_map::EdgeGrab::SCREEN_PX.scaled(px);
    let at = editing_target(tab, target_lid);
    write_local(tab, &at, |base, scope| {
        base.stroke_local(doc_pt, stroke, reach, scope)
    })
}

fn has_editable_selection(tab: &Tab) -> bool {
    if tab.session.selection.is_empty() {
        return false;
    }
    let editable_lids = editable_layer_ids(tab);
    tab.scene().iter().any(|s| {
        tab.session.selection.contains_shape(s)
            && editable_lids.contains(&s.layer_id)
            && s.is_in_editing_scope(&tab.session.editing_group_path)
    })
}

/// 選んでいるものに、ステージの座標の変換 `affine` をかける。`pivot_to_instance`：置けると決めたあと、変形の基準点
/// （`transform_pivot`）を選んだインスタンスの中心点に書く（拡大・回転・反転の確定。移動は基準点も一緒に動くので書かない）
fn transform_selected_shapes(tab: &mut Tab, affine: Affine, pivot_to_instance: bool) -> bool {
    if tab.session.selection.is_empty() {
        return false;
    }
    // 地図は格子の内の形しか持てないので、生の図形（浮いた片と、まだ持ち上げていない選んだ生の図形と、選んだグループの中の
    // 生の図形）が格子の外（±65,536px の外）へ出る変形と、潰れる変形は、文書を変える前に断る。確かめるのは確定と同じ式
    // （`TransformFit::fits`。ドラッグ中の「置けない」の表示も同じ）なので、確かめを通れば確定で失敗しない
    if !tab.session.transform_fit().fits(affine) {
        tab.session.refusal = Some(crate::canvas_tool::Refusal::OffGrid);
        return false;
    }
    if pivot_to_instance {
        if let Some(piv) = tab.session.transform_pivot {
            if apply_set_selected_symbol_world_pivot(tab, Some(piv)) {
                tab.session.transform_pivot = None;
            }
        }
    }
    // 選んだ生の図形は片に持ち上げて、片の変換で動かす（持ち上げられなければ断る）
    if !tab.session.lift_selected_raw_shapes_to_floating() {
        tab.session.refusal = Some(crate::canvas_tool::Refusal::OffGrid);
        return false;
    }
    let mut any_transformed = false;
    let selection = tab.session.selection.clone();
    let world = tab.session.edit_path.world_xform();
    let doc = world.inverse() * affine * world;
    let hidden = tab
        .session
        .floating()
        .map(|fl| fl.hidden().clone())
        .unwrap_or_default();
    // 選んだグループの中の生の図形の階層（レイヤー, 階層）
    let scopes = tab.session.selected_group_scopes();
    // 浮いた片（いつもみな選ばれている）は、片の組の変換に、文書の座標の変換を重ねる（確定のときに地図にかける）
    tab.session.update_floating(|fl_opt| {
        if let Some(fl) = fl_opt {
            if !fl.pieces().is_empty() {
                fl.transform(doc);
                any_transformed = true;
            }
        }
    });
    let mut target_lids: Vec<u64> = Vec::new();
    for s in tab.session.base_cache.shapes() {
        if !hidden.contains(&s.shape_id)
            && selection.contains_shape(s)
            && !target_lids.contains(&s.layer_id)
        {
            target_lids.push(s.layer_id);
        }
    }
    if !target_lids.is_empty() {
        // 選んだグループの中の生の図形は、階層ごとに、セル画の地図に文書の座標の変換をかけて、その座標の格子で正しい地図に
        // し、その地図にする差分を1本の道に渡す（`LayerBase::transformed_local`。図形はその地図の面で、面と線の番号は引き継ぐ。
        // 線の番号は辺にしまってあるので、動かしても変わらず、選択はそのまま）
        // （地図を書く入口 `write_local` の1つ。断ったら、そこで止めて断る：それまでに動かしたものは `apply` が戻す）
        any_transformed |= write_local(tab, &scopes, |base, scope| base.transformed_local(doc, scope));
        if tab.session.refusal.is_some() {
            return false;
        }
        // インスタンスは、結び付けの変換に文書の座標の変換を重ねる（生の図形は、片か、グループの階層の地図で動かした）
        any_transformed |= update_instances(
            tab,
            &target_lids,
            |gid| selection.instances.contains(&gid),
            |b| b.xform = doc * b.xform,
        );
        if tab.session.refusal.is_some() {
            return false;
        }
    }
    any_transformed
}

/// `Alt` 複製：選んでいるものの中身（`selected_content`）を、ステージで `offset` ずらした所に置く（面・グループ・
/// インスタンスには新しい番号）。置いたものを選ぶ。置けなければ（格子の外）文書を変えずに `false`
fn duplicate_selected_in_working_copy(tab: &mut Tab, offset: KVec2) -> bool {
    // ドラッグを始めるときに同じ中身を作ってあれば、それを使い回す（`Session::take_ahead_content`）
    let Some(content) = tab.session.take_ahead_content().or_else(|| selected_content(tab)) else {
        return false;
    };
    let xform = content_xform(content.world, tab.session.edit_path.world_xform(), offset);
    let Some(placement) = prepare_placement(tab, &content, xform, true, None) else {
        tab.session.refusal = Some(crate::canvas_tool::Refusal::OffGrid);
        return false;
    };
    if !commit_floating(tab) {
        return false;
    }
    tab.session.selection.clear();
    put_placements(tab, vec![placement], |sym| sym);
    true
}

fn apply_finish_move_selection_drag(
    tab: &mut Tab,
    total_delta: KVec2,
    alt_duplicate: bool,
) -> bool {
    tab.session.clear_overlay();
    if !tab.view.moves_on_screen(total_delta.hypot()) || !has_editable_selection(tab) {
        if let Some(piv) = &mut tab.session.transform_pivot {
            *piv -= total_delta;
        }
        return false;
    }
    push_undo(tab, None);
    if alt_duplicate {
        return duplicate_selected_in_working_copy(tab, total_delta);
    }
    transform_selected_shapes(tab, Affine::translate(total_delta), false)
}

fn apply_move_selected(tab: &mut Tab, delta: KVec2, coalesce: Option<CoalesceKey>) -> bool {
    cancel_active_drag(tab);
    if delta == KVec2::ZERO || !has_editable_selection(tab) {
        return false;
    }
    push_undo(tab, coalesce);
    let moved = transform_selected_shapes(tab, Affine::translate(delta), false);
    if let Some(piv) = &mut tab.session.transform_pivot {
        *piv += delta;
    }
    moved
}

/// 輪郭の変形を確定する。図形のレイヤー・階層の生の図形を境目の線で持つ地図にして編集し、変わった図形だけを差し替える。
/// つかんだ所の、動かす前と後の位置（`from`・`to`。文書の座標）の距離は、ステージで測る
fn apply_map_reshape(
    tab: &mut Tab,
    layer_id: LayerId,
    (from, to): (Point, Point),
    edit: ReshapeEdit,
) -> bool {
    tab.session.clear_overlay();
    let xform = tab.session.edit_path.world_xform();
    if !tab
        .view
        .moves_on_screen((xform * to - xform * from).hypot())
    {
        return false;
    }
    // 頂点・辺を動かす確定は重ね合わせで作る。辺・頂点が無い（つかんだあとに地図が変わった）ときは、変わらない
    apply_map_edit(tab, layer_id, |base, scope| {
        match base.reshape_change(edit) {
            Some(change) => base.edit_local(&change, scope),
            None => crate::planar_map::LocalEdit::Same,
        }
    })
}

/// 地図での編集（輪郭の変形・アンカーの追加・削除。何を書くかは `edit` が1本の道で決める）を確定する。レイヤー
/// `layer_id` の、編集中の階層の生の図形を境目の線で持つ地図にして編集し、変わった図形だけを差し替える
fn apply_map_edit(
    tab: &mut Tab,
    layer_id: LayerId,
    edit: impl Fn(&crate::planar_map::LayerBase, &[u64]) -> crate::planar_map::LocalEdit,
) -> bool {
    if !editable_layer_ids(tab).contains(&layer_id) {
        return false;
    }
    // 輪郭の変形は、始めるときに選択を解除して `Floating` を確定している。`Floating` があるときは何もしない
    if tab.session.floating().is_some() {
        return false;
    }
    // 形が変わらないとき・編集できないとき（消せない頂点など）は、write_local が false を返し、apply が Undo の控えを捨てる
    push_undo(tab, None);
    let at = editing_target(tab, layer_id);
    let changed = write_local(tab, &at, edit);
    if changed {
        focus_active_layer_by_id(tab, layer_id);
    }
    changed
}

#[cfg(test)]
fn apply_pull_edge_from_to(tab: &mut Tab, from: Point, to: Point) -> bool {
    if !clear_selection(tab) {
        return false;
    }
    let target = compute_hover_target(
        tab.scene(),
        &tab.session,
        from,
        tab.view.zoom(),
        SelectSubMode::Reshape,
    );
    let HoverTarget::Edge {
        t,
        map_edge: Some((MapOwner::Layer(layer_id), orig_seg)),
        ..
    } = target
    else {
        return false;
    };
    let grab_t = crate::planar_map::grab_t(t);
    let target = tab.session.edit_path.world_xform().inverse() * to;
    let follow = crate::tool_select::drag_follows(tab);
    apply_map_reshape(
        tab,
        layer_id,
        (orig_seg.eval(grab_t), target),
        ReshapeEdit::PullEdge {
            orig_seg,
            grab_t,
            target,
            follow,
        },
    )
}

#[cfg(test)]
fn apply_move_vertex_from_to(tab: &mut Tab, from: Point, to: Point) -> bool {
    if !clear_selection(tab) {
        return false;
    }
    let target = compute_hover_target(
        tab.scene(),
        &tab.session,
        from,
        tab.view.zoom(),
        SelectSubMode::Reshape,
    );
    let HoverTarget::Vertex {
        map_node: Some((MapOwner::Layer(layer_id), orig_vertex)),
        ..
    } = target
    else {
        return false;
    };
    let target = tab.session.edit_path.world_xform().inverse() * to;
    let follow = crate::tool_select::drag_follows(tab);
    apply_map_reshape(
        tab,
        layer_id,
        (orig_vertex, target),
        ReshapeEdit::vertex(orig_vertex, target, follow),
    )
}

fn apply_anchor_edit_at(tab: &mut Tab, pt: Point, is_add: bool) -> bool {
    let zoom = tab.view.zoom();
    let sub_mode = if is_add {
        SelectSubMode::AddAnchor
    } else {
        SelectSubMode::DeleteAnchor
    };
    let mut target = compute_hover_target(tab.scene(), &tab.session, pt, zoom, sub_mode);
    let edit_of = |target: HoverTarget| match (is_add, target) {
        (
            true,
            HoverTarget::Edge {
                t,
                map_edge: Some((owner, orig_seg)),
                ..
            },
        ) => Some((owner, AnchorEdit::Add { orig_seg, t })),
        (
            false,
            HoverTarget::Vertex {
                map_node: Some((owner, orig_vertex)),
                ..
            },
        ) => Some((owner, AnchorEdit::Delete { orig_vertex })),
        _ => None,
    };
    // 動かして浮いている片は、そのレイヤーの片の地図で編集する（境目なら両側の片が変わる）
    if let Some((MapOwner::Floating(layer_id, group), edit)) = edit_of(target) {
        return edit_floating_anchor(tab, layer_id, group, edit);
    }
    // ほかはレイヤーの地図で編集する（境目なら両側の図形が変わる）。選択・浮いている図形があれば、先に確定して探し直す
    if !tab.session.selection.is_empty() || tab.session.floating().is_some() {
        if !clear_selection(tab) {
            return false;
        }
        target = compute_hover_target(tab.scene(), &tab.session, pt, zoom, sub_mode);
    }
    match edit_of(target) {
        Some((MapOwner::Layer(layer_id), edit)) => {
            apply_map_edit(tab, layer_id, |base, scope| base.anchor_local(edit, scope))
        }
        _ => false,
    }
}

/// 動かして浮いている片のうち、レイヤー `layer_id` の組 `group` の片の地図（アンカーのカーソル合わせと同じ地図）を `edit` で
/// 編集する（境目なら両側の片が変わる。選択も入れ替える）
fn edit_floating_anchor(tab: &mut Tab, layer_id: LayerId, group: usize, edit: AnchorEdit) -> bool {
    let Some(mut fl) = tab.session.floating().filter(|fl| fl.modified).cloned() else {
        return false;
    };
    if !fl.edit_group(layer_id, group, |m| edit.apply(m)) {
        return false;
    }
    push_undo(tab, None);
    tab.session.set_floating(Some(fl));
    true
}

fn apply_set_selected_symbol_world_pivot(tab: &mut Tab, world_pivot: Option<Point>) -> bool {
    let Some(binding) = tab
        .scene()
        .selected_single_symbol_binding(&tab.session.selection)
    else {
        return false;
    };
    let new_local_pivot = match world_pivot {
        Some(pt) => {
            if !crate::vector_engine::invertible(binding.xform) {
                return false;
            }
            binding.xform.inverse() * pt
        }
        None => Point::ZERO,
    };
    if crate::vector_engine::same_pivot(new_local_pivot, binding.pivot) {
        return true;
    }
    // 変形の確定（apply_affine_to_selected_inner）から呼ぶときは、もう控えがあるので何もしない
    push_undo(tab, None);
    let inst_gid = binding.instance_gid;
    let layer_ids = editable_layer_ids(tab);
    update_instances(
        tab,
        &layer_ids,
        |gid| gid == inst_gid,
        |b| b.pivot = new_local_pivot,
    )
}

fn apply_affine_to_selected_inner(tab: &mut Tab, affine: Affine) -> bool {
    cancel_active_drag(tab);
    if affine == Affine::IDENTITY || !has_editable_selection(tab) {
        return false;
    }
    push_undo(tab, None);
    // 置けない変換は、中心点も書かずに断る（`transform_selected_shapes` が置けると決めてから中心点を書く）
    transform_selected_shapes(tab, affine, true)
}

fn apply_paste_clipboard(
    tab: &mut Tab,
    tool: &mut ToolSettings,
    clipboard: &mut Clipboard,
    toast: &mut Option<StatusToast>,
    in_place: bool,
) -> bool {
    if clipboard.content.is_none() {
        return false;
    }
    let target_lid = active_scope_layer_id(&tab.session, tab.active_timeline());
    if !tab.active_timeline().is_layer_editable_by_id(target_lid) {
        show_notice(
            toast,
            NoticeKind::Warning,
            "現在のレイヤーまたはフレームは編集できないためペーストできません",
        );
        return false;
    }

    let (content, had_cycle) = pasteable_content(tab, clipboard);
    if had_cycle {
        show_notice(
            toast,
            NoticeKind::Warning,
            "編集中のシンボル自身や親シンボルは循環参照になるため配置できません",
        );
    }
    let Some(content) = content else {
        return false;
    };

    let offset = if in_place {
        KVec2::ZERO
    } else {
        if clipboard.paste_count == 0 {
            clipboard.paste_offset = KVec2::ZERO;
        }
        clipboard.paste_count += 1;
        let step = 16.0 / (tab.view.zoom() as f64);
        clipboard.paste_offset += KVec2::new(step, step);
        clipboard.paste_offset
    };

    // 貼り付けるのは絵なので、いくつかのレイヤーから写した中身も、作業中のレイヤーへまとめて置く（下のレイヤーの絵から
    // 順に重ねるので、重なる所は上のレイヤーの絵が残る）。置く準備（格子の外へ出るなら、文書を変えずに断る）をしてから、
    // 選択を確定し、シンボルを移植して置く
    let xform = content_xform(content.world, tab.session.edit_path.world_xform(), offset);
    let Some(placement) = prepare_placement(tab, &content, xform, false, None) else {
        tab.session.refusal = Some(crate::canvas_tool::Refusal::OffGrid);
        return false;
    };
    if !clear_selection(tab) {
        return false;
    }
    push_undo(tab, None);
    let sym_map = import_clipboard_symbols(tab, clipboard, &content);
    put_placements(tab, vec![placement], |sym| {
        sym_map.get(&sym).copied().unwrap_or(sym)
    });
    tool_for_placed(tab, tool);
    true
}

/// 置いたもの（貼り付け・ライブラリからの配置）を選んだあと、それを動かせる道具にする：自由変形ツールならそのまま、
/// ほかは選択ツールの形の変形（アンカーの追加・削除の副モードは選択を持たないので戻す：`set_tool`）
fn tool_for_placed(tab: &mut Tab, tool: &mut ToolSettings) {
    let next = if tool.tool == Tool::Transform {
        Tool::Transform
    } else {
        Tool::Select
    };
    set_tool(tab, tool, next);
}

fn apply_delete_symbol(tab: &mut Tab, sym_id: u64) -> bool {
    if tab.doc.find_symbol(sym_id).is_none() {
        return false;
    }
    // 消すシンボルの段から出る。出られなければ（浮いた片の確定が断られた）、文書を変えずに断る
    while tab
        .session
        .edit_path
        .levels
        .iter()
        .any(|lvl| lvl.symbol_id == sym_id)
    {
        if !exit_symbol_timeline(tab) {
            return false;
        }
    }
    if !clear_selection(tab) {
        return false;
    }
    push_undo(tab, None);

    tab.doc.remove_symbol_and_instances(sym_id);
    if tab.view.selected_library_symbol == Some(sym_id) {
        tab.view.selected_library_symbol = tab.doc.library.first().map(|s| s.id);
    }

    tab.refresh_scene_base();
    reset_session_after_timeline_change(tab);
    true
}

/// ライブラリのシンボル `sym_id` を複製して、すぐ下に入れて選ぶ（Adobe Animate の「複製」と同じ）。名前は「（元の名前）の
/// コピー」（もうあれば「のコピー 2」「のコピー 3」…）。中身は同じ形の別のシンボルで、中のグループとインスタンスの番号は
/// 振り直す（別のタブから移すときと同じ `remap_timeline_ids`）。中の子のシンボルは写さず元と同じものを使い、置いてある
/// インスタンスは元のシンボルのまま。浮いた片は先に確定する（見えている片も写すため。そのシンボルの中で編集していたとき）
fn apply_duplicate_symbol(tab: &mut Tab, sym_id: u64) -> bool {
    if tab.doc.find_symbol(sym_id).is_none() || !commit_floating(tab) {
        return false;
    }
    let Some(i) = tab.doc.library.iter().position(|s| s.id == sym_id) else {
        return false;
    };
    push_undo(tab, None);

    let mut copy = (**tab.doc.library.get(i).expect("見つけた所")).clone();
    let base = std::mem::take(&mut copy.name);
    copy.name = tab.doc.unused_symbol_name(
        std::iter::once(format!("{base}のコピー"))
            .chain((2u64..).map(|n| format!("{base}のコピー {n}"))),
    );
    copy.id = tab.doc.alloc_symbol_id();
    copy.revision = crate::vector_engine::next_mesh_revision();
    remap_timeline_ids(
        &mut copy.timeline,
        &mut tab.doc,
        &Default::default(),
        &Default::default(),
    );
    let new_id = copy.id;
    tab.doc.library.insert(i + 1, std::sync::Arc::new(copy));
    tab.doc.bump_revision();
    tab.view.selected_library_symbol = Some(new_id);
    true
}

fn apply_timeline_toggle_tween(tab: &mut Tab, toast: &mut Option<StatusToast>) -> bool {
    let targets = tab.active_timeline().target_layers_for_frame_op();
    if targets.is_empty() {
        return false;
    }
    if !commit_floating(tab) {
        return false;
    }

    let (can_toggle, is_currently_tweened) =
        tab.active_timeline().can_toggle_tween_at_current_frame();
    if !can_toggle {
        show_notice(
            toast,
            NoticeKind::Warning,
            "トゥイーンを設定するには、キーフレームに単一のシンボルインスタンス（F8）のみを配置してください",
        );
        return false;
    }

    let next_tween = !is_currently_tweened;
    let changed = mutate_timeline_with_undo(tab, |tl, _| {
        let curr_f = tl.current_frame;
        let mut changed = false;
        for &l_idx in &targets {
            let layer = &mut tl.layers[l_idx];
            if let Some(kf_idx) = layer.keyframe_index_at(curr_f) {
                let kf = &layer.keyframes[kf_idx];
                if next_tween {
                    if kf.cel.single_symbol_instance().is_some() && !kf.tween {
                        layer.keyframe_mut(kf_idx).tween = true;
                        changed = true;
                    }
                } else if kf.tween {
                    layer.keyframe_mut(kf_idx).tween = false;
                    changed = true;
                }
            }
        }
        changed
    });

    if changed {
        select_shapes_for_timeline_selection(tab);
        tab.view.inspect_timeline_frame = true;
    }
    changed
}

fn apply_set_tween_rotate(tab: &mut Tab, rotate_mode: TweenRotate) -> bool {
    let tl = tab.active_timeline();
    let Some(layer) = tl.layers.get(tl.active_layer) else {
        return false;
    };
    if !layer.accepts_frame_ops() {
        return false;
    }
    let Some(kf_idx) = layer.keyframe_index_at(tl.current_frame) else {
        return false;
    };
    if !layer.keyframes[kf_idx].tween || layer.keyframes[kf_idx].tween_rotate == rotate_mode {
        return false;
    }

    let changed = mutate_timeline_with_undo(tab, |tl, _| {
        if let Some(layer_mut) = tl.layers.get_mut(tl.active_layer) {
            if kf_idx < layer_mut.keyframes.len() {
                layer_mut.keyframe_mut(kf_idx).tween_rotate = rotate_mode;
                return true;
            }
        }
        false
    });
    if changed {
        select_shapes_for_timeline_selection(tab);
        tab.view.inspect_timeline_frame = true;
    }
    changed
}

/// 選ばれている1つのシンボルインスタンスの設定を `update` で書き換える（取り消し1回分）
fn update_selected_symbol_binding(
    tab: &mut Tab,
    coalesce: Option<CoalesceKey>,
    update: impl Fn(&mut SymbolInstanceBinding),
) -> bool {
    let Some((binding, _, _)) = selected_single_symbol_info(tab) else {
        return false;
    };
    let mut next_binding = binding;
    update(&mut next_binding);
    if next_binding == binding {
        return false;
    }

    let inst_gid = binding.instance_gid;
    let sym_id = binding.symbol_id;
    let layer_ids = editable_layer_ids(tab);
    if tab.doc.find_symbol(sym_id).is_none()
        || !tab.scene().iter().any(|s| {
            s.symbol_instance
                .is_some_and(|b| b.instance_gid == inst_gid)
                && layer_ids.contains(&s.layer_id)
        })
    {
        return false;
    }

    if !commit_floating(tab) {
        return false;
    }
    push_undo(tab, coalesce);

    let updated = update_instances(tab, &layer_ids, |gid| gid == inst_gid, &update);
    if updated {
        reset_session_after_timeline_change(tab);
    }
    updated
}

fn apply_set_symbol_playback(
    tab: &mut Tab,
    loop_mode: Option<SymbolLoopMode>,
    first_frame: Option<usize>,
    coalesce: Option<CoalesceKey>,
) -> bool {
    let Some((binding, _, total_frames)) = selected_single_symbol_info(tab) else {
        return false;
    };
    let next_mode = loop_mode.unwrap_or(binding.loop_mode);
    let clamped_first = first_frame
        .unwrap_or(binding.first_frame)
        .min(total_frames.saturating_sub(1));
    update_selected_symbol_binding(tab, coalesce, |b| {
        b.loop_mode = next_mode;
        b.first_frame = clamped_first;
    })
}

/// フレーム移動（`prev_frame` -> `next_frame`）時に、同じキーフレームのスパン内に留まるレイヤー上の選択のみを残し、
/// 別のキーフレームへ跨いだレイヤー上の選択を解除する
fn retain_selection_on_same_keyframe(tab: &mut Tab, prev_frame: usize, next_frame: usize) {
    if prev_frame == next_frame || tab.session.selection.is_empty() {
        return;
    }
    let same_kf_layer_ids: HashSet<u64> = tab
        .active_timeline()
        .layers
        .iter()
        .filter(|l| {
            l.is_editable_at(next_frame)
                && l.keyframe_index_at(prev_frame).is_some()
                && l.keyframe_index_at(prev_frame) == l.keyframe_index_at(next_frame)
        })
        .map(|l| l.id)
        .collect();
    tab.session.retain_selection_in_layers(&same_kf_layer_ids);
}

fn apply_switch_to_layer_and_frame(tab: &mut Tab, layer_idx: usize, frame_idx: usize) {
    if tab.active_timeline().layers.is_empty() {
        return;
    }
    let prev_frame = tab.active_timeline().current_frame;
    if !commit_floating(tab) {
        return;
    }
    let clamped_layer = layer_idx.min(tab.active_timeline().layers.len().saturating_sub(1));
    tab.set_active_layer(clamped_layer);
    tab.set_active_frame(frame_idx);
    tab.set_active_cell_selection(Some(TimelineCellSelection::single(
        clamped_layer,
        frame_idx,
    )));
    let next_frame = tab.active_timeline().current_frame;
    retain_selection_on_same_keyframe(tab, prev_frame, next_frame);
    reset_session_after_timeline_change(tab);
    tab.view.inspect_timeline_frame = true;
}

fn apply_scrub_ruler_to_frame(tab: &mut Tab, frame_idx: usize) {
    let tl = tab.active_timeline();
    let clamped_frame = frame_idx.min(tl.total_frames.saturating_sub(1));
    if tl.current_frame == clamped_frame && tl.cell_selection.is_none() {
        return;
    }
    let prev_frame = tl.current_frame;
    if !commit_floating(tab) {
        return;
    }
    tab.set_active_frame(clamped_frame);
    tab.set_active_cell_selection(None);
    retain_selection_on_same_keyframe(tab, prev_frame, clamped_frame);
    reset_session_after_timeline_change(tab);
}

fn apply_select_timeline_cells(
    tab: &mut Tab,
    start_layer: usize,
    start_frame: usize,
    end_layer: usize,
    end_frame: usize,
) {
    let num_layers = tab.active_timeline().layers.len();
    if num_layers == 0 {
        return;
    }
    let max_l = num_layers - 1;
    let (l0, l1) = (start_layer.min(max_l), end_layer.min(max_l));
    let (f0, f1) = (start_frame, end_frame);

    apply_switch_to_layer_and_frame(tab, l1, f1);
    tab.set_active_cell_selection(Some(TimelineCellSelection::from_corners(l0, f0, l1, f1)));
    select_shapes_for_timeline_selection(tab);
}

fn apply_timeline_insert_keyframe(tab: &mut Tab, clone_prev: bool) -> bool {
    let targets = tab.active_timeline().target_layers_for_frame_op();
    if targets.is_empty() {
        return false;
    }
    let changed = mutate_timeline_with_undo(tab, |tl, doc| {
        tl.insert_keyframe_on_targets(&targets, clone_prev, doc.fps)
    });
    if changed && clone_prev {
        select_shapes_for_timeline_selection(tab);
    }
    changed
}

fn apply_timeline_delete_keyframe(tab: &mut Tab) -> bool {
    let targets = tab.active_timeline().target_layers_for_frame_op();
    if targets.is_empty() {
        return false;
    }
    if !commit_floating(tab) {
        return false;
    }
    if !tab
        .active_timeline()
        .can_delete_keyframe_on_targets(&targets)
    {
        return false;
    }
    mutate_timeline_with_undo(tab, |tl, _| {
        tl.delete_keyframe_on_targets(&targets);
        true
    })
}

fn apply_timeline_extend_frame(tab: &mut Tab) -> bool {
    let targets = tab.active_timeline().target_layers_for_frame_op();
    if targets.is_empty() {
        return false;
    }
    mutate_timeline_with_undo(tab, |tl, _| {
        tl.extend_frame_on_targets(&targets);
        true
    })
}

fn apply_timeline_remove_frame(tab: &mut Tab) -> bool {
    let targets = tab.active_timeline().target_layers_for_frame_op();
    if targets.is_empty() {
        return false;
    }
    if !commit_floating(tab) {
        return false;
    }
    if !tab.active_timeline().can_remove_frame_on_targets(&targets) {
        return false;
    }
    mutate_timeline_with_undo(tab, |tl, doc| tl.remove_frame_on_targets(&targets, doc.fps))
}

fn apply_timeline_move_or_copy_frames(
    tab: &mut Tab,
    sel: TimelineCellSelection,
    delta_layer: isize,
    delta_frame: isize,
    copy_mode: bool,
) -> bool {
    let Some((dst_sel, actual_dl)) =
        tab.active_timeline()
            .resolve_frame_move_bounds(sel, delta_layer, delta_frame)
    else {
        return false;
    };
    mutate_timeline_with_undo(tab, |tl, doc| {
        let fps = doc.fps;
        tl.move_or_copy_frames(sel, dst_sel, actual_dl, copy_mode, fps, || doc.alloc_group_id());
        true
    });
    select_shapes_for_timeline_selection(tab);
    true
}

fn apply_timeline_add_layer(tab: &mut Tab) -> bool {
    if !tab.active_timeline().can_add_layer() {
        return false;
    }
    mutate_timeline_with_undo(tab, |tl, _| {
        tl.add_layer_above_active();
        true
    })
}

fn apply_timeline_delete_active_layer(tab: &mut Tab) -> bool {
    if tab.active_timeline().layers.len() <= 1 {
        return false;
    }
    mutate_timeline_with_undo(tab, |tl, _| {
        tl.delete_active_layer();
        true
    })
}

fn apply_timeline_reorder_layer(tab: &mut Tab, from_idx: usize, to_idx: usize) -> bool {
    let len = tab.active_timeline().layers.len();
    if from_idx >= len || to_idx >= len || from_idx == to_idx {
        return false;
    }
    mutate_timeline_with_undo(tab, |tl, _| {
        tl.reorder_layer(from_idx, to_idx);
        true
    })
}

/// レイヤー `layer_idx` の表示・ロックの印（`get` で読み、`get_mut` で書く）を `value` にする
fn apply_set_layer_flag(
    tab: &mut Tab,
    layer_idx: usize,
    value: bool,
    get: fn(&Layer) -> bool,
    get_mut: fn(&mut Layer) -> &mut bool,
) -> bool {
    if tab
        .active_timeline()
        .layers
        .get(layer_idx)
        .is_none_or(|l| get(l) == value)
    {
        return false;
    }
    mutate_timeline_with_undo(tab, |tl, _| {
        if let Some(layer) = tl.layers.get_mut(layer_idx) {
            *get_mut(layer) = value;
        }
        true
    })
}

fn apply_set_stage_size(tab: &mut Tab, next_size: Vec2, coalesce: Option<CoalesceKey>) -> bool {
    // 文書に入れる大きさは1つの決まり（範囲にそろえ、有限でなければ断る）
    let Some(next_size) = crate::symbol::stage_size_of(next_size) else {
        return false;
    };
    if (next_size.x - tab.doc.stage_size.x).abs() <= f32::EPSILON
        && (next_size.y - tab.doc.stage_size.y).abs() <= f32::EPSILON
    {
        return false;
    }
    push_undo(tab, coalesce);
    tab.doc.stage_size = next_size;
    true
}

fn apply_set_fps(tab: &mut Tab, next_fps: u32, coalesce: Option<CoalesceKey>) -> bool {
    let next_fps = next_fps.clamp(1, crate::symbol::MAX_FPS);
    if next_fps != tab.doc.fps {
        push_undo(tab, coalesce);
        tab.doc.fps = next_fps;
        return true;
    }
    false
}

// ============================================================================
// 5. テスト向けのラッパー関数（すべて `apply` を経由）
// ============================================================================

#[cfg(test)]
pub(crate) fn commit_raw_outline(host: &mut impl ActionHost, raw: &[Point]) {
    apply(host, Action::CommitRawOutline { raw: raw.to_vec() });
}

#[cfg(test)]
pub(crate) fn bucket_fill_at(host: &mut impl ActionHost, pt: Point) -> bool {
    let color = host.tool().brush_color;
    apply(host, Action::BucketFill { pt, color })
}

#[cfg(test)]
pub(crate) fn finish_move_selection_drag(
    host: &mut impl ActionHost,
    total_delta: KVec2,
    alt_duplicate: bool,
) {
    apply(
        host,
        Action::FinishMoveSelectionDrag {
            total_delta,
            alt_duplicate,
        },
    );
}

#[cfg(test)]
pub(crate) fn group_selected_shapes(host: &mut impl ActionHost) -> Option<u64> {
    if !apply(host, Action::Group) {
        return None;
    }
    let tab = host.tab();
    tab.session
        .selected_indices()
        .first()
        .and_then(|&idx| tab.scene().get(idx))
        .and_then(|s| s.active_group_id_at_scope(&tab.session.editing_group_path))
}

#[cfg(test)]
pub(crate) fn ungroup_selected_shapes(host: &mut impl ActionHost) -> bool {
    apply(host, Action::Ungroup)
}

#[cfg(test)]
pub(crate) fn nudge_selected_shapes(host: &mut impl ActionHost, delta: KVec2) -> bool {
    apply(host, Action::NudgeSelection { delta })
}

#[cfg(test)]
pub(crate) fn delete_selected_shapes(host: &mut impl ActionHost) -> bool {
    apply(host, Action::DeleteSelection)
}

#[cfg(test)]
pub(crate) fn clear_active_frame_shapes(host: &mut impl ActionHost) -> bool {
    apply(host, Action::ClearActiveFrame)
}

#[cfg(test)]
pub(crate) fn copy_selected_shapes(host: &mut impl ActionHost) -> bool {
    apply(host, Action::Copy)
}

#[cfg(test)]
pub(crate) fn cut_selected_shapes(host: &mut impl ActionHost) -> bool {
    apply(host, Action::Cut)
}

#[cfg(test)]
pub(crate) fn paste_clipboard(host: &mut impl ActionHost, in_place: bool) -> bool {
    apply(host, Action::Paste { in_place })
}

#[cfg(test)]
pub(crate) fn set_selected_symbol_world_pivot(
    host: &mut impl ActionHost,
    world_pivot: Option<Point>,
) -> bool {
    apply(host, Action::SetTransformPivot { world_pivot })
}

#[cfg(test)]
pub(crate) fn scale_selected_shapes(host: &mut impl ActionHost, sx: f64, sy: f64) -> bool {
    apply(host, Action::ScaleSelection { sx, sy })
}

#[cfg(test)]
pub(crate) fn flip_selected_shapes(host: &mut impl ActionHost, flip_x: bool, flip_y: bool) -> bool {
    apply(host, Action::FlipSelection { flip_x, flip_y })
}

#[cfg(test)]
pub(crate) fn rotate_selected_shapes(host: &mut impl ActionHost, angle_rad: f64) -> bool {
    apply(host, Action::RotateSelection { angle_rad })
}

#[cfg(test)]
pub(crate) fn convert_selection_to_symbol(host: &mut impl ActionHost) -> Option<u64> {
    apply(host, Action::ConvertToSymbol).then(|| host.tab().doc.library.last().map(|s| s.id))?
}

#[cfg(test)]
pub(crate) fn place_symbol_instance_from_library(host: &mut impl ActionHost, sym_id: u64) -> bool {
    apply(host, Action::PlaceSymbol { sym_id })
}

#[cfg(test)]
pub(crate) fn rename_symbol_in_library(
    host: &mut impl ActionHost,
    sym_id: u64,
    new_name: impl Into<String>,
) -> bool {
    apply(
        host,
        Action::RenameSymbol {
            sym_id,
            new_name: new_name.into(),
        },
    )
}

#[cfg(test)]
pub(crate) fn delete_symbol_from_library(host: &mut impl ActionHost, sym_id: u64) -> bool {
    apply(host, Action::DeleteSymbol { sym_id })
}

#[cfg(test)]
pub(crate) fn timeline_toggle_tween(host: &mut impl ActionHost) -> bool {
    apply(host, Action::ToggleTween)
}

#[cfg(test)]
pub(crate) fn set_active_keyframe_tween_rotate(
    host: &mut impl ActionHost,
    rotate: TweenRotate,
) -> bool {
    apply(host, Action::SetTweenRotate { rotate })
}

#[cfg(test)]
pub(crate) fn set_selected_symbol_playback(
    host: &mut impl ActionHost,
    loop_mode: SymbolLoopMode,
    first_frame: usize,
) -> bool {
    apply(
        host,
        Action::SetSymbolPlayback {
            loop_mode: Some(loop_mode),
            first_frame: Some(first_frame),
        },
    )
}

#[cfg(test)]
pub(crate) fn switch_to_layer_and_frame(
    host: &mut impl ActionHost,
    layer_idx: usize,
    frame: usize,
) {
    apply(host, Action::SwitchLayerAndFrame { layer_idx, frame });
}

#[cfg(test)]
pub(crate) fn scrub_ruler_to_frame(host: &mut impl ActionHost, frame: usize) {
    apply(host, Action::ScrubRulerToFrame { frame });
}

#[cfg(test)]
pub(crate) fn select_timeline_cells(
    host: &mut impl ActionHost,
    l0: usize,
    f0: usize,
    l1: usize,
    f1: usize,
) {
    apply(host, Action::SelectTimelineCells { l0, f0, l1, f1 });
}

#[cfg(test)]
pub(crate) fn timeline_insert_keyframe(host: &mut impl ActionHost, copy_prev: bool) -> bool {
    apply(host, Action::InsertKeyframe { copy_prev })
}

#[cfg(test)]
pub(crate) fn timeline_delete_keyframe(host: &mut impl ActionHost) -> bool {
    apply(host, Action::DeleteKeyframe)
}

#[cfg(test)]
pub(crate) fn timeline_extend_frame(host: &mut impl ActionHost) -> bool {
    apply(host, Action::ExtendFrame)
}

#[cfg(test)]
pub(crate) fn timeline_remove_frame(host: &mut impl ActionHost) -> bool {
    apply(host, Action::RemoveFrame)
}

#[cfg(test)]
pub(crate) fn timeline_move_or_copy_frames(
    host: &mut impl ActionHost,
    sel: TimelineCellSelection,
    layer_delta: isize,
    frame_delta: isize,
    copy_mode: bool,
) -> bool {
    apply(
        host,
        Action::MoveOrCopyFrames {
            sel,
            layer_delta,
            frame_delta,
            copy_mode,
        },
    )
}

#[cfg(test)]
pub(crate) fn timeline_add_layer(host: &mut impl ActionHost) {
    apply(host, Action::AddLayer);
}

#[cfg(test)]
pub(crate) fn timeline_delete_active_layer(host: &mut impl ActionHost) -> bool {
    apply(host, Action::DeleteActiveLayer)
}

#[cfg(test)]
pub(crate) fn timeline_reorder_layer(
    host: &mut impl ActionHost,
    src_idx: usize,
    dst_idx: usize,
) -> bool {
    apply(host, Action::ReorderLayer { src_idx, dst_idx })
}

#[cfg(test)]
pub(crate) fn set_layer_visible(host: &mut impl ActionHost, layer_idx: usize, visible: bool) {
    apply(host, Action::SetLayerVisible { layer_idx, visible });
}

#[cfg(test)]
pub(crate) fn set_layer_locked(host: &mut impl ActionHost, layer_idx: usize, locked: bool) {
    apply(host, Action::SetLayerLocked { layer_idx, locked });
}
