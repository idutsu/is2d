# is2d 課題リスト 第2回（キャンバスとツール）

## 1. 対象と範囲

- 対象リポジトリ：https://github.com/idutsu/is2d
- 対象コミット：`c93b647`（`c93b64744b2f970e89c301a61e8590d6122f7839`）
- ビルド：`cargo build --release` 成功
- 最後まで読んだファイル（合計 6923 行）

| ファイル | 行数 |
|---|---|
| `src/canvas_tool.rs` | 4097 |
| `src/select_ops.rs` | 922 |
| `src/tool_select.rs` | 843 |
| `src/tool_transform.rs` | 686 |
| `src/tool_paint.rs` | 375 |

- 照らし合わせた章：ARCHITECTURE.md 1・3・4・5・8章（Undo 履歴の実装が範囲内にあるため 9章も参照）、GEOMETRY.md 5章、DOCUMENT.md 第1〜3章
- 確かめ方：コードを読んだうえで、主張や指摘の裏付けが必要なものは一時的なプローブテストを書いて `cargo test --release` で実行した（計測値は下の表に記載。プローブはレビュー後に削除し、リポジトリは元の状態に戻してある）。既存テストは `test_stage*` の 23 件を実行し、すべて成功した。

## 2. 課題一覧

検証：✔ = コードで確認済み（計測値があるものはプローブで再現済み）、○ = 推測

| ID | 状態 | 重大度 | 観点 | 場所 | 内容 | 根拠 | 検証 | 発見元 |
|---|---|---|---|---|---|---|---|---|
| I-2001 | 未解決 | High | 堅牢性・パフォーマンス | `tool_paint.rs:164-232`, `tool_paint.rs:256-268`（呼び出し元 `panels.rs:1731-1733`） | ロックされたレイヤー上のスポイト判定が、入れ子シンボルを上限なしで全経路たどる。インスタンス単位の bbox による枝刈りもステップ上限もないため、入れ子のシンボルを含むファイルではスポイトにしてカーソルを動かすだけで固まる。スポイトのカーソル表示は毎フレームこの関数を呼ぶ。描画側は `MAX_SYMBOL_EXPANSION_STEPS` などで上限があるのに、こちらは別の経路で文書を直接読んでいるため上限が効かない | `symbol_binding_color_at` は全レイヤーについて `locked_layer_color_at(..., depth + 1)` を呼び、`locked_layer_color_at` は全インスタンスについて `symbol_binding_color_at` を呼ぶ。止まる条件は `depth >= MAX_SYMBOL_NEST_DEPTH`（8）だけ。プローブ（各段 20 インスタンス、ルートのレイヤーをロック）で 1 回の判定にかかった時間：4 段 = 55.6 ms、5 段 = 1.22 s（1 段ごとに約 20 倍）。既存テスト `test_nested_symbol_expansion_memoization_and_step_cap` と同じ 20^8 の構造では、1 フレームに数分かかる計算になる。同じ構造でも土台キャッシュの構築は 60 µs で終わる | ✔ | 初回レビュー（第2回） |
| I-2002 | 未解決 | Medium | パフォーマンス | `canvas_tool.rs:2551-2571`（`UndoEntry.timeline: Timeline`）, `canvas_tool.rs:2700-2717`（`push`） | Undo 履歴の 1 件ごとに、ルートタイムラインの全レイヤー・全キーフレームの構造体を丸ごと複製している。共有されるのは中の図形配列（`Arc`）だけで、`Keyframe`（424 バイト。内部に `Mutex<CelMeshCache>` 376 バイトを持つ）は件数分コピーされる。コマ打ちアニメのようにキーフレームが多い文書では、履歴だけで数百 MB を使う。また 1 件記録するたびに、直前の履歴とキーフレームを突き合わせる処理がキーフレーム数の二乗に比例してかかる | プローブ（20 レイヤー × 600 キーフレーム、セルは空）：空のストロークを 100 回描くと RSS が 72 MB → 616 MB（1 件あたり約 5.4 MB）。1 ストロークあたり約 5 ms。突き合わせ処理は `timeline.rs:1302-1319` で、キーフレームごとに `pl.keyframes.iter().find(..)` を線形探索している。ARCHITECTURE 9章の「未変更のキーフレームは O(1) でポインタ共有し、メモリ肥大化を防ぐ」とも合わない | ✔ | 初回レビュー（第2回） |
| I-2003 | 未解決 | Medium | 仕様適合性 | `tool_transform.rs:419-456`（`handle_transform_click`） | 自由変形ツールで、拡大縮小ハンドル・回転ゾーン・変形枠の内側の空白をドラッグせずにクリックすると、選択が解除される。カーソルは「拡大縮小」や「移動」を表示しているのに、その操作が成り立たない。手が少しでも動かない短いクリックは日常的に起きる。Adobe Animate では、ハンドルや変形枠の内側をクリックしても選択は外れない。選択解除は確定を伴うため、スライス選択などの状態も失われる | `handle_transform_click` は、ピボット以外では「その位置の図形」だけで判定し、何もなければ `Action::Deselect` を出す。`compute_transform_hover` の結果（`Scale` / `Rotate` / `Move`）を見ていない。プローブ：右上ハンドルの上をクリックすると（hover = `Scale(TopRight)`）選択が空になった。変形枠の内側の空白をクリックしても（hover = `Move`）選択が空になった | ✔ | 初回レビュー（第2回） |
| I-2004 | 未解決 | Medium | アーキテクチャ | `canvas_tool.rs:3365-3380`（書き戻し）, `canvas_tool.rs:3126-3154`, `canvas_tool.rs:1867-1875`, `select_ops.rs:634-650`, `select_ops.rs:678-697` | 「図形が変わったか」を `shape_id` と `mesh_revision` だけで判定しているのに、輪郭を直接書き換える API（`get_contour_mut` / `segments_mut`、`set_pulled_edge`、`set_moved_vertex`）は `mesh_revision` を上げない。呼び出し側が `rebuild_cache()` を忘れると、`edit_layers` は変更前の文書の図形をそのまま書き戻し、編集が黙って消える。同じ判定は Undo の `Floating` 共有判定でも使われており、こちらは古い片が履歴に残る。今のところすべての呼び出し側が `rebuild_cache` を呼ぶか新しい図形を作っているので実害はないが、同じ種類の不具合を生みやすい | 書き戻しは `if orig.mesh_revision == s.mesh_revision { let mut kept = orig.clone(); ... continue; }` で元の図形を採用する。`InitialLayerShapeState::matches` も `shapes_equal_for_snapshot` も輪郭そのものは比べない。`set_pulled_edge` は `contour.segments_mut()[seg_idx].p1 = new_p1;` と書くだけで、リビジョンは上がらない | ✔ | 初回レビュー（第2回） |
| I-2005 | 未解決 | Low | パフォーマンス | `tool_paint.rs:311-323` | ブラシ・消しゴムの描画中プレビューで、確定済みの前半メッシュ `committed_screen_mesh` を毎フレーム `clone()` している。ストロークが長いほど 1 フレームのコストが増え、ARCHITECTURE 8章の「O(1) の増分コスト」にならない。描画中にズームやパンが変わったフレームでは、前半全体をスクリーン座標へ変換し直す | `artwork_painter.add(egui::Shape::Mesh(mesh))` に渡すため、`committed_screen_mesh.clone()` で頂点配列を全部コピーしている（epaint 0.29 の `Shape::Mesh` は所有権のある `Mesh` を受け取る） | ✔ | 初回レビュー（第2回） |
| I-2006 | 未解決 | Low | パフォーマンス | `tool_transform.rs:301-329` | 拡大縮小・回転のドラッグ中、マウスが動くたびに変形前の図形列 `orig_shapes` を丸ごと `clone()` している。借用の都合を避けるための複製で、必要のない処理 | `Some(orig_shapes.clone())` を `update_transform_overlay` に渡している。プローブ：2000 図形を回転ドラッグすると、1 イベントあたり約 0.9 ms（変形処理を含む） | ✔ | 初回レビュー（第2回） |
| I-2007 | 未解決 | Low | パフォーマンス | `tool_select.rs:37-116` | 選択中の図形ごとに、網点メッシュに加えて輪郭を細かい `line_segment` に分けて毎フレーム描いている。大量に選択すると 1 フレームの処理が数倍に増える | プローブ（2000 図形、選択ツール、1 フレームの更新とテッセレーション）：未選択 約 1.0 ms・41,815 頂点 → 全選択 約 6.2 ms・272,311 頂点 | ✔ | 初回レビュー（第2回） |
| I-2008 | 未解決 | Low | アーキテクチャ | `tool_paint.rs:251-279`, `canvas_tool.rs:987-998`（ロック・非表示のレイヤーを土台キャッシュから除外） | ARCHITECTURE 4章は「描画・当たり判定・スポイトは、すべて `Scene` を読む」としている。しかし土台キャッシュはロック中のレイヤーを空にしており、ロック中のレイヤーのスポイトは文書を直接再帰的にたどる（描画は `panels.rs:1808` で別のセルキャッシュを使う）。読み取りの経路が二重になり、上限や枝刈りが片方にしか効かない（I-2001 の原因） | `if !layer.visible \|\| layer.locked { ... range: start..start }`。スポイトは `if layer.locked { locked_layer_color_at(layer, cf, local_pt, &tab.doc.library, 0) }` | ✔ | 初回レビュー（第2回） |
| I-2009 | 未解決 | Low | アーキテクチャ | `canvas_tool.rs:2910-2933`, `canvas_tool.rs:3600-3619` | ARCHITECTURE 6章は「タイムラインを取り出してよいのは `with_active_timeline_mut` の中だけ」としているが、`edit_layers` も同じ取り出しを独自に行っている。さらに両方とも、取り出したシンボルのタイムラインの `fps` をルートの fps で上書きしてから戻すという同じ処理を重複して持つ。シンボル固有の fps が知らないうちに書き換わり、重複しているため片方だけ直すと食い違う | どちらも `let mut taken_tl = std::mem::take(&mut ...library[idx].timeline); taken_tl.fps = prev_fps; ... if taken_tl.fps != prev_fps { root.fps = taken_tl.fps }` | ✔ | 初回レビュー（第2回） |
| I-2010 | 未解決 | Low | 仕様適合性（設計書） | `canvas_tool.rs:2930-2932` | ARCHITECTURE 4章は `with_active_timeline_mut` について「呼ぶたびに編集中シンボルのリビジョンと文書のリビジョンを必ず上げる」としているが、ルート編集中は `doc.revision` を上げない。テスト（`test_collapsed_symbol_instance_blocks_enter_and_timeline_mut_bumps_revision`）もシンボルの中だけを確かめている。実害はないと思われるが、設計書の記述と合わない | ルート側の分岐は `let res = f(&mut self.doc.root, ...); self.refresh_scene_base(); res` だけ | ✔ | 初回レビュー（第2回） |
| I-2011 | 未解決 | Low | 堅牢性 | `canvas_tool.rs:1956` | `Scene` は、上乗せが空で並び順キャッシュの長さが土台と違うときに、黙って土台をそのまま使う。並び順キャッシュの無効化漏れを隠してしまう応急処置で、上乗せがある場合は同じずれを検出できない | `let direct_base_fallback = self.overlay.is_empty() && order.len() != base.len();` | ✔ | 初回レビュー（第2回） |
| I-2012 | 未解決 | Low | 堅牢性 | `canvas_tool.rs:3446-3447`, `canvas_tool.rs:3556-3566` | `edit_layers` に編集できないレイヤーの図形が渡されると、その図形は警告なしに「対象レイヤーのうち一番下のもの（`target_lids[0]`）」へ移される。アクティブなレイヤーでもない。たとえば複数レイヤーからコピーしたグループやシンボルを、一部のレイヤーにフレームがない位置へ貼ると、別のレイヤーへ入る | `let fallback_target_lid = target_lids[0]; ... let dst_lid = if edited_by_lid.contains_key(&s.layer_id) { s.layer_id } else { fallback_target_lid };` | ○ | 初回レビュー（第2回） |
| I-2013 | 未解決 | Low | 堅牢性 | `canvas_tool.rs:4058-4069`, `canvas_tool.rs:4080` | `insert_and_select_shapes_in_scope` は、既存の `Floating` を確定せずに上書き・破棄する。今の呼び出し元（`doc_actions.rs:1267`, `1652`, `symbol_ops.rs:147`）はどれも先に確定しているが、その前提は関数のどこにも書かれておらず、守られているかも確かめていない | `tab.session.floating = Some(Floating { ... })` / `tab.session.floating = None;` | ✔ | 初回レビュー（第2回） |
| I-2014 | 未解決 | Low | 堅牢性 | `select_ops.rs:876-883` | 削除のときに有効な選択が見つからないと、`Floating` を確定せずに捨てる。選択 ID が古くなっているなどの異常時に、変更済みの浮いた図形が消える | `if !any_valid { tab.session.selection.clear(); tab.session.floating = None; ... }` | ○ | 初回レビュー（第2回） |
| I-2015 | 未解決 | Low | 堅牢性 | `canvas_tool.rs:3302-3304`, `canvas_tool.rs:909-916` | `writeback_edited_layer_shapes` が「現在フレームにキーフレームがない」として `false` を返した場合でも、土台キャッシュにはコールバックで編集した図形がそのまま入る。文書は変わらないので、キャッシュと文書が食い違う | `let Some(kf_idx) = ...keyframe_index_at(frame) else { return false; };` のあとも、`replace_edited_layers` が `edited_by_lid` の図形でキャッシュを置き換える | ○ | 初回レビュー（第2回） |
| I-2016 | 未解決 | Low | 仕様適合性 | `canvas_tool.rs:613-628`（呼び出し元 `main.rs:970-975`） | `Space` キーを押しただけで `is_panning = true` になり、呼び出し元が進行中のドラッグを毎フレーム取り消す。ブラシで描いている途中に `Space` に触れると、そのストロークは描画されずに消える。また ARCHITECTURE 3章は「パン」も `ActiveCanvasDrag` の一つとしているが、実際のパン状態は `View.pan_last_screen_pos` にあり、enum の外にある | `} else { self.pan_last_screen_pos = None; if space_down { is_panning = true; } }` | ✔ | 初回レビュー（第2回） |
| I-2017 | 未解決 | Low | 仕様適合性 | `canvas_tool.rs:1994-1998`, `canvas_tool.rs:2080-2084` | 選択中（スライス・移動）の生の図形の片は「レイヤーの一番上」に置いて表示されるため、同じレイヤーにあるグループやシンボルより手前に描かれる。確定すると `merge_shapes_into_canvas` が最後の生の図形の直後（グループより後ろ）に戻すので、選択を解除した瞬間に重なり順が入れ替わって見える。何もせず解除したスライスでも同じことが起きる | `Placement::TopOfLayer(piece.layer_id, scope.clone())`。ルートのスコープでは `scope_pos` が `None` になり、レイヤー内の最後の図形（グループを含む）の後ろに置かれる | ✔ | 初回レビュー（第2回） |
| I-2018 | 未解決 | Low | 仕様適合性 | `canvas_tool.rs:1778-1789`, `tool_transform.rs:262-263` | 細い図形では、選択範囲の枠を最低 6px（画面上）まで広げている。拡大縮小の基準点もこの広げた枠の辺になるため、細い線を縦に拡大すると図形が平行移動してしまう | `if bbox.height() < min_extent { bbox.y0 = cy - half_extent; ... }` を `handle_start` と `anchor` に使っている | ○ | 初回レビュー（第2回） |
| I-2019 | 未解決 | Low | 仕様適合性 | `tool_transform.rs:142-146`, `tool_transform.rs:254` | `Alt` を押しながら拡大縮小を始め、途中で `Alt` を離しても、基準点が「反対側の辺」に戻らない。開始時の `anchor` がピボットのまま保存されているため | `let anchor = if alt_held { orig_pivot } else { handle.opposite_position(orig_bbox) };` と `let eff_anchor = if alt_held { *orig_pivot } else { *anchor };` | ✔ | 初回レビュー（第2回） |
| I-2020 | 未解決 | Low | 仕様適合性 | `tool_transform.rs:284` | 回転を受け付けない範囲が、ズームに関係なくワールド座標で 2.0 に固定されている。拡大表示ではピボット近くで回転が反応しない範囲が画面上で大きくなる | `if (pt - *pivot).hypot() > 2.0 {` | ✔ | 初回レビュー（第2回） |
| I-2021 | 未解決 | Low | 仕様適合性（GEOMETRY） | `tool_select.rs:315-318`, `tool_select.rs:428` | GEOMETRY 5.1 は「曲線が新しい通過点 P_drag を通る」としているが、実装は「つかんだ点 + マウスの移動量」を通る（t を [0.18, 0.82] に丸めたとき、カーソルと曲線がずれたままになる）。開始時に曲線が跳ねないようにするための意図的な違いだが、設計書に書かれていない | `grab_offset = (closest_pos - pt) + (grab_pos - closest_pos)`, `target_pt = pt + *grab_offset` | ✔ | 初回レビュー（第2回） |
| I-2022 | 未解決 | Low | アーキテクチャ（設計書） | `tool_select.rs:253-266`, `tool_select.rs:344-346`, `tool_transform.rs:196-203` | ARCHITECTURE 3章は「マウスを離したときに Action を1回だけ発行する」としているが、ドラッグの開始時にも `Action::Deselect` や `Action::SelectSingleAt` を出しており、その中で `Floating` の確定（文書への書き込み）も起きる。挙動としては妥当だが、設計書の記述と合わない | `apply(host, Action::Deselect);`（頂点・辺のドラッグ開始時）、`select_single_shape_at(host, pt)`（塗りのドラッグ開始時） | ✔ | 初回レビュー（第2回） |
| I-2023 | 未解決 | Low | パフォーマンス | `tool_select.rs:402-405`, `tool_select.rs:436-439` | 頂点移動・辺のつまみ変形のプレビューで、マウスが動くたびに図形全体を `rebuild_cache()` してから lyon で再テッセレーションする。セグメント数の多い大きな図形（背景の塗りなど）では、1 イベントのコストが大きくなる | `set_moved_vertex(...); shape.rebuild_cache();` / `set_pulled_edge(...); shape.rebuild_cache();` | ○ | 初回レビュー（第2回） |
| I-2024 | 未解決 | Low | テスト | `tests.rs:10048-10104` | `test_stage7_edit_layers_preserves_mesh_cache_without_rebuilding_from_doc` は処理時間を測っているが、その値を何にも使っていない。性能が確かめられているように見えるだけ | `let _ = (root_elapsed, sym_elapsed);` | ✔ | 初回レビュー（第2回） |
| I-2025 | 未解決 | Low | テスト | `tests.rs:10117-10404` | 「各 Action のあとに作り直しても Scene が変わらない」ことを確かめるテストが扱う Action は一部だけ。`ReshapeEdge` / `MoveVertex` / `AddAnchor` / `DeleteAnchor` / `FinishMoveSelectionDrag`（Alt 複製を含む）/ `CommitTransformDrag` / 同じ位置へのペースト / 削除 / フレーム全消去 / トゥイーン途中のフレームでの編集 / 複数レイヤーにまたがる編集は含まれていない | テスト内で呼んでいる Action の一覧から判断 | ✔ | 初回レビュー（第2回） |
| I-2026 | 未解決 | Low | テスト | `tests.rs` 全体 | I-2001（ロック中のレイヤーと入れ子シンボルに対するスポイト）、I-2002（Undo 履歴のメモリ量）、I-2003（変形ハンドルのクリック）、ストローク増分プレビューの計算量を確かめるテストがない | `fn test_` を検索して該当なし | ✔ | 初回レビュー（第2回） |

## 3. 範囲外のメモ

| 場所 | メモ |
|---|---|
| `timeline.rs:1302-1319` | `strip_transient_caches_with_prev` がレイヤーごと・キーフレームごとに線形探索しており、キーフレーム数の二乗に比例する（I-2002 の時間面の原因） |
| `timeline.rs:221-263` | `Cel` が `Mutex<CelMeshCache>`（376 バイト）を内部に持つため、Undo で複製される `Keyframe` が 424 バイトになる（I-2002 のメモリ面の原因） |
| `vector_engine.rs:1208-1210` | `arc_mesh_to_shape` は `Arc::unwrap_or_clone` で、キャッシュしたメッシュを毎フレーム全部コピーする。ARCHITECTURE 8章 Tier 3 の「O(1)」は実際には頂点数に比例する |
| `vector_engine.rs:1002-1005` | 網点の間隔は、図形の面積が大きいと上限に合わせて広がる。GEOMETRY 5.3 の「一定画面サイズ」にならない場合がある |
| `vector_engine.rs:624-655` | 拡大率が 1.001 以下の `transform`（平行移動・回転・縮小）では、確定時もテッセレーションし直さず、f32 のメッシュを変換したものを使い続ける。ARCHITECTURE 8章の「ドロップ確定時のみ f64 曲線から再構築」とは違う |
| `panels.rs:1824-1833` | 図形を1つ描くたびに `layer_index_by_id`（線形探索）を呼んでいる。毎フレーム O(図形数 × レイヤー数) |
| `doc_actions.rs:2130-2139` | `apply_set_fps` の確定の条件が逆になっている（選択も `Floating` もないときだけ確定するので、実質的に確定しない） |
| `doc_actions.rs`（`apply_set_stage_size` / `apply_set_fps` / `rename_symbol`） | ARCHITECTURE 4章が挙げる「文書を書き換える 4 つの経路」以外から文書を直接書き換えている |

## 4. ドキュメントの主張と裏付け

| 出典 | 主張 | 裏付け | 判定 |
|---|---|---|---|
| ARCH 1章・4章 | 文書を変える入口は `apply` だけ | テストなし。`edit_layers` と `with_active_timeline_mut` は `pub(crate)` で、どこからでも呼べる（テストも直接呼んでいる） | 裏付けなし |
| ARCH 3章 | `ActiveCanvasDrag` で、複数のドラッグが同時に有効になることを型で防ぐ | enum であることはコードで確認。`test_active_canvas_drag_state_machine_and_symbol_inplace_roundtrip_cache` がある。ただしパンは enum の外にある（I-2016） | 一部のみ |
| ARCH 3章 | ドラッグ中は文書を書き換えず、仮表示は Overlay で行う | `test_stage7a_drag_preview_uses_overlay_without_mutating_working_copy` | あり（開始時の確定は除く。I-2022） |
| ARCH 4章 | Scene は図形を複製せず、並び順の列は土台か上乗せが変わったときだけ作り直す | コードで確認（`invalidate_scene_order` を呼ぶ場所）。作り直す回数を確かめるテストはない。無効化漏れを隠す処理がある（I-2011） | 一部のみ |
| ARCH 4章 | 土台キャッシュはキーが変わったレイヤーだけ作り直し、`ShapeId` と `mesh_revision` が同じ図形の描画キャッシュを引き継ぐ | `test_stage7c_scene_base_cache_and_edit_layers`、`test_stage4_drag_2000_shapes_preserves_unmoved_mesh_caches`、`test_stage7_set_active_frame_and_per_layer_backdrop_cache`（実行して成功） | あり |
| ARCH 4章 | 土台キャッシュは、文書から作り直した結果と常にビット単位で一致する（各 Action のあとにテストで確認） | `test_stage7_scene_unchanged_after_rebuild_overlay_and_base_cache_on_every_action`（実行して成功）。ただし扱う Action は一部（I-2025） | 一部のみ |
| ARCH 4章 | `set_active_frame` などはリビジョンを上げない | コードで確認（`canvas_tool.rs:2891-2907`） | コードのみ |
| ARCH 4章 | `with_active_timeline_mut` は呼ぶたびに、編集中シンボルと文書のリビジョンを必ず上げる | シンボルの中についてはテストあり。ルートでは文書のリビジョンを上げない（I-2010） | 一部が誤り |
| ARCH 5章 | 確定するまで文書は変えない。スライスを何もせず解除すれば、数値誤差なく元に戻る | `test_stage7b_floating_holds_pieces_without_mutating_cels_until_commit`、`test_stage01_move_deselect_merge_and_marquee_slice_restore_on_cel` | あり |
| ARCH 5章 | 確定は Undo の 1 手にせず、未保存マーク用の世代 ID も変えない | コードで確認（`commit_floating_except` は履歴を積まない）。直接確かめるテストは見当たらない | コードのみ |
| ARCH 5章 | Undo / Redo のあと、選択されていない片は確定する | コードで確認（`canvas_tool.rs:3029-3032`）。`test_stage3_selection_preserved_across_undo_redo_frame_scrub_and_reexpansion` は選択の保持を確かめている | 一部のみ |
| ARCH 8章 Tier 1 | 平行移動や軽微なアフィン変換の間は再テッセレーションせず、ドロップ確定時だけ f64 曲線から作り直す | 前半は `test_stage4_drag_2000...` で確認。確定時に作り直さない場合がある（範囲外のメモ `vector_engine.rs:624-655`） | 一部が誤り |
| ARCH 8章 Tier 3 | キャッシュの取得は `Arc::clone` の O(1) | 描画のたびに頂点配列を全部コピーしている（範囲外のメモ）。ベンチマークはない | 誤り（実際は O(頂点数)） |
| ARCH 8章 | ブラシの描画中プレビューは O(1) の増分コスト | 毎フレーム、確定済みの前半メッシュを全部コピーしている（I-2005）。テスト・ベンチマークともにない | 誤り |
| ARCH 9章（Undo の実装は範囲内） | 未変更のキーフレームは O(1) でポインタ共有し、メモリ肥大化を防ぐ | 共有されるのは図形配列だけ。プローブで 1 件あたり約 5.4 MB（20 × 600 キーフレーム）を確認（I-2002）。テストは件数の上限（100）だけを確かめている | 誤り |
| GEOMETRY 5.1 | `P1' = P1 + Δ / (2 t (1 - t))`、t は [0.18, 0.82] | コード（`select_ops.rs:641-644`、`tool_select.rs:315`）。`test_pull_edge_near_endpoint_and_off_center_does_not_jump_on_drag_start`、`test_pull_edge_across_circle_splits_into_two_shapes`。通過点の定義が設計書と違う（I-2021） | あり（記述に違いあり） |
| GEOMETRY 5.2 | 頂点の移動に合わせて、前後の制御点を ½Δ ずつ動かす | コード（`select_ops.rs:691-694`）。`test_stage01_reshape_and_anchor_edit_on_cel` が頂点移動を扱っている（式そのものを確かめているかは未確認） | コードのみ |
| GEOMETRY 5.2 | 反対側の辺を越えると `Positive` で裏返りを消し、2 つに分かれる | `test_pull_edge_across_circle_splits_into_two_shapes` | あり |
| GEOMETRY 5.3 | 曲線のテッセレーションは lyon（許容誤差 0.04、EvenOdd）で行い、描画時はビューポートの変換だけを行う | 許容誤差と塗りの規則はコード（`vector_engine.rs:975`, `1119`）で確認。毎フレームの頂点コピーがある（ARCH 8章 Tier 3 と同じ） | 一部のみ |
| GEOMETRY 5.3 | 網点はズームに反比例し、画面上で一定の大きさ | 予算の上限で大きさが変わる場合がある（範囲外のメモ）。大きさを確かめるテストはない | 一部が誤り |

### 参考：今回の範囲に関係するテスト（`fn test_` 全 120 件のうち、名前と必要な箇所の本文から判断）

- 選択・スライス・確定：`test_marquee_select_partial_slice_and_shift_multi_select`、`test_marquee_shift_deselect_restores_for_bucket_fill`、`test_shift_partial_deselect_only_merges_deselected_raw_shape`、`test_onion_skin_per_tab_isolation_and_shift_marquee_no_premature_merge`、`test_stage5_floating_lifecycle_and_commit_floating`、`test_stage7b_floating_holds_pieces_without_mutating_cels_until_commit`、`test_stage01_move_deselect_merge_and_marquee_slice_restore_on_cel`、`test_save_while_selected_flushes_merge_drawing_and_preserves_selection`、`test_pivot_commit_restores_marquee_slice_and_persists`
- 輪郭変形・アンカー：`test_pull_edge_across_circle_splits_into_two_shapes`、`test_pull_edge_near_endpoint_and_off_center_does_not_jump_on_drag_start`、`test_add_and_delete_anchor_on_contour`、`test_adobe_animate_click_selects_edge_and_selected_shape_disables_deformation`、`test_stage01_reshape_and_anchor_edit_on_cel`、`test_stage5_anchor_edit_preserved_after_nudge_deselect_and_frame_scrub`
- 自由変形：`test_free_transform_rotate_scale_and_flip`、`test_transform_rotate_zone_only_near_four_corners_and_paste_staircase_across_zoom`、`test_scale_commit_and_repeated_transform_rebuild_mesh_cache`、`test_disjoint_different_color_bbox_overlap_preserves_shape_id_and_small_shape_scale_handles`、`test_raw_input_transform_handles_vertex_move_wheel_zoom_pan_and_save_on_close`、`test_egui_select_and_transform_tool_raw_input_interactions`
- グループ・重なり順：`test_group_selection_bucket_protection_and_non_destructive_overlap`、`test_nested_grouping_edit_in_place_and_single_level_ungroup`、`test_double_click_raw_shape_does_not_exit_group_or_symbol`、`test_stage7_z_order_group_symbol_ungroup_paste_alt_duplicate_and_move`、`test_stage01_group_ungroup_transform_and_clipboard_on_cel`
- Scene・キャッシュ・edit_layers：`test_stage4_scene_resolve_and_hit_testing_via_scene`、`test_stage4_drag_2000_shapes_preserves_unmoved_mesh_caches`、`test_stage7a_drag_preview_uses_overlay_without_mutating_working_copy`、`test_stage7c_scene_base_cache_and_edit_layers`、`test_stage7_edit_layers_preserves_mesh_cache_without_rebuilding_from_doc`、`test_stage7_scene_unchanged_after_rebuild_overlay_and_base_cache_on_every_action`、`test_stage7_set_active_frame_and_per_layer_backdrop_cache`、`test_stage6b_non_uniform_scale_rotated_symbol_edit_preserves_untouched_shape_bits`、`test_hidden_layer_shapes_excluded_from_canvas_and_preserved_on_sync`、`test_cancel_drag_and_noop_drag_preserve_tessellated_mesh_caches`、`test_active_canvas_drag_state_machine_and_symbol_inplace_roundtrip_cache`、`test_collapsed_symbol_instance_blocks_enter_and_timeline_mut_bumps_revision`
- Undo：`test_stage3_selection_preserved_across_undo_redo_frame_scrub_and_reexpansion`、`test_stage6b_undo_redo_across_before_inside_and_after_symbol_restores_doc_and_edit_path`、`test_normalized_cel_instances_empty_symbol_revival_and_undo_cache_stripping`、`test_compact_json_serialization_nan_rejection_and_undo_history_cap`、`test_stage01_undo_redo_coalesce_and_save_load_on_cel`
- ペイント・スポイト：`test_egui_brush_drag_press_origin_and_right_click_ignored`、`test_eraser_only_erases_active_layer_and_preserves_other_unlocked_layers`、`test_noop_eraser_does_not_push_undo_and_active_layer_lock_blocks_paint_tools`、`test_eyedropper_picks_topmost_shape_color`、`test_eyedropper_samples_interpolated_tween_position_on_locked_layer`、`test_multilayer_isolation_cross_layer_marquee_and_lock_protection`
- 視点操作：`test_zoom_around_cursor_preserves_world_point_and_clamps_bounds`、`test_viewport_fit_stage_to_window`、`test_untitled_tab_numbering_and_per_tab_wireframe_state`
