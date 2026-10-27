# `src/` ディレクトリ構成・各ファイルの役割

`animelite` のソースコードは、UIパネル・ツール操作・ベクター幾何演算・タイムライン・テストの役割ごとに以下の **9ファイル** に分割されています。

---

## ファイル一覧（早見表）

| ファイル | 主な役割 |
| :--- | :--- |
| **[`main.rs`](file:///Users/dskzts/animelite/src/main.rs)** | アプリ起動、フォント・テーマ設定、アプリ全体状態（`FlashBrushApp`）、Undo/Redo、ショートカット、メインループ |
| **[`panels.rs`](file:///Users/dskzts/animelite/src/panels.rs)** | 左端縦ツールバー、ステージ上部スリムバー、右端プロパティパネル（3タブ切り替え）、Material Icons 定数 |
| **[`canvas_tool.rs`](file:///Users/dskzts/animelite/src/canvas_tool.rs)** | `Tool` 列挙型、**ツールモード切り替え（`set_tool`）**、ズーム/パン座標変換、ステージ＆オニオンスキン描画、カーソル表示 |
| **[`tool_paint.rs`](file:///Users/dskzts/animelite/src/tool_paint.rs)** | **描画・塗りツール**（ブラシ `B`・消しゴム `E`・バケツ塗り `K`・スポイト `I`）の操作ロジック |
| **[`tool_select.rs`](file:///Users/dskzts/animelite/src/tool_select.rs)** | **選択ツール（`V`）**、マーキー範囲選択（部分スライス）、輪郭の粘土変形（Reshape）、アンカー追加/削除、グループ編集、コピー＆ペースト |
| **[`tool_transform.rs`](file:///Users/dskzts/animelite/src/tool_transform.rs)** | **自由変形ツール（`Q`）**、8方向バウンディングボックス拡大縮小、回転、ピボット移動、反転 |
| **[`timeline.rs`](file:///Users/dskzts/animelite/src/timeline.rs)** | **タイムライン（X-Sheet）**、レイヤー・キーフレーム管理、3段階メッシュキャッシュ＆ゼロコピー再生、タイムラインUI描画 |
| **[`vector_engine.rs`](file:///Users/dskzts/animelite/src/vector_engine.rs)** | **2Dベクター幾何エンジン**（`FillShape` / `Contour`）、ストローク生成・ベジェ曲線フィット、ブーリアン演算（結合・切り抜き・閉領域検出）、三角形分割 |
| **[`tests.rs`](file:///Users/dskzts/animelite/src/tests.rs)** | 全機能の自動ユニットテスト（19件） |

---

## 各ファイルの詳細な役割

### 1. [`main.rs`](file:///Users/dskzts/animelite/src/main.rs) — エントリポイント・アプリ状態・ショートカット・メインループ
- **ウィンドウ起動・初期化**：
  - `main`：ウィンドウサイズとネイティブオプションの設定
  - `configure_japanese_fonts`：`fonts/` 内の日本語フォント（`NotoSansJP`）とアイコンフォント（`MaterialIcons`）の登録
  - `configure_sharp_light_theme`：角丸なし（`Rounding::ZERO`）のシャープなライトテーマ設定
- **アプリケーション状態（`FlashBrushApp`）**：
  - 現在のツール（`tool`）、プロパティパネル状態（`properties_panel`）、ブラシ/消しゴム半径、塗り色、ズーム/パン状態、タイムライン（`timeline`）、現在フレームの作業図形リスト（`shapes`）、Undo/Redo履歴、選択・ドラッグ状態を保持します。
- **Undo / Redo・ショートカット・メインループ**：
  - `push_undo` / `undo` / `redo`：タイムラインとキャンバスのスナップショット履歴管理
  - `handle_shortcuts`：単一キー（`V`, `Q`, `B`, `E`, `K`, `I`, `F5`, `F6`, `F7`, `O`, `Enter` 等）および `Cmd/Ctrl` ショートカットの受付
  - `eframe::App::update`：毎フレームの再生更新、左右パネル・下部タイムライン・上部バー・中央キャンバスの呼び出し、および `handle_canvas_tool_input` によるツール別マウス入力の振り分けを行います。

---

### 2. [`panels.rs`](file:///Users/dskzts/animelite/src/panels.rs) — 左ツールバー・ステージ上部バー・右プロパティパネル
Adobe Animate 準拠の周辺パネルUIを担当します。
- **`mod icons`**：`MaterialIcons-Regular.ttf` のアイコンコードポイント定数一覧
- **左端縦ツールバー（`draw_left_toolbar`）**：
  - `draw_left_main_tool_buttons`：`Tool::ALL` を走査してメインツールボタンを自動生成
  - `draw_left_tool_sub_actions`：選択中ツールの補助ボタン（アンカー追加/削除、反転/回転/ピボットリセット）
  - `draw_left_group_and_view_buttons`：グループ化（`Cmd+G`）、グループ解除（`Cmd+B`）、アンカー・パス表示切替
  - `draw_color_picker_controls`：塗りカラースウォッチとプリセットパレット
  - `draw_left_history_buttons`：Undo / Redo / フレーム全消去ボタン
- **ステージ上部スリムバー（`draw_stage_header_bar`）**：
  - `draw_group_breadcrumb`：シーンとグループ階層のパンくずナビゲーション（`シーン 1 > Group #N`）
  - ブラシ/消しゴムサイズスライダーと右端のズーム倍率リセットボタン（`100%`）
- **右端プロパティパネル（`PropertiesPanel` / `PropertyTab` / `draw_right_properties_panel`）**：
  - `ツール`（`draw_tool_property_tab`）、`オブジェクト`（`draw_object_property_tab`）、`ドキュメント`（`draw_document_property_tab`）の3タブ切り替えUI

---

### 3. [`canvas_tool.rs`](file:///Users/dskzts/animelite/src/canvas_tool.rs) — ツール共通定義・モード切替・ビューポート・ステージ描画
- **`enum Tool`**：6種類のツールのアイコン、ラベル名、ツールチップ、ショートカットキー定義（`Tool::ALL`, `from_shortcut_input`）
- **ドラッグ・ホバー状態型**：`ScaleHandle`, `TransformHoverTarget`, `TransformDragState`, `SelectSubMode`, `HoverTarget`, `SelectDragState`
- **モード切り替え**：
  - `set_tool`：選択状態を維持したまま現在のツールを切り替え、ドラッグ・ホバーの一時状態をリセット
  - `toggle_select_sub_mode`：選択ツールのアンカー追加（`+`）・削除（`-`）サブモード切り替え
- **ビューポート操作**：`world_to_screen`, `screen_to_world`, `zoom_around_screen_pos`, `reset_view`, `handle_viewport_navigation`
- **ステージ＆カーソル描画**：
  - `draw_canvas_contents` / `draw_onion_skins`：現在フレームとオニオンスキンのメッシュ高速描画
  - `draw_tool_cursor_and_feedback`：ツール別のカスタムカーソル・プレビュー円・変形ハンドルのオーバーレイ描画

---

### 4. [`tool_paint.rs`](file:///Users/dskzts/animelite/src/tool_paint.rs) — ブラシ・消しゴム・バケツ・スポイト
- **`handle_brush_or_eraser_input`**：マウスドラッグ中のストローク座標収集とリアルタイムアウトライン生成
- **`commit_raw_outline`**：描き終わったストロークをベジェ曲線にスムージングし、ブラシなら同一レイヤー内で結合/切り抜き（`merge_shapes_into_canvas`）、消しゴムなら減算（`erase_from_canvas`）を実行
- **`bucket_fill_at`**：選択中の生シェイプの色変更、既存シェイプの塗り替え、または複数レイヤーの図形で囲まれた閉領域（穴）の検出とベクター流し込み
- **`pick_color_at`**：クリック位置の最前面シェイプから塗りカラーを抽出

---

### 5. [`tool_select.rs`](file:///Users/dskzts/animelite/src/tool_select.rs) — 選択ツール・輪郭変形・グループ・クリップボード
- **選択操作**：
  - `handle_select_tool_input`：選択ツール（`V`）のマウス入力エントリポイント
  - `apply_marquee_selection`：矩形ドラッグによる範囲選択（生シェイプの部分スライスとグループ全体の選択）
  - `clear_selection`：選択解除時のスライス復元または変更済み生シェイプの再マージ（Merge Drawing 確定）
- **輪郭の粘土変形（Reshape）＆アンカー編集**：
  - `compute_hover_target` / `find_snap_vertex`：頂点（角）・輪郭エッジ・塗り領域のヒット判定と頂点スナップ
  - `start_reshape_drag` / `update_reshape_drag` / `finish_reshape_drag`：エッジ引き伸ばし（`PullEdge`）、頂点移動（`MoveVertex`）、対向エッジ貫通分割（`punch_through_opposite_edge`）
  - `handle_anchor_edit_input`：輪郭上のアンカーポイント追加（`+`）と削除（`-`）
- **グループ＆インプレイス編集**：
  - `group_selected_shapes` (`Cmd+G`) / `ungroup_selected_shapes` (`Cmd+B`)
  - ダブルクリックによるグループ階層への潜り込み（`editing_group_path`）と `exit_current_group`
- **クリップボード**：
  - `copy_selected_shapes` (`Cmd+C`) / `cut_selected_shapes` (`Cmd+X`) / `paste_clipboard` (`Cmd+V` / `Cmd+Shift+V`)

---

### 6. [`tool_transform.rs`](file:///Users/dskzts/animelite/src/tool_transform.rs) — 自由変形ツール（`Q`）
- **ヒット判定とアフィン変換**：
  - `compute_transform_hover`：ピボット点、8方向スケールハンドル、四隅外側の回転ゾーン、内部移動ゾーンの判定
  - `apply_affine_to_selected` / `flip_selected_shapes` / `rotate_selected_shapes` / `scale_selected_shapes`
- **インタラクションと変形枠描画**：
  - `handle_transform_tool_input` / `start_transform_drag` / `update_transform_drag` / `finish_transform_drag`
  - `draw_free_transform_box`：変形バウンディングボックス、8つの四角ハンドル、ピボット丸マークの描画

---

### 7. [`timeline.rs`](file:///Users/dskzts/animelite/src/timeline.rs) — タイムライン・レイヤー・再生キャッシュ・X-Sheet UI
- **データ構造**：`Timeline`, `Layer`, `Keyframe`, `Cel`, `TimelineCellSelection`
- **3段階メッシュキャッシュ＆ゼロコピー再生**：
  - `CelMeshCache`：セル単位でスクリーン座標変換済みの描画メッシュ（`cached_screen_mesh`）とオニオンスキン用メッシュ（`cached_onion_mesh`）をキャッシュ
  - `update_playback` / `toggle_playback` / `stop_playback`：再生中は図形のクローンを行わずキャッシュ済みメッシュを直接GPUへ渡すゼロコピー再生
- **タイムラインUI描画（`draw_timeline_panel`）**：
  - コントロールバー、左側のレイヤー一覧（表示 `👁` / ロック `🔒` / ドラッグ＆ドロップ順序入れ替え）、フレーム番号ルーラー、キーフレーム・保持フレームグリッドを描画

---

### 8. [`vector_engine.rs`](file:///Users/dskzts/animelite/src/vector_engine.rs) — 2Dベクター幾何・曲線フィット・ブーリアン演算エンジン
- **図形データ構造**：
  - `FillShape`, `Contour`, `PathSegment`（2次ベジェ曲線セグメント、外周と穴の輪郭、所属レイヤーID、グループパス、ローカル三角形メッシュキャッシュ、`mesh_revision`）
- **ストローク生成＆ベジェ曲線スムージング**：
  - `generate_brush_outline_at_zoom` / `build_smoothed_stroke_shapes_at_zoom`：手描き点列から一定幅の輪郭を生成し、丸端キャップの保持とG1連続ベジェ曲線フィッティングを実行
- **ブーリアン演算（`i_overlay`）＆三角形分割（`lyon_tessellation`）**：
  - `boolean_operation` / `merge_shapes_into_canvas` / `erase_from_canvas` / `find_enclosed_region_at`
  - `tessellate_shape`：ベジェ輪郭を `egui::Mesh` の頂点・インデックス配列へ変換

---

### 9. [`tests.rs`](file:///Users/dskzts/animelite/src/tests.rs) — ユニットテストスイート
- `cargo test` で実行される全19件のユニットテスト（ストローク一定幅、丸端キャップ、同色結合・異色切り抜き、エッジ貫通分割、アンカー追加/削除、バケツ閉領域塗り、マーキー部分スライス、自由変形、スポイト、ネストグループ、ズーム、コピペ、タイムライン操作など）を収録しています。
