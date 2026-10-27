# `src/` ディレクトリ完全リファレンス（全11ファイル・全型・全関数一覧）

`is2d` のソースコードは、UIパネル・キャンバスツール・ベクター幾何演算・タイムラインデータ・タイムラインUI・シンボル＆トゥイーン・テストの責務ごとに **全11ファイル** に分割されています。
本ドキュメントでは、`src/` 内のすべてのファイル・構造体・列挙型・定数・メソッド・ヘルパー関数・ユニットテストを漏れなく記載します。

---

## ファイル構成一覧（早見表）

| # | ファイル | 行数目安 | 主な責務 |
| :-: | :--- | :---: | :--- |
| 1 | **[`main.rs`](./main.rs)** | ~610行 | アプリ起動、フォント・テーマ初期化、アプリ全体状態（`FlashBrushApp`）、Undo/Redo履歴、キーボードショートカット、メインループ（`eframe::App::update`） |
| 2 | **[`panels.rs`](./panels.rs)** | ~650行 | Material Icons 定数（`mod icons`）、左端縦ツールバー、ステージ上部スリムバー（パンくずリスト）、右端4タブパネル（`プロパティ` / `カラー` / `ライブラリ` / `キャンバス`） |
| 3 | **[`canvas_tool.rs`](./canvas_tool.rs)** | ~1,200行 | `Tool` 列挙型、変形・選択のドラッグ＆ホバー状態型、ビューポート（ズーム・パン）＆ステージ枠描画、レイヤースコープ同期、ツール切替、カスタムカーソル＆キャンバス描画 |
| 4 | **[`tool_paint.rs`](./tool_paint.rs)** | ~175行 | **描画・塗りツール**（ブラシ `B`・消しゴム `E`・バケツ塗り `K`・スポイト `I`）の入力処理とコミットロジック |
| 5 | **[`tool_select.rs`](./tool_select.rs)** | ~1,535行 | **選択ツール（`V`）**、マーキー範囲選択（部分スライス）、輪郭の粘土変形（Reshape）、アンカー追加/削除（`+`/`-`）、グループ階層＆インプレイス編集、コピー＆ペースト |
| 6 | **[`tool_transform.rs`](./tool_transform.rs)** | ~480行 | **自由変形ツール（`Q`）**、ピボット管理、8方向スケールハンドル・回転・平行移動・反転、変形バウンディングボックス描画 |
| 7 | **[`timeline.rs`](./timeline.rs)** | ~1,220行 | **タイムラインのデータモデル＆ロジック**（`Timeline` / `Layer` / `Keyframe` / `Cel`、3段階メッシュキャッシュ＆ゼロコピー再生、キーフレーム操作、フレーム移動/複製、レイヤー操作） |
| 8 | **[`timeline_ui.rs`](./timeline_ui.rs)** | ~1,450行 | **タイムライン（X-Sheet）UI描画＆マウス操作**（`TimelineLayout`、コントロールバー、ルーラー、フレームグリッド、キーフレーム区間描画、レイヤー一覧、D&D操作） |
| 9 | **[`symbol.rs`](./symbol.rs)** | ~1,340行 | **シンボル＆ライブラリ＆クラシックトゥイーン**（`F8` シンボル化、ライブラリ管理、シンボル内タイムライン編集、再生モード `Loop`/`PlayOnce`/`SingleFrame`、`T` トゥイーン補間、関連UI描画） |
| 10 | **[`vector_engine.rs`](./vector_engine.rs)** | ~2,880行 | **2Dベクター幾何エンジン**（`Contour` / `FillShape`、ストローク輪郭生成、G1連続2次ベジェ曲線フィット、`i_overlay` ブーリアン演算＆De Casteljau 再構築、`lyon` 三角形メッシュ化） |
| 11 | **[`tests.rs`](./tests.rs)** | ~1,760行 | 全機能を検証する自動ユニットテストスイート（全22テスト） |

---

## 1. [`main.rs`](./main.rs) — エントリポイント・アプリ状態・Undo/Redo・ショートカット・メインループ

### 定数・初期化関数
- **`MAX_UNDO_HISTORY: usize`** (`50`) — Undo スタックが保持する最大スナップショット数。
- **`main() -> eframe::Result<()>`** — ウィンドウサイズ（`1280 × 820`）とタイトル（`"is2d"`）を設定し、フォント・テーマを初期化して `eframe::run_native` を起動。
- **`configure_japanese_fonts(ctx: &egui::Context)`** — `fonts/NotoSansJP-Medium.ttf`（日本語表示）および `fonts/MaterialIcons-Regular.ttf`（UIアイコン）を読み込み、`Proportional` / `Monospace` フォントファミリーへ登録。
- **`configure_sharp_light_theme(ctx: &egui::Context)`** — すべてのウィジェット・パネル・メニューの角丸を `Rounding::ZERO` に統一したシャープなライトテーマを設定。

### 構造体 `FlashBrushApp`
アプリケーション全体の状態を保持するルート構造体（`Default` 実装あり）。
- **ツール・UI・ブラシ設定フィールド**：
  - `tool: Tool` — 現在選択中のメインツール（初期値 `Tool::Brush`）。
  - `properties_panel: PropertiesPanel` — 右端パネルのアクティブタブ状態。
  - `brush_radius: f32` — ブラシ半径（ワールド座標px、初期値 `10.0`）。
  - `eraser_radius: f32` — 消しゴム半径（ワールド座標px、初期値 `18.0`）。
  - `brush_color: Color32` — 現在の描画・塗りつぶしカラー。
  - `show_wireframe: bool` — アンカーポイントとベジェ曲線パスの常時オーバーレイ表示フラグ。
- **ビューポート・ステージ枠設定フィールド**：
  - `view_zoom: f32` — キャンバスのズーム倍率（`0.1`〜`32.0`、初期値 `1.0`）。
  - `view_pan: Vec2` — キャンバスのパン（平行移動）オフセット。
  - `pan_last_screen_pos: Option<Pos2>` — スペースキー／中ボタンドラッグ中の直前スクリーン座標。
  - `stage_size: Vec2` — ステージ（カメラ出力枠）の幅・高さ（初期値 `800.0 × 450.0`、背景色は常時 `Color32::WHITE`）。
  - `clip_outside_stage: bool` — ステージ枠外の絵を隠すクリッピングマスク有効フラグ。
  - `last_canvas_rect: egui::Rect` — 直近フレームの中央キャンバス描画領域（「ウィンドウに合わせる」計算用）。
- **タイムライン・キャンバス・履歴フィールド**：
  - `timeline: Timeline` — 現在アクティブなタイムライン（シーン1、またはインプレイス編集中のシンボル内タイムライン）。
  - `shapes: Vec<FillShape>` — 現在フレームの可視・非ロックレイヤーから作業用に展開されたベクター図形リスト。
  - `undo_stack: Vec<Timeline>` / `redo_stack: Vec<Timeline>` — タイムライン全体（レイヤー・キーフレーム・ライブラリ含む）のスナップショット履歴。
- **ストローク入力・選択・変形・グループ・シンボル編集フィールド**：
  - `active_stroke: Vec<Point>` — ブラシ／消しゴムでドラッグ中の中心軌跡点列。
  - `active_raw_outline: Vec<Point>` — ドラッグ中にリアルタイム生成された閉輪郭ポリゴン点列。
  - `selected_shapes: Vec<usize>` — `self.shapes` 内で現在選択されている図形のインデックス一覧。
  - `unsliced_canvas: Option<Vec<FillShape>>` — マーキー選択で生シェイプを部分スライスした際、未編集で選択解除したときに無劣化復元するためのスライス前スナップショット。
  - `selection_modified: bool` — 選択中の図形に対して移動・変形・色変更・削除が行われたかどうかのフラグ。
  - `select_sub_mode: SelectSubMode` — 選択ツール（`V`）のサブモード（`Reshape` / `AddAnchor` / `DeleteAnchor`）。
  - `hover_target: HoverTarget` — 選択ツールでマウス直下にヒットしている要素（頂点・エッジ・塗り）。
  - `select_drag: SelectDragState` — 選択ツールのドラッグ状態。
  - `editing_group_path: Vec<u64>` — 現在インプレイス編集中のグループ階層パス（ルート階層では空）。
  - `symbol_edit_stack: Vec<SymbolEditSession>` — シンボルをダブルクリックして内部タイムラインへ潜っている際の親シーン退避スタック。
  - `next_group_id: u64` — グループID／シンボルインスタンスIDの連番採番カウンタ。
  - `clipboard: Vec<FillShape>` / `paste_count: usize` — コピー＆ペースト用内部クリップボードと連続ペーストのオフセット回数。
  - `transform_pivot: Option<Point>` / `transform_hover: TransformHoverTarget` / `transform_drag: TransformDragState` — 自由変形ツール（`Q`）の基準点・ホバー・ドラッグ状態。
  - `tessellator: FillTessellator` — `lyon` の三角形分割器インスタンス。

### `FlashBrushApp` のメソッド（`main.rs` 内）
- **Undo / Redo 履歴管理**：
  - `snapshot_timeline(&mut self) -> Timeline` — 現在の作業キャンバス（スライス前スナップショットがあればそれを優先）をタイムラインへ同期した状態の `Timeline` クローンを作成。
  - `restore_state_snapshot(&mut self, state: Timeline)` — 指定した `Timeline` スナップショットを復元し、作業図形・選択状態・グループ編集パス・シンボル編集同期を更新。
  - `record_undo_snapshot(&mut self, snapshot: Timeline)` — 渡された操作前スナップショットを `undo_stack` に積み（上限 `MAX_UNDO_HISTORY`）、`redo_stack` をクリア。
  - `push_undo(&mut self)` — 現在の状態をスナップショット化して `record_undo_snapshot` に記録。
  - `undo(&mut self)` / `redo(&mut self)` — `Cmd/Ctrl + Z` および `Cmd/Ctrl + Y`（`Cmd+Shift+Z`）による履歴の巻き戻し・やり直し。
- **キーボードショートカット処理**：
  - `handle_shortcuts(&mut self, ctx: &egui::Context)` — 再生中のショートカット制限と、単一キー／修飾キー付きショートカットの振り分け。
  - `handle_single_key_shortcuts(&mut self, ctx: &egui::Context)` — `Enter`（再生/停止）、`,` / `.`（コマ送り）、`F5` / `Shift+F5`（フレーム延長/削除）、`F6` / `Shift+F6`（複製KF/KF削除）、`F7`（空白KF）、`F8`（シンボル変換）、`T`（クラシックトゥイーン切替）、`O`（オニオンスキン）、`V`/`Q`/`B`/`E`/`K`/`I`（ツール切替）、`+`/`-`（アンカー追加/削除）、`Delete`/`Backspace`（削除）、`Esc`（選択解除またはグループ/シンボル階層から出る）を処理。
  - `handle_command_shortcuts(&mut self, ctx: &egui::Context)` — `Cmd/Ctrl + 1`（および `0`：100%表示）、`Cmd/Ctrl + 2`（ウィンドウに合わせる）、`Cmd/Ctrl + 3`（枠外を隠す）、`Cmd/Ctrl + Z/Y`（Undo/Redo）、`Cmd/Ctrl + G/B`（グループ化/解除）、`Cmd/Ctrl + C/X/V` 及び `Cmd/Ctrl + Shift + V`（コピー/カット/ペースト/同じ位置にペースト）を処理。
- **キャンバス入力振り分け＆カーソル表示**：
  - `is_current_tool_allowed(&self) -> bool` — 再生中でなく、かつ対象レイヤーが編集可能（可視・非ロック）かどうかを判定（スポイト・選択・自由変形は他レイヤーが編集可能なら許可）。
  - `handle_canvas_tool_input(&mut self, response: &egui::Response, painter: &egui::Painter, space_down: bool, shift_down: bool, alt_down: bool)` — パン中でない場合に現在のツールに応じた入力ハンドラを呼び出す。
  - `draw_locked_layer_badge(&self, ctx: &egui::Context, response: &egui::Response, painter: &egui::Painter)` — 非表示またはロックされたレイヤー上で描画しようとした際にカーソル横に警告バッジを表示。
  - `update_canvas_cursor(&self, ctx: &egui::Context, response: &egui::Response, painter: &egui::Painter, is_panning: bool, space_down: bool)` — パン中・ロック中・通常操作中のOSカーソルとカスタムツールカーソルを更新。
- **メインループ**：
  - `eframe::App::update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame)` — 毎フレームの再生タイマー進行、ショートカット処理、左ツールバー・右プロパティパネル・下部タイムライン・上部ステージバー・中央キャンバスの描画とイベント処理を実行。

---

## 2. [`panels.rs`](./panels.rs) — アイコン定数・左ツールバー・上部ステージバー・右プロパティパネル

### モジュール `icons`
`MaterialIcons-Regular.ttf` の Unicode コードポイント定数（全22種）：
- `ADD` (`\u{e145}`), `REMOVE` (`\u{e15b}`), `FLIP_H` (`\u{e3e8}`), `FLIP_V` (`\u{e8d5}`), `ROTATE_LEFT` (`\u{e419}`), `ROTATE_RIGHT` (`\u{e41a}`), `PIVOT_RESET` (`\u{e55c}`), `GROUP` (`\u{e7ef}`), `SYMBOL` (`\u{e40a}`), `WIREFRAME` (`\u{e922}`), `DELETE` (`\u{e872}`), `SCENE_MOVIE` (`\u{e02c}`), `ARROW_BACK` (`\u{e5c4}`), `VISIBILITY` (`\u{e8f4}`), `LOCK` (`\u{e897}`), `SKIP_PREV` (`\u{e045}`), `STEP_PREV` (`\u{e5cb}`), `PLAY` (`\u{e037}`), `PAUSE` (`\u{e034}`), `STEP_NEXT` (`\u{e5cc}`), `SKIP_NEXT` (`\u{e044}`), `ONION_SKIN` (`\u{e53b}`).

### 型定義
- **`enum RightPanelTab`** — 右端パネルの4タブ列挙型：
  - `Properties`（`プロパティ`）、`Color`（`カラー`）、`Library`（`ライブラリ`）、`Canvas`（`キャンバス`）。
  - `RightPanelTab::ALL` — 全4タブの定数配列。
  - `label(self) -> &'static str` — タブの表示名を返す。
- **`struct PropertiesPanel`** — 右端パネルの状態（`active_tab: RightPanelTab`、`Default` では `Properties`）。

### `FlashBrushApp` のメソッド（`panels.rs` 内）
- **左端縦ツールバー**：
  - `draw_left_toolbar(&mut self, ctx: &egui::Context)` — 幅 `46px` の左端縦ツールバーパネルを描画。
  - `add_left_selectable_icon(ui: &mut egui::Ui, selected: bool, icon: &str, tooltip: &str) -> egui::Response` — ツールバー用の正方形アイコンボタンを描画するヘルパー。
  - `draw_left_main_tool_buttons(&mut self, ui: &mut egui::Ui)` — `Tool::ALL` を走査して6つのメインツールボタンを描画。
  - `draw_color_picker_controls(ui: &mut egui::Ui, brush_color: &mut Color32)` — 塗りカラーピッカーボタンと10色のプリセットカラーパレットを2列グリッドで描画。
- **ステージ上部スリムバー**：
  - `draw_stage_header_bar(&mut self, ctx: &egui::Context)` — 上部バーを描画し、左側に階層パンくずリスト（`draw_group_breadcrumb`）、右端に現在のズーム倍率ラベル（`100%`）を配置。
  - `draw_group_breadcrumb(&mut self, ui: &mut egui::Ui)` — `シーン 1 › シンボル名 › Group #N` の階層パンくずリストと1階層戻るボタンを描画し、クリックで任意の親階層へ直接ジャンプ可能にする。
- **右端プロパティパネル（4タブ構成）**：
  - `draw_right_properties_panel(&mut self, ctx: &egui::Context)` — 幅 `280px` の右端パネルを描画。
  - `draw_right_panel_tab_bar(&mut self, ui: &mut egui::Ui)` — 上部の4タブ（`プロパティ` / `カラー` / `ライブラリ` / `キャンバス`）切り替えバーを描画。
  - `draw_properties_tab(&mut self, ui: &mut egui::Ui)` — 現在のツールに応じたプロパティセクションを描画。
  - `draw_select_tool_properties(&mut self, ui: &mut egui::Ui)` — 選択ツール時のプロパティ（アンカー追加 `+` / 削除 `-`、グループ化 `Cmd+G` / 解除 `Cmd+B`、シンボルに変換 `F8`、および単一シンボル選択時の再生設定 `draw_selected_symbol_properties`）を描画。
  - `draw_transform_tool_properties(&mut self, ui: &mut egui::Ui)` — 自由変形ツール時のプロパティ（左右反転・上下反転・90°左回転・90°右回転・ピボット中央リセット・単一シンボル再生設定）を描画。
  - `draw_brush_tool_properties(&mut self, ui: &mut egui::Ui)` — ブラシ太さスライダーを描画。
  - `draw_eraser_tool_properties(&mut self, ui: &mut egui::Ui)` — 消しゴム太さスライダーを描画。
  - `draw_bucket_tool_properties(&mut self, ui: &mut egui::Ui)` — バケツツールのプロパティを表示。
  - `draw_eyedropper_tool_properties(&mut self, ui: &mut egui::Ui)` — スポイトツールのプロパティを表示。
  - `draw_color_tab(&mut self, ui: &mut egui::Ui)` — `カラー` タブを描画。
  - `draw_canvas_tab(&mut self, ui: &mut egui::Ui)` — `キャンバス` タブ（キャンバス情報：ステージ幅×高さ `DragValue`、キャンバス操作：`枠外を隠す` ボタン・`ウィンドウに合わせる` ボタン・`100%表示` ボタン）を描画。

---

## 3. [`canvas_tool.rs`](./canvas_tool.rs) — ツール定義・ビューポート・ステージ枠・カーソル・キャンバス描画

### 列挙型・状態型
- **`enum Tool`** — メインツール6種（`Select`, `Transform`, `Brush`, `Eraser`, `Bucket`, `Eyedropper`）。
  - `Tool::ALL: [Tool; 6]` — 全ツールの定数配列。
  - `icon(self) -> &'static str` — Material Icon 文字を返す。
  - `label(self) -> &'static str` — 日本語ツール名を返す。
  - `tooltip(self) -> &'static str` — ショートカット付きのツールチップ文字列を返す。
  - `shortcut_key(self) -> egui::Key` — 対応するショートカットキー（`V`, `Q`, `B`, `E`, `K`, `I`）。
  - `from_shortcut_input(i: &egui::InputState) -> Option<Self>` — 押されたキーから対応する `Tool` を返す。
- **`enum ScaleHandle`** — 自由変形バウンディングボックスの8方向スケールハンドル（`TopLeft`, `Top`, `TopRight`, `Right`, `BottomRight`, `Bottom`, `BottomLeft`, `Left`）。
  - `ScaleHandle::ALL: [ScaleHandle; 8]`
  - `position(self, bbox: Rect) -> Point` — バウンディングボックス上のハンドル座標を返す。
  - `opposite(self) -> Self` / `opposite_position(self, bbox: Rect) -> Point` — 対角・対辺の反対側ハンドルとその座標を返す。
  - `affects_x(self) -> bool` / `affects_y(self) -> bool` / `is_corner(self) -> bool` — X軸・Y軸への影響と四隅判定。
- **`enum TransformHoverTarget`** — 自由変形ツールのホバー対象（`None`, `Pivot`, `Scale(ScaleHandle)`, `Rotate`, `Move`）。
- **`enum TransformDragState`** — 自由変形ツールのドラッグ状態（`None`, `MovePivot`, `Move`, `Scale`, `Rotate`, `MarqueeSelect`）。
- **`enum SelectSubMode`** — 選択ツールのサブモード（`Reshape`, `AddAnchor`, `DeleteAnchor`）。
- **`enum HoverTarget`** — 選択ツールのホバー対象（`None`, `Vertex { shape_idx, contour_idx, seg_idx }`, `Edge { shape_idx, contour_idx, seg_idx, t, nearest_pt }`, `Fill { shape_idx }`）。
  - `shape_idx(&self) -> Option<usize>` — ホバー中の図形インデックスを返す。
- **`enum SelectDragState`** — 選択ツールのドラッグ状態（`None`, `MoveShape`, `MarqueeSelect`, `PullEdge`, `MoveVertex`）。

### `FlashBrushApp` のメソッド（`canvas_tool.rs` 内）
- **ビューポート（ズーム・パン）＆ステージ枠**：
  - `world_to_screen(&self, pt: Point) -> Pos2` / `screen_to_world(&self, pos: Pos2) -> Point` — ワールド座標とスクリーン座標の相互変換。
  - `zoom_around_screen_pos(&mut self, pivot_screen: Pos2, zoom_factor: f32)` — 指定スクリーン座標を不動点としてズーム倍率を変更。
  - `reset_view(&mut self)` — ズームを `1.0`（100%）、パンを `Vec2::ZERO` にリセット。
  - `fit_stage_to_window(&mut self)` — ステージ枠全体が中央キャンバス領域に余白付きでぴったり収まるようにズーム倍率とパン位置を自動調整（「ウィンドウに合わせる」）。
  - `stage_center_world(&self) -> Point` — ステージ中心のワールド座標 `(640.0, 360.0)`（シンボルインプレイス編集中は登録点 `(0.0, 0.0)`）を返す。
  - `stage_world_rect(&self) -> Rect` / `stage_screen_rect(&self) -> egui::Rect` — ワールド座標およびスクリーン座標でのステージ矩形を返す。
  - `draw_stage_background(&self, painter: &egui::Painter, canvas_rect: egui::Rect)` — シーン編集中にペーストボード背景とステージ用紙のドロップシャドウ・背景色を描画。
  - `draw_stage_foreground_overlay(&self, painter: &egui::Painter, canvas_rect: egui::Rect)` — 絵の上にステージ境界枠線を描画し、`clip_outside_stage` 有効時はステージ外周4方向を覆う半透明ベールを描画。シンボル編集中は中心 `(0, 0)` に基準十字マークを描画。
  - `handle_viewport_navigation(&mut self, ctx: &egui::Context, response: &egui::Response, space_down: bool) -> bool` — ホイール／ピンチによるカーソル中心ズームと、スペースキー／中ボタンドラッグによるパン操作を処理。
- **選択マスク＆レイヤースコープ同期**：
  - `selected_shape_mask(&self) -> Vec<bool>` — `self.shapes` の各要素が選択中かどうかを示すブール配列を生成。
  - `active_scope_layer_id(&self) -> u64` — 現在の編集スコープ（グループ編集中ならそのグループが属するレイヤー、通常時はタイムラインのアクティブレイヤー）のレイヤーIDを返す。
  - `topmost_layer_id_for_indices(&self, indices: &[usize]) -> Option<u64>` — 指定された図形インデックス群の中で最前面にある図形の `layer_id` を返す。
  - `focus_active_layer_to_shape(&mut self, shape_idx: usize)` — 指定図形が属するレイヤーをタイムラインのアクティブレイヤーに設定。
  - `sync_timeline_layer_from_canvas_selection(&mut self)` — キャンバス上の選択図形に合わせてタイムラインのアクティブレイヤーとセル選択を自動同期。
- **ツールモード切り替え**：
  - `set_tool(&mut self, new_tool: Tool)` — 選択状態を維持したままメインツールを切り替え、ドラッグ・ホバーの一時状態をリセット。
  - `toggle_select_sub_mode(&mut self, target: SelectSubMode)` — 選択ツールへ切り替えつつ、アンカー追加（`+`）・削除（`-`）サブモードをトグル切り替え。
- **カスタムカーソル＆キャンバス描画**：
  - `draw_tool_cursor_and_feedback(&self, ctx: &egui::Context, painter: &egui::Painter, hover_pos: Option<Pos2>)` — 現在のツールに応じたカーソルオーバーレイを振り分け。
  - `draw_transform_tool_cursor(&self, ctx: &egui::Context, painter: &egui::Painter, pos: Pos2)` — 自由変形ツールの8方向リサイズ矢印、4方向移動矢印、回転アークバッジ、ピボット移動カーソルを表示。
  - `draw_select_tool_cursor(&self, ctx: &egui::Context, painter: &egui::Painter, pos: Pos2)` — 選択ツールの頂点角バッジ、曲線エッジバッジ、アンカー `+`/`-` バッジ、移動カーソルを表示。
  - `draw_highlighted_quad_segment(&self, painter: &egui::Painter, shape_idx: usize, contour_idx: usize, seg_idx: usize, color: Color32)` — ホバー中の2次ベジェ曲線セグメント1本を強調ハイライト描画。
  - `draw_canvas_contents(&mut self, painter: &egui::Painter)` — 再生中は `resolved_cel_screen_mesh` によるゼロコピー高速描画、停止中は編集対象レイヤーとトゥイーン・多フレームシンボルを含む全レイヤーを正確な重ね順で描画し、選択網点・ワイヤーフレーム・自由変形枠を重ねて描画。
  - `draw_onion_skins(&mut self, painter: &egui::Painter)` / `draw_onion_skin_frame(&mut self, painter: &egui::Painter, frame: usize, tint: Color32)` — 前後フレームのオニオンスキン（過去フレーム＝青系、未来フレーム＝緑系、距離に応じた段階的アルファ減衰）を描画。
  - `draw_active_marquee_box(&self, painter: &egui::Painter)` — 選択ツール／自由変形ツールでドラッグ中の矩形選択ボックスを描画。
  - `draw_active_stroke_preview(&mut self, painter: &egui::Painter)` — ブラシ／消しゴムでドラッグ中のリアルタイムストロークプレビューを描画。

### カーソルバッジ描画フリー関数（`canvas_tool.rs` 内）
- `draw_flash_curve_cursor_badge(painter: &egui::Painter, pos: Pos2)` — エッジホバー時の曲線アークバッジを描画。
- `draw_flash_corner_cursor_badge(painter: &egui::Painter, pos: Pos2)` — 頂点ホバー時のシャープ角バッジ＋スナップリングを描画。
- `draw_anchor_plus_cursor_badge(painter: &egui::Painter, pos: Pos2)` — アンカー追加モードの緑 `+` バッジを描画。
- `draw_anchor_minus_cursor_badge(painter: &egui::Painter, pos: Pos2)` — アンカー削除モードの赤 `-` バッジを描画。
- `scale_handle_cursor_icon(handle: ScaleHandle) -> egui::CursorIcon` — `ScaleHandle` に対応するOSリサイズカーソルアイコンを返す。
- `draw_transform_rotate_cursor_badge(painter: &egui::Painter, pos: Pos2)` — 自由変形ツールの回転アーク矢印バッジを描画。
- `draw_eyedropper_cursor_badge(painter: &egui::Painter, pos: Pos2, sample_color: Option<Color32>, current_color: Color32)` — スポイトツールの十字照準と抽出色プレビュースウォッチを描画。

---

## 4. [`tool_paint.rs`](./tool_paint.rs) — ブラシ・消しゴム・バケツ・スポイト操作ロジック

### `FlashBrushApp` のメソッド（`tool_paint.rs` 内）
- **`handle_brush_or_eraser_input(&mut self, response: &egui::Response, painter: &egui::Painter)`** — ブラシ（`B`）および消しゴム（`E`）のドラッグ開始・ドラッグ中（点列追加と `generate_brush_outline_at_zoom` によるリアルタイムアウトライン生成）・ドラッグ終了（`commit_raw_outline` 呼び出し）を処理。
- **`commit_raw_outline(&mut self, raw_outline: &[Point])`** — 描かれた生アウトライン点列を `build_smoothed_stroke_shapes_at_zoom` で滑らかな2次ベジェ図形に変換し、現在の編集スコープとアクティブレイヤーに対してブラシなら `merge_shapes_into_canvas`（同色結合・異色切り抜き）、消しゴムなら `erase_from_canvas`（減算）を実行してタイムラインへ同期。
- **`bucket_fill_at(&mut self, world_pt: Point) -> bool`** — バケツツール（`K`）の塗りつぶし処理：
  1. 選択中の図形がある場合：グループ・シンボルが含まれていれば無効化し、選択中の生シェイプ内部をクリックしたときだけ選択領域の色を変更。
  2. 未選択時：編集スコープ内の既存生シェイプ上ならその図形をリカラーして同色マージ。空白部分なら全可視レイヤーの輪郭を境界壁として `find_enclosed_region_at` で閉領域（穴）を検出し、アクティブレイヤーへ新しい塗り図形を流し込む。
- **`pick_color_at(&mut self, world_pt: Point) -> Option<Color32>`** — スポイトツール（`I`）でクリック／ドラッグ位置の最前面可視図形（シンボル・グループ含む）の塗りカラーを取得し、`self.brush_color` に設定。

---

## 5. [`tool_select.rs`](./tool_select.rs) — 選択ツール・輪郭粘土変形・グループ階層・クリップボード

### `FlashBrushApp` のメソッド（`tool_select.rs` 内）
- **スナップショット＆スコープ挿入ヘルパー**：
  - `take_or_clone_unsliced_snapshot(&mut self) -> Vec<FillShape>` — `unsliced_canvas` があれば取り出し、なければ現在の `self.shapes` をクローンして返す。
  - `scope_insert_index(&self) -> usize` — 現在の編集スコープおよびアクティブレイヤーの末尾に対応する挿入インデックスを返す。
  - `get_contour_segment(&self, shape_idx: usize, contour_idx: usize, seg_idx: usize) -> Option<QuadBez>` — 指定した図形・輪郭・セグメント番号の `QuadBez` を安全に取得。
- **選択・ヒット判定・移動・マーキースライス・削除**：
  - `clear_selection(&mut self)` — 選択状態を解除。マーキースライス後に未編集なら元の図形を無劣化復元し、移動・色変更された生シェイプがあれば同じレイヤー・同じスコープの生シェイプに対して Merge Drawing（結合・切り抜き）を確定。
  - `topmost_shape_at(&self, world_pt: Point) -> Option<usize>` — 編集スコープ内で指定座標にある最前面図形を検索（細線でも掴みやすいよう輪郭・頂点のヒット半径も考慮）。
  - `topmost_visible_color_at(&self, world_pt: Point) -> Option<Color32>` — レイヤーのロック状態やグループ階層を問わず、画面上で見えている最前面図形の色を返す（スポイト用）。
  - `expand_to_group_unit(&self, shape_idx: usize) -> Vec<usize>` — 指定図形が現在のスコープでグループまたはシンボルに属していれば、同一グループ／シンボル単位の全構成図形インデックスへ展開。
  - `selection_has_group(&self) -> bool` — 現在の選択図形の中にグループまたはシンボルが含まれているか判定。
  - `toggle_shape_selection(&mut self, shape_idx: usize)` — `Shift` クリックで図形（またはグループ塊）の選択状態をトグル切り替え。
  - `select_single_shape_at(&mut self, world_pt: Point) -> bool` — 単一クリックで指定座標の図形（またはグループ塊）を選択し、タイムラインのレイヤー選択も同期。
  - `translate_selected_shapes(&mut self, delta: Vec2)` — 選択中の全図形を `delta` だけ平行移動。
  - `nudge_selected_shapes(&mut self, delta: Vec2) -> bool` — 選択中の全図形を十字キー等で `delta` だけ移動し、Undo 履歴記録・最前面整列・タイムライン同期を実行。
  - `apply_marquee_selection(&mut self, start_pt: Point, end_pt: Point, additive: bool)` — 矩形ドラッグによる範囲選択を実行。グループやシンボルは矩形と交差すれば塊全体を選択し、生シェイプは矩形境界で `Intersection` / `Difference` による部分スライス（ナイフ切断選択）を行う（`Shift` 押下時は既存の選択スライスを維持したまま追加スライス）。
  - `delete_selected_shapes(&mut self) -> bool` — 選択中の図形・スライス領域を一括削除し、Undo 記録とタイムライン同期を実行。
  - `commit_moved_selection(&mut self, undo_snapshot: Vec<FillShape>)` — 選択図形の移動・変形完了時に選択図形を各レイヤー・スコープ内の最前面へ並べ替え、シンボルインスタンスのアフィン変換行列も更新。
  - `finish_move_selection_drag(&mut self, start_pt: Point, last_pt: Point, undo_snapshot: Timeline)` — 選択移動ドラッグの終了処理と Undo 記録。
  - `finish_deform_shape_drag(&mut self, shape_idx: usize, start_pt: Point, last_pt: Point, undo_snapshot: Timeline)` — エッジ引き伸ばし／頂点移動ドラッグの終了時に `resolve_deformed_shape` で自己交差解消・横断分割を実行し、同一レイヤー・同一スコープ内へマージ。
  - `selected_shapes_snapshot(&self) -> Vec<FillShape>` — 現在選択中の図形群のクローン配列を返す。
  - `selection_bounding_box(&self) -> Option<Rect>` — 現在選択中の全図形の統合バウンディングボックスを返す。
- **グループ化・グループ解除・インプレイス階層編集**：
  - `group_selected_shapes(&mut self) -> Option<u64>` — 選択中の図形群に新しいグループIDを付与して1つのグループにまとめる（`Cmd/Ctrl + G`、ネストグループ対応）。
  - `ungroup_selected_shapes(&mut self) -> bool` — 選択中のグループまたはシンボルの現在の階層レベルを1段階解除して展開する（`Cmd/Ctrl + B`）。
  - `enter_group_at(&mut self, world_pt: Point) -> bool` — ダブルクリック位置にあるシンボルインスタンス（`enter_symbol_timeline` へ委譲）またはグループ（`editing_group_path` にプッシュ）の内部へインプレイス編集で潜る。
  - `exit_current_group(&mut self) -> bool` — 現在のグループ階層から1つ上の親階層へ出る（ルート階層でシンボル編集中なら `exit_symbol_timeline` で親シーンへ戻る）。
  - `sanitize_editing_group_path(&mut self)` — タイムライン切替や削除後に `editing_group_path` の図形が存在しなくなった場合、存在する親階層まで自動的にポップする。
- **コピー・カット・ペースト（クリップボード操作）**：
  - `copy_selected_shapes(&mut self) -> bool` — 選択中の図形群を内部クリップボードに保存（`Cmd/Ctrl + C`）。
  - `cut_selected_shapes(&mut self) -> bool` — 選択中の図形群をコピーして削除（`Cmd/Ctrl + X`）。
  - `paste_clipboard(&mut self, in_place: bool) -> bool` — クリップボードの図形群を貼り付け（`in_place = false` なら `(+16, +16)` ずつオフセット、`in_place = true` なら元と同じ座標に配置）。グループやシンボルインスタンスには新しい一意なIDを再採番。
  - `insert_and_select_shapes_in_scope(&mut self, new_shapes: Vec<FillShape>)` — 新規生成またはペーストされた図形群を現在の編集スコープへ挿入し、それらを選択状態にしてタイムラインへ同期する共通ヘルパー。
- **輪郭の粘土変形（Reshape）＆アンカーポイント編集**：
  - `punch_through_opposite_edge(contour: &mut Contour, dragged_seg_idx: usize, grab_pt: Point, target_pt: Point, eps: f64)` — エッジを対向側の輪郭線ギリギリまたは向こう側へドラッグした際、確実に2図形へ分割されるよう微小に切り込みを貫通させる補正処理。
  - `apply_pull_edge_to_shape(shape: &mut FillShape, contour_idx: usize, seg_idx: usize, grab_t: f64, orig_seg: QuadBez, target_pt: Point, punch_opposite: bool)` — 2次ベジェ曲線の式 `B(t) = (1-t)^2 P0 + 2t(1-t) P1 + t^2 P2` から制御点 `P1` を逆算し、掴んだ点 `grab_t` が `target_pt` を通るように曲線をリアルタイム変形。
  - `apply_move_vertex_to_shape(shape: &mut FillShape, contour_idx: usize, seg_idx: usize, orig_contour: &Contour, target_pt: Point)` — 掴んだ頂点（アンカーポイント）を `target_pt` へ移動し、前後のセグメントの制御点を自然な比率で追従させる。
  - `point_within_shape_contours(shape: &FillShape, pt: Point, margin: f64) -> bool` — 指定座標が図形の内部または輪郭から `margin` 以内にあるかを判定。
  - `compute_hover_target(&self, world_pt: Point) -> HoverTarget` — 選択ツールでのマウス位置から `Vertex` / `Edge` / `Fill` / `None` を判定（選択済み図形やグループは変形ではなく常に `Fill` 移動として扱う Adobe Animate 準拠挙動）。
  - `find_snap_vertex(&self, world_pt: Point, exclude_shape: usize, exclude_contour: usize, exclude_seg: usize) -> Option<Point>` — 頂点ドラッグ中に近くの他頂点（または同一輪郭の別頂点）へ吸着するスナップ座標を検索。
  - `draw_flash_selection_overlay(&self, painter: &egui::Painter)` — 選択中の生シェイプには白い網点（Polka-Dot）パターン、選択中のグループ／シンボルには水色枠線（シンボルには中心基準十字マークも表示）、ホバー中のエッジ／頂点にはハイライトを描画。
  - `draw_shape_contour_lines(&self, painter: &egui::Painter, shape: &FillShape, color: Color32, stroke_width: f32)` — 図形の外周・穴のベジェ曲線をスクリーン座標でストローク描画。
  - `handle_select_tool_input(&mut self, response: &egui::Response, painter: &egui::Painter, shift_down: bool)` — 選択ツール（`V`）のメイン入力ハンドラ（ダブルクリック階層移動、サブモード振り分け、ドラッグ開始・更新・終了、クリック選択）。
  - `handle_anchor_edit_input(&mut self, response: &egui::Response, painter: &egui::Painter) -> bool` — アンカー追加（`+`：De Casteljau 分割で曲線形状を変えずにアンカー挿入）およびアンカー削除（`-`：アンカーを消去して前後を直線化）のクリック処理を実装。
  - `start_reshape_drag(&mut self, start_pt: Point, shift_down: bool)` / `update_reshape_drag(&mut self, curr_pt: Point)` / `finish_reshape_drag(&mut self, end_pt: Point, shift_down: bool)` / `handle_reshape_click(&mut self, click_pt: Point, shift_down: bool)` — 選択ツールのドラッグ開始・ドラッグ中・ドロップ確定・単発クリックの各フェーズ処理。

---

## 6. [`tool_transform.rs`](./tool_transform.rs) — 自由変形ツール（`Q`）

### `FlashBrushApp` のメソッド（`tool_transform.rs` 内）
- **`active_transform_pivot(&self, bbox: Rect) -> Point`** — カスタムピボットが設定されていればその座標、単一シンボルインスタンス選択時はインスタンスに保存されたピボット（`binding.world_pivot()`）、それ以外は選択バウンディングボックスの中心座標を返す。
- **`has_custom_transform_pivot(&self) -> bool`** — 現在の選択対象に初期位置から移動されたカスタム変形中心（ピボット）が設定されているかを返す。
- **`set_selected_symbol_world_pivot(&mut self, world_pivot: Option<Point>, commit_to_timeline: bool) -> bool`** — 単一シンボルインスタンス選択時にワールド座標ピボットをローカル座標に変換して `SymbolInstanceBinding::pivot` に保存・同期する。
- **`reset_selected_transform_pivot(&mut self) -> bool`** — 選択オブジェクトの変形中心（ピボット）を初期位置（選択中央またはシンボル基準点 `(0, 0)`）へリセットする。
- **`compute_transform_hover(&self, world_pt: Point) -> TransformHoverTarget`** — マウス位置からピボット点、8方向スケールハンドル、四隅外側の回転ゾーン、バウンディングボックス内部の移動ゾーンの優先順位でヒット判定。
- **`apply_affine_to_selected(&mut self, affine: Affine) -> bool`** — 選択中の全図形にアフィン変換行列を適用してタイムラインへ同期する共通ヘルパー。
- **`flip_selected_shapes(&mut self, horizontal: bool, vertical: bool) -> bool`** — ピボット点を基準に選択図形を左右反転または上下反転（反転後のベジェ輪郭ワインディングも自動補正）。
- **`rotate_selected_shapes(&mut self, angle_rad: f64) -> bool`** — ピボット点を基準に選択図形を指定角度（ラジアン）回転。
- **`scale_selected_shapes(&mut self, sx: f64, sy: f64) -> bool`** — ピボット点を基準に選択図形を `(sx, sy)` 倍に拡大・縮小。
- **`handle_transform_tool_input(&mut self, response: &egui::Response, painter: &egui::Painter, shift_down: bool, alt_down: bool)`** — 自由変形ツール（`Q`）のマウス入力エントリポイント（ダブルクリックでのピボットリセットまたはグループ/シンボル内部への潜り込みを含む）。
- **`start_transform_drag(&mut self, start_pt: Point, shift_down: bool)`** — ホバー対象に応じてピボット移動、8方向スケール、回転、平行移動、またはマーキー範囲選択ドラッグを開始。
- **`update_transform_drag(&mut self, curr_pt: Point, shift_down: bool, alt_down: bool)`** — ドラッグ中のリアルタイム変形更新（`Shift` で15°回転スナップ／縦横比固定スケール、`Alt` でピボット中心対称スケール、対辺越えドラッグによるスムーズな反転に対応）。
- **`finish_transform_drag(&mut self, end_pt: Point, shift_down: bool)`** — 自由変形ドラッグの確定処理と Undo 記録。
- **`handle_transform_click(&mut self, click_pt: Point, shift_down: bool)`** — 自由変形ツールでのクリック選択処理。
- **`draw_free_transform_box(&self, painter: &egui::Painter)`** — 選択図形の外周バウンディングボックス、8つの四角ハンドル、中心ピボット丸マーク（`○`）を描画。

---

## 7. [`timeline.rs`](./timeline.rs) — タイムラインのデータモデル・3段階メッシュキャッシュ・キーフレーム＆レイヤー操作

### 構造体・列挙型（`timeline.rs` 内）
- **`struct CelMeshCache`** — セル単位のキャッシュ構造体（`world_revision`, `world_batch_mesh`, `last_zoom`, `last_pan`, `last_screen_mesh`, `last_onion_tint`, `last_onion_mesh`）。
- **`struct Cel`** — 1つのキーフレームが保持する描画セル（`shapes: Vec<FillShape>` と `cache: RefCell<CelMeshCache>`）。
  - `new(shapes: Vec<FillShape>) -> Self` / `is_empty(&self) -> bool`
  - `ensure_world_batch_mesh(&self)` — セル内の全図形の `mesh_revision` ハッシュを検証し、変化があればワールド座標での結合メッシュを再構築（変化がなければ O(1) でスキップ）。
  - `cached_screen_mesh(&self, zoom: f32, pan: Vec2, clip_to_stage: bool) -> Arc<egui::Mesh>` — 現在のズーム・パンに応じたスクリーン座標メッシュをキャッシュから O(1)（`Arc::clone` のみ）で返す。
  - `cached_onion_mesh(&self, zoom: f32, pan: Vec2, tint: Color32) -> Arc<egui::Mesh>` — オニオンスキン用の単色半透明スクリーンメッシュをキャッシュから返す。
- **`struct Keyframe`** — キーフレーム構造体（`start_frame: usize`, `cel: Cel`, `tween: bool`）。
  - `new_blank(start_frame: usize) -> Self` — 指定フレーム開始の空キーフレームを生成。
  - `clear_cel(&mut self)` — キーフレームを空のセル＆`tween = false` に初期化。
- **`struct TimelineCellSelection`** — タイムライン上の矩形セル選択範囲（`min_layer`, `max_layer`, `min_frame`, `max_frame`）。
  - `single(layer: usize, frame: usize) -> Self` / `from_corners(l0: usize, f0: usize, l1: usize, f1: usize) -> Self`
  - `contains(&self, layer: usize, frame: usize) -> bool` / `contains_layer(&self, layer: usize) -> bool`
- **`enum TimelineGridDrag`** — タイムライングリッド上のドラッグ状態（`None`, `ScrubRuler`, `SelectCells`, `MoveFrames`）。
- **`struct Layer`** — タイムラインの1レイヤー（`id: u64`, `name: String`, `visible: bool`, `locked: bool`, `keyframes: Vec<Keyframe>`, `frame_count: usize`）。
  - `new_with_id_and_frames(id: u64, name: impl Into<String>, frame_count: usize) -> Self` / `new_with_id(id: u64, name: impl Into<String>) -> Self`
  - `keyframe_index_at(&self, frame: usize) -> Option<usize>` — 指定フレームが属するキーフレームのインデックスを返す。
  - `span_end_for_keyframe(&self, kf_idx: usize) -> usize` — 指定キーフレームの保持スパン終了フレーム（排他的上限）を返す。
  - `cel_at(&self, frame: usize) -> Option<&Cel>` / `cel_at_mut(&mut self, frame: usize) -> Option<&mut Cel>` — 指定フレームで有効な `Cel` への参照を返す。
  - `insert_keyframe(&mut self, frame: usize, clone_prev: bool) -> usize` — 指定フレームに複製キーフレーム（`F6`）または空白キーフレーム（`F7`）を挿入。
  - `delete_keyframe_at(&mut self, frame: usize) -> bool` — 指定フレームのキーフレームを削除（先頭フレーム0の場合は空セルにクリア）。
  - `insert_frame_at(&mut self, frame: usize)` — 指定フレーム位置に保持フレームを1コマ挿入して後続キーフレームを右へシフト（`F5`）。
  - `remove_frame_at(&mut self, frame: usize) -> bool` — 指定フレーム位置から1コマ削除して後続キーフレームを左へシフト（`Shift+F5`、Adobe Animate 準拠で1コマ単独KFならKF自体を削除）。
  - `extract_keyframes_in_range(&self, min_f: usize, max_f: usize) -> Vec<(usize, Cel, bool)>` — 選択範囲内のキーフレーム群（または先頭スパンの引き継ぎセル）を相対オフセット付きで抽出。
  - `remove_keyframes_in_range(&mut self, min_f: usize, max_f: usize)` — 通常ドラッグ移動時に元の範囲のキーフレームをクリア・削除。
  - `place_keyframes_at(&mut self, base_frame: usize, extracted: Vec<(usize, Cel, bool)>, min_required_end: usize)` — 抽出したキーフレーム群を移動／複製先フレームへ配置。
- **`struct LoadedShapeSig`** — キャンバス展開時の図形署名（`mesh_revision`, `color`, `group_path`, `symbol_instance`）。未編集のトゥイーン補間図形やシンボル展開図形で元のキーフレームを上書き破壊しないための変更検知に使用。
  - `from_shape(s: &FillShape) -> Self` / `matches(&self, s: &FillShape) -> bool`
- **`struct Timeline`** — タイムライン全体の状態（`layers`, `next_layer_id`, `library: Vec<SymbolDef>`, `next_symbol_id`, `active_layer`, `current_frame`, `total_frames`, `fps`, `is_playing`, `loop_playback`, `onion_skin_enabled`, `onion_prev_frames`, `onion_next_frames`, `last_Advance_instant`, `scroll_x`, `scroll_y`, `cell_selection`, `grid_drag`, `dragging_layer`, `loaded_layer_sigs`）。
  - `Default` 実装（24fps、1フレーム、`Layer 1` 1枚）
  - `new_for_symbol_edit(symbol: &SymbolDef, library: Vec<SymbolDef>, next_symbol_id: u64, fps: u32, onion_skin_enabled: bool, onion_prev_frames: usize, onion_next_frames: usize) -> Self` — シンボル内部のインプレイス編集用タイムラインを構築。
  - `sync_total_frames(&mut self)` — 全レイヤーの `frame_count` の最大値に `total_frames` を同期。
  - `select_single_cell(&mut self, layer: usize, frame: usize)` — アクティブレイヤー・現在フレーム・単一セル選択を一括設定。
  - `active_layer_id(&self) -> u64` / `layer_index_by_id(&self, id: u64) -> Option<usize>`
  - `is_layer_editable_by_id(&self, id: u64) -> bool` / `has_any_editable_layer(&self) -> bool` / `is_active_layer_editable(&self) -> bool`
  - `target_layers_for_frame_op(&self) -> Vec<usize>` — フレーム延長・短縮操作の対象レイヤー一覧（セル選択があればそのレイヤー範囲、なければ全レイヤー）を返す。
  - `active_cel_shapes(&self) -> &[FillShape]` — 現在のアクティブレイヤー・現在フレームのセル内図形スライスを返す。
  - `record_loaded_signatures(&mut self, shapes: &[FillShape])` / `is_layer_unchanged_since_load(&self, layer_id: u64, current_layer_shapes: &[FillShape]) -> bool` — ロード時の図形署名の記録と未変更判定。
  - `write_shapes_to_editable_cels(&mut self, shapes: &[FillShape])` — 作業キャンバス `shapes` を各レイヤーの `layer_id` ごとに振り分け、実際に編集されたレイヤーのみセルへ書き戻す。

### `FlashBrushApp` のメソッド（`timeline.rs` 内）
- **キャンバス同期・セル選択・移動**：
  - `flush_canvas_before_timeline_op(&mut self)` — タイムライン操作前に選択を解除して作業キャンバスをタイムラインへ同期。
  - `sort_shapes_by_layer_order(&mut self)` — `self.shapes` をタイムラインのレイヤー背面→前面の順序へ安定ソート。
  - `sync_shapes_to_timeline(&mut self)` — 現在の作業キャンバスをタイムラインへ書き戻し、シンボル編集中なら `sync_active_symbol_edit` で親シーンのライブラリ定義へも即座に反映。
  - `load_shapes_from_timeline(&mut self)` — 現在フレームの全可視・非ロックレイヤーの図形（トゥイーン補間・シンボル評価済み）を `self.shapes` に展開。
  - `select_shapes_for_timeline_selection(&mut self)` — タイムライン上で選択されたセル範囲（レイヤー群）に属するキャンバス上の図形をすべて選択状態にする。
  - `select_timeline_cells(&mut self, l0: usize, f0: usize, l1: usize, f1: usize)` — タイムラインのセル範囲を選択し、キャンバス上の対応図形も自動選択。
  - `scrub_ruler_to_frame(&mut self, target_frame: usize)` — ルーラーのスクラブ等でレイヤー選択を解除したまま指定フレームへ移動。
  - `switch_to_layer_and_frame(&mut self, target_layer: usize, target_frame: usize)` — 指定レイヤー・指定フレームへ移動して単一セル選択状態にする。
- **再生制御＆キーフレーム・フレーム操作**：
  - `step_frame(&mut self, delta: isize)` — 前後のフレームへ1コマ移動（ループ設定対応）。
  - `stop_playback(&mut self)` / `toggle_playback(&mut self)` / `update_playback(&mut self, ctx: &egui::Context)` — 再生開始・停止と、`1.0 / fps` 秒ごとのフレーム進行・再描画要求。
  - `timeline_insert_keyframe(&mut self, clone_prev: bool) -> bool` — `F6`（複製KF）・`F7`（空白KF）の挿入（既存KF先頭で押した場合は自動的に次フレームへ進んで作成する Adobe Animate 準拠挙動）。
  - `timeline_delete_keyframe(&mut self) -> bool` — `Shift+F6` によるキーフレーム削除。
  - `timeline_extend_frame(&mut self) -> bool` / `timeline_remove_frame(&mut self) -> bool` — `F5`（フレーム延長）および `Shift+F5`（フレーム短縮・削除）。
  - `timeline_move_or_copy_frames(&mut self, sel: TimelineCellSelection, layer_delta: isize, frame_delta: isize, copy_mode: bool) -> bool` — 選択フレーム範囲のドラッグ移動および `Alt`+ドラッグ複製。
- **レイヤー操作**：
  - `set_layer_visible(&mut self, layer_idx: usize, visible: bool)` / `set_layer_locked(&mut self, layer_idx: usize, locked: bool)` — レイヤーの表示/非表示（👁）とロック（🔒）の切り替え。
  - `timeline_add_layer(&mut self)` — 現在のアクティブレイヤーの1つ上（前面）に新規レイヤーを追加。
  - `timeline_delete_active_layer(&mut self) -> bool` — アクティブレイヤーを削除（最低1レイヤーは維持）。
  - `timeline_reorder_layer(&mut self, from_idx: usize, to_idx: usize) -> bool` / `timeline_move_active_layer(&mut self, move_up_in_ui: bool) -> bool` — レイヤーの重ね順変更（ドラッグ＆ドロップおよび上下ボタン）。

---

## 8. [`timeline_ui.rs`](./timeline_ui.rs) — タイムライン（X-Sheet）UI描画・マウス操作

### 定数・レイアウト構造体
- **レイアウト定数**：
  - `LAYER_COL_W: f32` (`196.0`) — 左側のレイヤー一覧カラム幅。
  - `RULER_H: f32` (`22.0`) — 上部のフレーム番号ルーラー高さ。
  - `ROW_H: f32` (`24.0`) — 各レイヤー行の高さ。
  - `CELL_W: f32` (`16.0`) — 1フレームセルの横幅。
- **`struct TimelineLayout`** — タイムラインパネル内の矩形領域（`layer_header_rect`, `ruler_rect`, `layer_list_rect`, `grid_rect`）と座標変換を提供するヘルパー構造体。
  - `new(body_rect: egui::Rect, scroll_x: f32, scroll_y: f32, total_layers: usize, max_cols: usize) -> Self`
  - `frame_at_x(&self, x: f32) -> usize` — スクリーンX座標からフレーム番号を算出。
  - `row_at_y(&self, y: f32) -> Option<usize>` — スクリーンY座標からUI行番号を算出。
  - `layer_at_y(&self, y: f32) -> Option<usize>` / `clamped_layer_at_y(&self, y: f32) -> usize` — スクリーンY座標からレイヤーインデックス（上行＝前面レイヤー）を算出。
  - `frame_x(&self, frame: usize) -> f32` / `row_y(&self, ui_row: usize) -> f32` — フレーム番号・UI行番号からスクリーン座標を算出。

### `Timeline` のメソッド＆フリー関数（`timeline_ui.rs` 内）
- **`Timeline::ensure_current_frame_visible(&mut self, grid_width: f32)`** — 再生ヘッド（`current_frame`）が表示領域外に出た際、`scroll_x` を自動スクロールして追従させる。
- **`draw_floating_drag_badge(ctx: &egui::Context, id_str: &'static str, pos: Pos2, text: String, bg_color: Color32, stroke_color: Color32)`** — フレーム移動／複製ドラッグ中やレイヤー並べ替えドラッグ中にカーソル横へ追従するフローティングバッジを描画。

### `FlashBrushApp` のメソッド（`timeline_ui.rs` 内）
- **パネル全体＆コントロールバー描画**：
  - `draw_timeline_panel(&mut self, ctx: &egui::Context)` — 下部タイムラインパネル全体（コントロールバー＋X-Sheet本体）を描画。
  - `draw_timeline_control_bar(&mut self, ui: &mut egui::Ui)` — 上部コントロールバーを描画（以下4つのセクション関数を呼び出し）。
  - `draw_control_bar_layer_buttons(&mut self, ui: &mut egui::Ui, is_playing: bool)` — レイヤー追加（`+`）・削除（`🗑`）・上へ移動（`▲`）・下へ移動（`▼`）ボタンを描画。
  - `draw_control_bar_transport_buttons(&mut self, ui: &mut egui::Ui, is_playing: bool)` — 先頭フレーム・前コマ・再生/一時停止・次コマ・最終フレーム・ループ切替・現在フレーム番号・FPS `DragValue` を描画。
  - `draw_control_bar_keyframe_buttons(&mut self, ui: &mut egui::Ui, is_playing: bool)` — `F6 KF`（複製キーフレーム）、`F7 空KF`（空白キーフレーム）、`+F5`（延長）、`-F5`（短縮）、`KF削除`、`T Tween`（クラシックトゥイーン切替）ボタンを描画。
  - `draw_control_bar_onion_controls(&mut self, ui: &mut egui::Ui)` — オニオンスキン有効トグルと前後参照フレーム数（`前` / `後` `DragValue`）を描画。
- **スクロール＆ポインタ（マウス・ドラッグ）操作**：
  - `handle_timeline_scroll(&mut self, ctx: &egui::Context, body_rect: egui::Rect, grid_w: f32, grid_h: f32, max_cols: usize, total_layers: usize)` — ホイール・トラックパッドによる水平/垂直スクロール制御。
  - `handle_timeline_pointer_input(&mut self, ctx: &egui::Context, response: &egui::Response, layout: &TimelineLayout)` — タイムライン上のポインタ入力を統合処理（以下の開始・ドラッグ更新関数へ振り分け）。
  - `handle_timeline_click_start(&mut self, pointer_pos: Pos2, shift_down: bool, layout: &TimelineLayout)` — ルーラークリック、フレームセルクリック（`Shift` 範囲選択・選択済みセルの再ドラッグによる移動開始）、レイヤー行クリック（👁表示切替・🔒ロック切替・レイヤーD&D開始）を処理。
  - `update_timeline_grid_drag(&mut self, ctx: &egui::Context, pointer_pos: Option<Pos2>, primary_down: bool, primary_released: bool, alt_down: bool, layout: &TimelineLayout)` — ルーラースクラブ、セル矩形範囲選択、フレーム移動／`Alt` 複製ドラッグの更新と確定、およびドラッグ中のゴースト枠・バッジ描画。
  - `update_timeline_layer_drag(&mut self, ctx: &egui::Context, pointer_pos: Option<Pos2>, primary_released: bool, layout: &TimelineLayout)` — レイヤー重ね順ドラッグ＆ドロップの挿入ライン表示とドロップ確定。
- **X-Sheet グリッド・ルーラー・レイヤー一覧描画**：
  - `draw_timeline_frame_grid(&self, painter: &egui::Painter, layout: &TimelineLayout, max_cols: usize)` — フレームグリッド背景、5フレームごとの目盛り列、各レイヤーのキーフレームスパン、選択セルの青ハイライト、赤い再生ヘッド縦線を描画。
  - `draw_keyframe_span(&self, painter: &egui::Painter, layout: &TimelineLayout, layer: &Layer, kf_idx: usize, kf: &Keyframe, row_rect: egui::Rect)` — 1つのキーフレームスパンの背景色（通常＝白/グレー、トゥイーン有効＝薄紫）、クラシックトゥイーンの実線矢印／未完横点線、先頭丸印（塗りあり＝黒丸 `●`、空＝白抜き丸 `○`）、終端縦バーを描画。
  - `draw_timeline_ruler(&self, painter: &egui::Painter, layout: &TimelineLayout, max_cols: usize)` — 上部ルーラーのフレーム番号目盛り、オニオンスキン範囲帯、赤の再生ヘッドマーカーを描画。
  - `draw_timeline_layer_list(&self, painter: &egui::Painter, layout: &TimelineLayout)` — 左側カラムの各レイヤー行（アクティブ強調、レイヤー名、👁表示アイコン、🔒ロックアイコン）を描画。

---

## 9. [`symbol.rs`](./symbol.rs) — シンボル・ライブラリ・再生オプション・クラシックトゥイーン・関連UI

### 構造体・列挙型（`symbol.rs` 内）
- **`enum SymbolLoopMode`** — グラフィックシンボルの再生モード列挙型：
  - `Loop`（`ループ`：親タイムラインの進行に合わせて内部タイムラインをループ再生）
  - `PlayOnce`（`1回再生`：内部タイムラインの最終フレームに達したら停止）
  - `SingleFrame`（`任意のフレーム`：親フレームに関係なく指定した `first_frame` を固定表示）
- **`struct SymbolInstanceBinding`** — シンボルインスタンスの紐付け情報（`instance_gid: u64`, `symbol_id: u64`, `xform: Affine`, `pivot: Point`, `loop_mode: SymbolLoopMode`, `first_frame: usize`）。
  - `new(instance_gid: u64, symbol_id: u64, xform: Affine) -> Self` — デフォルト設定（`pivot: Point::ZERO`, `Loop`, `first_frame: 0`）でバインディングを生成。
  - `world_pivot(self) -> Point` — インスタンスの現在のワールド座標における変形中心点（`self.xform * self.pivot`）を返す。
  - `instantiate_shapes(&self, local_shapes: &[FillShape], layer_id: u64, parent_scope: &[u64]) -> Vec<FillShape>` — シンボルのローカル図形群にインスタンスのアフィン変換・レイヤーID・グループパス・バインディングを適用してステージ上の図形群を生成。
  - `parent_scope_from_path(&self, group_path: &[u64]) -> Vec<u64>` — 図形の `group_path` から自身の `instance_gid` を除いた親スコープパスを抽出。
- **`enum TweenState`** — キーフレーム区間のクラシックトゥイーン状態：
  - `None`（トゥイーン無効）
  - `Incomplete`（トゥイーンONだが条件未達 → タイムライン上に横点線を表示）
  - `Complete { start_frame, end_frame, start_binding, end_binding }`（開始・終了ともに同一シンボルの単一インスタンスのみ → タイムライン上に実線矢印を表示しTRS線形補間を実行）
- **`struct SymbolDef`** — ライブラリに登録されるシンボル定義（`id: u64`, `name: String`, `layers: Vec<Layer>`, `next_layer_id: u64`, `total_frames: usize`）。
  - `new(id: u64, name: String, local_shapes: Vec<FillShape>) -> Self` — ローカル図形群から1レイヤー・1フレームの初期シンボル定義を作成。
  - `collect_visible_local_shapes_at(&self, internal_frame: usize, library: &[SymbolDef], depth: usize) -> Vec<FillShape>` — 指定した内部フレームにおける全可視レイヤーのローカル図形を収集（入れ子シンボル・内部トゥイーンも再帰評価）。
  - `resolve_internal_frame(&self, binding: &SymbolInstanceBinding, keyframe_offset: usize) -> usize` — インスタンスの再生モード（`Loop` / `PlayOnce` / `SingleFrame`）と `first_frame`、および親キーフレーム先頭からの経過フレーム数 `keyframe_offset` から表示すべき内部フレーム番号（0始まり）を算出。
  - `evaluated_local_shapes_for_instance(&self, binding: &SymbolInstanceBinding, keyframe_offset: usize, library: &[SymbolDef], depth: usize) -> Vec<FillShape>` — インスタンス設定に基づいて評価されたローカル図形群を返す。
  - `evaluated_local_shapes_at(&self, parent_frame: usize, library: &[SymbolDef], depth: usize) -> Vec<FillShape>` — デフォルトのループ評価でローカル図形群を返す（ライブラリのミニプレビュー等で使用）。
  - `update_from_working_timeline(&mut self, timeline: &Timeline)` — インプレイス編集中の作業タイムラインからレイヤー構造と `total_frames` をシンボル定義へ反映。
- **`struct SymbolEditSession`** — シンボルをダブルクリックしてインプレイス編集する際の親タイムライン退避セッション（`symbol_id`, `instance_gid`, `instance_xform`, `parent_timeline`, `parent_editing_group_path`）。

### `Cel` / `Layer` / `Timeline` の拡張メソッド＆フリー関数（`symbol.rs` 内）
- **`Cel::single_symbol_instance(&self) -> Option<SymbolInstanceBinding>`** — セル内の図形が生の絵や別グループを含まず、**ただ1つのシンボルインスタンスのみ**で構成されている場合にその `SymbolInstanceBinding` を返す（クラシックトゥイーンの成立条件判定）。
- **`Cel::rebuild_symbol_instances_with(&mut self, library: &[SymbolDef], mut map_binding: impl FnMut(SymbolInstanceBinding) -> SymbolInstanceBinding) -> bool`** — セル内のすべてのシンボルインスタンスを最新のライブラリ定義および変換関数 `map_binding` で一括再構築する共通ヘルパー。
- **`Layer::tween_state_at_keyframe(&self, kf_idx: usize) -> TweenState`** — 指定キーフレームのクラシックトゥイーン状態（`None` / `Incomplete` / `Complete`）を判定。
- **`Layer::interpolated_tween_binding_at(&self, frame: usize) -> Option<(SymbolInstanceBinding, usize)>`** — トゥイーン補間区間内の指定フレームにおける補間済み `SymbolInstanceBinding`（位置・回転・スケールのTRS分解補間）とキーフレーム先頭からの経過フレーム数 `keyframe_offset` を返す。
- **`Timeline::has_multiframe_symbol(&self) -> bool`** — ライブラリ内に2フレーム以上のタイムラインを持つシンボルが存在するか判定。
- **`Timeline::resolve_shapes_with_symbols(&self, raw_shapes: &[FillShape], kf_start_frame: usize, frame: usize) -> Vec<FillShape>`** — セル内の図形配列に対し、シンボルインスタンスを現在フレーム・再生モードに応じた内部フレームの図形へ展開。
- **`Timeline::evaluate_layer_shapes_at(&self, layer_idx: usize, frame: usize) -> Option<Vec<FillShape>>`** — 指定レイヤー・指定フレームで描画すべき図形一覧を、クラシックトゥイーン補間またはシンボル再生モード展開を行って返す。
- **`Timeline::build_layer_resolved_world_batch_mesh(&self, layer_idx: usize, frame: usize) -> Option<egui::Mesh>`** — トゥイーン補間・シンボル展開後の図形群からワールド座標の結合メッシュを生成。
- **`Timeline::resolved_cel_screen_mesh(&self, layer_idx: usize, frame: usize, zoom: f32, pan: Vec2, clip_to_stage: bool) -> Option<Arc<egui::Mesh>>`** — トゥイーン・多フレームシンボルがあれば動的評価メッシュ、なければ `Cel::cached_screen_mesh` のゼロコピーキャッシュを返す。
- **`Timeline::resolved_cel_onion_mesh(&self, layer_idx: usize, frame: usize, zoom: f32, pan: Vec2, tint: Color32) -> Option<Arc<egui::Mesh>>`** — オニオンスキン描画用の解決済みメッシュを返す。
- **`draw_registration_cross(painter: &egui::Painter, center: Pos2, arm_len: f32, color: Color32)`** — シンボル基準点（`+` 十字マーク）を描画するフリー関数。
- **`transform_layers_shapes(layers: &[Layer], xform: Affine) -> Vec<Layer>`** — レイヤー配列内の全キーフレームの図形およびシンボルインスタンス行列にアフィン変換を一括適用するフリー関数。

### `FlashBrushApp` のメソッド（`symbol.rs` 内）
- **シンボル変換・ライブラリ管理**：
  - `convert_selection_to_symbol(&mut self) -> Option<u64>` — 選択中の図形群の中心を `(0, 0)` 基準に正規化して新規シンボルとしてライブラリへ登録し、ステージ上の選択図形をそのシンボルインスタンスへ置き換える（`F8`）。
  - `place_symbol_instance_from_library(&mut self, symbol_id: u64) -> bool` — ライブラリ内の指定シンボルをステージ中央へ新しいインスタンスとして配置。
  - `delete_symbol_from_library(&mut self, symbol_id: u64) -> bool` — 指定シンボルをライブラリから削除し、タイムライン上の全インスタンスを通常のグループ（ベクター図形）へ自動分解。
- **クラシックトゥイーン操作**：
  - `can_toggle_tween_at_current_frame(&self) -> bool` — 現在のアクティブレイヤー・現在フレームのキーフレームでクラシックトゥイーンを切り替え可能か判定。
  - `timeline_toggle_tween(&mut self) -> bool` — 現在フレームが属するキーフレームのクラシックトゥイーン（`T` キー）を ON/OFF 切り替え。
- **シンボル再生オプション（ループ / 1回再生 / 任意のフレーム）**：
  - `selected_single_symbol_info(&self) -> Option<(SymbolInstanceBinding, String, usize)>` — キャンバス上で単一のシンボルインスタンスのみが選択されている場合、その `(binding, シンボル名, シンボル総フレーム数)` を返す。
  - `select_symbol_instance_by_gid(&mut self, instance_gid: u64)` — 指定 `instance_gid` を持つ全構成図形を再選択するヘルパー。
  - `set_selected_symbol_playback(&mut self, new_mode: SymbolLoopMode, new_first_frame: usize) -> bool` — 選択中の単一シンボルインスタンスの再生モードと開始/指定フレームを変更し、現在フレームの表示を即座に再評価。
- **シンボル内タイムライン編集（インプレイス編集）**：
  - `enter_symbol_timeline(&mut self, binding: SymbolInstanceBinding) -> bool` — シンボルインスタンスをダブルクリックした際、親タイムラインを `symbol_edit_stack` に退避し、シンボルのローカルタイムラインとステージへ切り替える。
  - `exit_symbol_timeline(&mut self) -> bool` — シンボル内タイムラインでの編集結果をライブラリ定義へ保存し、親タイムラインへ復帰して全インスタンスの表示を更新。
  - `sync_active_symbol_edit(&mut self)` — シンボル編集中にタイムラインが更新された際、直ちに親セッション側のライブラリ定義へ変更を同期。
  - `refresh_symbol_instances_in_timeline(timeline: &mut Timeline)` — タイムライン内の全レイヤー・全キーフレームに配置されているシンボルインスタンスを最新のライブラリ定義で再構築。
- **シンボル関連UI描画（プロパティパネル＆ライブラリタブ）**：
  - `draw_selected_symbol_properties(&mut self, ui: &mut egui::Ui)` — 単一シンボル選択時に右プロパティパネルへ表示される「シンボル再生設定」（`ループ` / `1回再生` / `任意のフレーム` 切り替え、フレーム番号 `DragValue`、クイックフレーム選択ボタン、タイムライン編集ボタン）を描画。
  - `draw_library_tab(&mut self, ui: &mut egui::Ui)` — 右パネルの `ライブラリ` タブ（選択図形のシンボル登録ボタン、登録済みシンボル一覧カード、名前インライン編集、ミニプレビュー、配置・編集・削除ボタン）を描画。
  - `draw_symbol_mini_preview(ui: &mut egui::Ui, sym: &SymbolDef, library: &[SymbolDef])` — ライブラリカード左側のシンボル縮小プレビューを描画。

---

## 10. [`vector_engine.rs`](./vector_engine.rs) — 2Dベクター幾何・曲線フィット・ブーリアン演算・メッシュ化エンジン

### 定数・アトミックカウンタ
- **`REFERENCE_BRUSH_RADIUS: f64`** (`8.0`) — ブラシ太さに依存せず一定のスムージング品質を保つための基準ブラシ半径。
- **`NEXT_MESH_REVISION: AtomicU64`** / **`next_mesh_revision() -> u64`** — 図形メッシュが再構築されるたびに一意なリビジョン番号を発行するアトミックカウンタ。

### 構造体・列挙型（`vector_engine.rs` 内）
- **`struct Contour`** — 閉じた2次ベジェ曲線（`segments: Vec<QuadBez>`）の輪郭ループ（外周または穴）。
  - `translate(&mut self, delta: Vec2)` / `transform(&mut self, affine: Affine)` — 輪郭の平行移動およびアフィン変換。
  - `append_to_bez_path(&self, path: &mut BezPath)` — `kurbo::BezPath` へ輪郭パスを追記。
  - `signed_area(&self) -> f64` — 輪郭の符号付き面積を計算（スクリーン座標系で外周時計回り `> 0`、穴反時計回り `< 0`）。
  - `cleanup_degenerate(&mut self)` — 長さほぼゼロの退化セグメントを除去し、端点の連続性を保証。
  - `from_segments_oriented(segments: Vec<QuadBez>, is_exterior: bool) -> Self` / `reverse(&mut self)` / `enforce_continuity(&mut self)` — ワインディング向きを揃えた `Contour` の構築・反転・接続点補正。
  - `insert_anchor_at(&mut self, seg_idx: usize, t: f64) -> bool` — De Casteljau 分割（`subsegment`）により曲線形状を保ったまま指定パラメータ `t` にアンカーポイントを挿入。
  - `delete_anchor_at(&mut self, seg_idx: usize) -> bool` — 指定アンカーポイントを削除し、前後のアンカー間を直線セグメント（中点制御点）で繋ぐ。
- **`struct FillShape`** — 単一カラーの塗りベクター図形（`color: Color32`, `exterior: Contour`, `holes: Vec<Contour>`, `layer_id: u64`, `group_path: Vec<u64>`, `symbol_instance: Option<SymbolInstanceBinding>`, `cached_bbox: Rect`, `cached_bez_path: BezPath`, `cached_local_mesh: Arc<egui::Mesh>`, `cached_polka_dot_mesh: OnceLock<Arc<egui::Mesh>>`, `mesh_revision: u64`）。
  - `new(color: Color32, exterior: Contour, holes: Vec<Contour>) -> Self`
  - `with_layer_id(self, layer_id: u64) -> Self` / `with_group_path(self, group_path: Vec<u64>) -> Self` / `with_scope(self, layer_id: u64, group_path: Vec<u64>) -> Self`
  - `contours(&self) -> impl Iterator<Item = &Contour>` — 外周とすべての穴を順に走査するイテレータ。
  - `is_in_editing_scope(&self, editing_scope: &[u64]) -> bool` — 図形が現在のグループ編集スコープ内に属するか判定。
  - `is_raw_at_scope(&self, editing_scope: &[u64]) -> bool` — 現在の編集スコープにおいてグループ・シンボルに属さない「生のシェイプ（Merge Drawing 対象）」であるか判定。
  - `active_group_id_at_scope(&self, editing_scope: &[u64]) -> Option<u64>` — 現在の編集スコープ直下の子グループID（またはシンボルインスタンスID）を返す。
  - `rebuild_cache(&mut self)` — バウンディングボックス、`BezPath`、ローカル三角形メッシュ、メッシュリビジョンを再計算。
  - `bounding_box(&self) -> Rect` / `contains_point(&self, pt: Point) -> bool`（NonZero ワインディング判定）
  - `translate(&mut self, delta: Vec2)` — 制御点とキャッシュ済み頂点座標を直接平行移動（再テッセレーション不要の高速移動）。
  - `transform(&mut self, affine: Affine)` — アフィン変換を適用し、負の行列式（反転）の場合は外周・穴のワインディング向きを自動補正してキャッシュを再構築。
  - `selection_overlay_mesh(&self) -> Arc<egui::Mesh>` — 選択時の白い網点（Polka-Dot）メッシュを遅延生成・キャッシュ取得。
  - `contour_count(&self) -> usize` / `get_contour(&self, idx: usize) -> Option<&Contour>` / `get_contour_mut(&mut self, idx: usize) -> Option<&mut Contour>` — インデックス（`0`＝外周、`1..`＝穴）による輪郭アクセス。
- **`enum BoolOp`** — ブーリアン演算種別（`Union`, `Difference`, `Intersection`）。
- **`struct QuadRecord`** / **`struct TaggedEdge`** — ブーリアン演算後のポリゴン辺から元の2次ベジェ曲線セグメントを逆引き・De Casteljau 部分抽出するための内部レコード。

### フリー関数（`vector_engine.rs` 内）
- **アフィン変換・マーキー矩形生成**：
  - `affine_scale_around(pivot: Point, sx: f64, sy: f64) -> Affine` — 指定ピボット中心のスケール行列を生成。
  - `affine_rotate_around(pivot: Point, angle_rad: f64) -> Affine` — 指定ピボット中心の回転行列を生成。
  - `interpolate_affine_with_pivot(start: Affine, start_pivot: Point, end: Affine, end_pivot: Point, t: f64) -> (Affine, Point)` — 2つのアフィン変換行列をローカルピボット `start_pivot`, `end_pivot` を軸として TRS（ピボットのワールド平行移動・最短回転角度・スケール・シアー）に分解し、滑らかに補間（クラシックトゥイーン用）。
  - `decompose_affine_trs(c: [f64; 6]) -> (f64, f64, f64, f64, f64, f64)` — 2Dアフィン行列係数を `(tx, ty, angle, sx, sy, shear)` へ極分解。
  - `make_marquee_rect_shape(p0: Point, p1: Point, color: Color32) -> Option<FillShape>` — 2つの対角座標から矩形の `FillShape` を生成。
- **アルゴリズム1：ブラシ軌跡からの閉輪郭（アウトライン）生成**：
  - `generate_brush_outline(points: &[Point], radius: f64) -> Vec<Point>` / `generate_brush_outline_at_zoom(points: &[Point], radius: f64, view_zoom: f64) -> Vec<Point>` — ブラシ中心軌跡点列から進行方向左右の法線オフセット点列と始点・終点の半円キャップを計算し、単一の閉じた輪郭ポリゴンを生成（微小手ブレ除去フィルタ＋中心スパイン重み付け平均を含む）。
- **アルゴリズム2：100%スムージング＆G1連続2次ベジェ曲線フィッティング**：
  - `build_smoothed_stroke_shapes(raw_outline: &[Point], color: Color32) -> Vec<FillShape>` / `build_smoothed_stroke_shapes_at_zoom(raw_outline: &[Point], color: Color32, view_zoom: f64) -> Vec<FillShape>` — 生アウトラインを `i_overlay::SimplifyShape` で自己交差解消したのち、各外周・穴リングを滑らかな2次ベジェ曲線へ変換。
  - `estimate_brush_radius_from_shape_rings(rings: &[Vec<[f64; 2]>]) -> f64` / `estimate_brush_radius_from_ring(ring: &[[f64; 2]]) -> f64` — 輪郭の面積と周囲長から実効ブラシ半径 `r_est` を推定。
  - `smooth_ring_to_quad_contour(ring: &[[f64; 2]], is_exterior: bool, r_est: f64) -> Option<Contour>` — 単一の閉リングから特徴点分類・Taubin ゼロ収縮平滑化・2次ベジェ曲線フィット・G1連続化を行って `Contour` を構築。
  - `resample_closed_ring_uniform(pts: &[Point], step: f64) -> Vec<Point>` — 閉リングを等間隔弧長でリサンプリング。
  - `find_true_runs(flags: &[bool]) -> Vec<(usize, usize)>` / `clamp_run_bounds(s: usize, e: usize, n: usize, max_half_steps: usize) -> (usize, usize)` — 周期配列上の連続する `true` 区間の検出とクランプ。
  - `classify_and_align_ring_features(resampled: &mut [Point], cum_arc: &[f64], total_len: f64, step: f64, scale: f64, r_est: f64) -> (Vec<bool>, Vec<bool>)` — 閉リング上の各頂点を「シャープな角（折れ角）」と「丸端キャップ／円弧」に分類し、真の角頂点へ位置を整列。
  - `detect_sharp_corners(resampled: &[Point], step: f64, scale: f64, r_est: f64) -> Vec<bool>` — マルチスケール弦方向角度から鋭角・直角コーナーを検出。
  - `detect_round_arcs(resampled: &[Point], is_sharp_corner: &[bool], step: f64, r_est: f64) -> Vec<bool>` — 曲率が一貫して高い半円キャップや小円ループ領域を検出。
  - `filter_protected_round_features(resampled: &mut [Point], cum_arc: &[f64], total_len: f64, step: f64, r_est: f64, is_sharp_corner: &[bool], is_round_arc: &[bool]) -> Vec<bool>` — 保護すべき丸端キャップと真のコーナーを選別し、外角・内角の頂点座標を交線方向へスナップ補正。
  - `apply_zero_shrinkage_smoothing(pts: &[Point], is_sharp_corner: &[bool], is_protected_round: &[bool], step: f64, scale: f64) -> Vec<Point>` — コーナーと丸端キャップを保護しつつ直線・緩曲線のジャギーを除去する Taubin 膨張・収縮フィルタ。
  - `select_locked_feature_anchors(smoothed: &[Point], cum_arc: &[f64], total_len: f64, is_sharp_corner: &[bool], is_protected_round: &[bool], r_est: f64) -> (Vec<usize>, Vec<bool>)` — ベジェ分割の初期ロックアンカーとコーナーフラグを選出。
  - `find_longest_false_run_midpoint(flags: &[bool]) -> Option<usize>` — 非特徴区間の最長ランの中点を返す。
  - `fit_quad_control_point_with_arc_lengths(pts: &[Point], arc_lens: &[f64]) -> Point` / `fit_quad_control_point_least_squares(pts: &[Point]) -> Point` — 弧長パラメータに基づく最小二乗法で最適な2次ベジェ制御点 `P1` を算出。
  - `enforce_g1_tangent_continuity_on_segments(segments: &mut [QuadBez], anchor_is_corner: &[bool])` — 非コーナーアンカーにおいて前後の2次ベジェ曲線の接線方向を一直線（G1連続）に揃える。
  - `reduce_outline_anchors_by_quad_fit(pts: &[Point], cum_arc: &[f64], total_len: f64, locked_indices: &[usize], locked_is_corner: &[bool], fit_tol: f64, rdp_eps: f64) -> (Vec<usize>, Vec<bool>)` — 2次ベジェ曲線1本で許容誤差内に収まるスパンはアンカーを増やさず、収まらないスパンのみ再帰的に分割して最小限のアンカー列を構築。
  - `fill_cyclic_span(pts: &[Point], cum_arc: &[f64], total_len: f64, start: usize, end: usize, span_pts: &mut Vec<Point>, span_arcs: &mut Vec<f64>)` — 閉リング上の `start..=end` 区間の点列と累積弧長を取り出す。
  - `span_turn_metrics(pts: &[Point]) -> (f64, f64)` — スパン内の累積回転角とネット回転角を算出。
  - `max_quad_fit_error_with_arc_lengths(pts: &[Point], arc_lens: &[f64], q: QuadBez) -> f64` — 点列と2次ベジェ曲線の最大乖離誤差を計算。
  - `max_deviation_from_chord(pts: &[Point]) -> (usize, f64)` / `rdp_recursive(pts: &[Point], eps: f64, keep: &mut [bool])` — Ramer-Douglas-Peucker 頂点間引きアルゴリズム。
- **アルゴリズム3：マージ描画（`i_overlay` ブーリアン演算＆De Casteljau 2次ベジェ再構築）**：
  - `merge_shapes_into_canvas(canvas: &mut Vec<FillShape>, new_shapes: Vec<FillShape>)` — 新規図形群を同じレイヤー・同じグループスコープ内の生シェイプに対して同色 `Union`・異色 `Difference` でマージ統合。
  - `merge_disjoint_fill_into_canvas(canvas: &mut Vec<FillShape>, fill_shape: FillShape)` — バケツ塗りで検出された閉領域を周囲の同色生シェイプとのみ `Union` 統合（異色シェイプのDifference切り抜きを省略する高速パス）。
  - `merge_shapes_into_canvas_inner(canvas: &mut Vec<FillShape>, new_shapes: Vec<FillShape>, cut_different_color: bool)` — マージ描画の内部共通実装。
  - `erase_from_canvas(canvas: &mut Vec<FillShape>, eraser_shapes: &[FillShape], target_layer_id: u64, editing_scope: &[u64])` — 消しゴム図形で対象スコープ内の生シェイプを `Difference` 減算。
  - `union_bounding_boxes(shapes: &[FillShape]) -> Option<Rect>` — 図形配列の統合バウンディングボックスを計算。
  - `boolean_operation(subject: &[FillShape], clip: &[FillShape], op: BoolOp, result_color: Color32) -> Vec<FillShape>` — `subject` と `clip` の2次ベジェ図形群に対して `Union` / `Difference` / `Intersection` を実行し、結果を再び滑らかな2次ベジェ `FillShape` 群として再構築。
  - `run_float_overlay(subj_shapes: &[Vec<Vec<[f64; 2]>>], clip_shapes: &[Vec<Vec<[f64; 2]>>], rule: OverlayRule) -> Vec<Vec<Vec<[f64; 2]>>>` — `i_overlay::FloatOverlay` の実行ラッパー。
  - `reconstruct_fill_shape_from_rings(shape_rings: &[Vec<[f64; 2]>], source_quads: &[QuadRecord], tagged_edges: &[TaggedEdge], result_color: Color32) -> Option<FillShape>` — ブーリアン演算結果のポリゴンリング群から `FillShape` を復元。
  - `find_enclosed_region_at(canvas: &[FillShape], target_pt: Point, fill_color: Color32, workspace_bounds: Rect) -> Option<FillShape>` — バケツツール用：キャンバス全体の輪郭で作業領域を分割し、クリック座標を含む閉領域（穴）を抽出してベクター図形化。
  - `flatten_shape_for_overlay(shape: &FillShape, source_quads: &mut Vec<QuadRecord>, tagged_edges: &mut Vec<TaggedEdge>) -> Vec<Vec<[f64; 2]>>` — `FillShape` の各 `QuadBez` をタグ付き折れ線に細分化して `i_overlay` 入力形式へ変換。
  - `resolve_deformed_shape(shape: &FillShape) -> Vec<FillShape>` — エッジ引き伸ばし・頂点移動後の輪郭に対して `SimplifyShape` を実行し、自己交差の解消や2図形への横断分割を解決。
  - `quad_subdivision_steps(q: &QuadBez) -> usize` — `QuadBez` の長さと曲率に応じた適切な分割ステップ数を算出。
  - `sample_contour_ring_for_overlay(contour: &Contour, is_exterior: bool) -> Vec<[f64; 2]>` / `flatten_contour_preserving_direction(contour: &Contour, steps_per_seg: usize) -> Vec<[f64; 2]>` / `flatten_contour_for_overlay(contour: &Contour, is_exterior: bool, source_quads: &mut Vec<QuadRecord>, tagged_edges: &mut Vec<TaggedEdge>) -> Vec<[f64; 2]>` — 輪郭のポリゴンサンプリングと元セグメント追跡タグ付け。
  - `reconstruct_or_refit_quad_contour(ring: &[[f64; 2]], is_exterior: bool, source_quads: &[QuadRecord], tagged_edges: &[TaggedEdge]) -> Option<Contour>` / `match_edge_to_quad(a: Point, b: Point, source_quads: &[QuadRecord], tagged_edges: &[TaggedEdge]) -> Option<(usize, f64, f64)>` / `refit_open_polyline_to_quads(pts: &[Point]) -> Vec<QuadBez>` / `extract_sub_quad(q: QuadBez, t0: f64, t1: f64) -> QuadBez` — ブーリアン演算後の各ポリゴン辺を元の `QuadBez` セグメントへ照合し、De Casteljau 法（`extract_sub_quad`）で無劣化に部分曲線を切り出して閉じた2次ベジェ輪郭へ再構築。
- **幾何ユーティリティ関数**：
  - `ring_f64_to_points(ring: &[[f64; 2]]) -> Vec<Point>` / `normalize_vec(v: Vec2) -> Vec2` / `left_normal(v: Vec2) -> Vec2` / `signed_angle(a: Vec2, b: Vec2) -> f64` / `to_pos2(p: Point) -> Pos2`
  - `fill_cumulative_arc_lengths(pts: &[Point], out: &mut Vec<f64>) -> f64` / `dedup_closed_ring(pts: &mut Vec<Point>, eps: f64)`
  - `signed_area_f64(ring: &[[f64; 2]]) -> f64` / `ring_area_and_perimeter_f64(ring: &[[f64; 2]]) -> (f64, f64)` / `polygon_signed_area(pts: &[Point]) -> f64`
  - `point_to_segment_dist(p: Point, a: Point, b: Point) -> f64` / `rects_overlap(a: Rect, b: Rect) -> bool`
- **`lyon` テッセレーション＆スクリーンメッシュ変換**：
  - `tessellate_contours_to_mesh(exterior: &Contour, holes: &[Contour], color: Color32) -> egui::Mesh` — 外周と穴の2次ベジェ輪郭を `lyon`（`FillRule::NonZero`）で三角形メッシュ化。
  - `build_polka_dot_selection_mesh(local_mesh: &egui::Mesh) -> egui::Mesh` — 図形の三角形メッシュとグリッド状の小さな正方形群の交差から、Flash 風の白い網点（Polka-Dot）選択オーバーレイメッシュを生成。
  - `tessellate_raw_polygon(tessellator: &mut FillTessellator, points: &[Point], color: Color32, zoom: f32, pan: Vec2) -> egui::Mesh` — ドラッグ中の生アウトラインポリゴンを直接スクリーン座標メッシュへ分割。
  - `tessellate_lyon_path(tessellator: &mut FillTessellator, path: &LyonPath, color: Color32) -> egui::Mesh` — `lyon::path::Path` から `egui::Mesh` への変換コア。
  - `transform_mesh_to_screen(local_mesh: &egui::Mesh, zoom: f32, pan: Vec2) -> egui::Mesh` / `transform_mesh_to_screen_onion(local_mesh: &egui::Mesh, zoom: f32, pan: Vec2, onion_color: Color32) -> egui::Mesh` — ワールド座標メッシュの頂点配列を現在のズーム・パンでスクリーン座標へ一括変換（通常用およびオニオンスキン単色用）。
  - `append_mesh(dst: &mut egui::Mesh, src: &egui::Mesh)` — 複数の `egui::Mesh` の頂点・インデックス配列を1つのバッチメッシュへ結合。

---

## 11. [`tests.rs`](./tests.rs) — 自動ユニットテストスイート（全22件）

`cargo test` で実行される全22件のユニットテストとテスト用ヘルパー関数：

- **ヘルパー関数**：
  - `make_stroke(p0: Point, p1: Point, radius: f32, color: Color32) -> Vec<FillShape>` — 2点間を結ぶ直線ブラシストロークを生成・スムージングして返す。
- **ユニットテスト一覧（全22件）**：
  1. `test_same_color_union_and_diff_split_with_i_overlay` — 同色ストロークの `Union` 融合と、異色ストローク横断による `Difference` 分断を検証。
  2. `test_smoothing_100_preserves_width_round_caps_and_minimal_anchors` — 100%スムージング時に太さが痩せず、両端の丸キャップが保たれ、最小限のアンカー数に収まることを検証。
  3. `test_curved_stroke_has_strictly_uniform_width` — 手ブレを含む曲線ストロークでも全長にわたり厳密に一定の線幅が保たれることを検証。
  4. `test_hand_drawn_rectangle_at_smoothing_100_keeps_corners_and_straight_sides` — 手描き四角形で四隅の直角コーナーと直線辺、中央の穴が潰れず保持されることを検証。
  5. `test_hand_drawn_circle_at_smoothing_100_is_round_and_uniform_width` — 手描き円ストロークが均一な太さの滑らかな円環になることを検証。
  6. `test_thin_2px_stroke_and_su_loop_keep_round_caps_and_uniform_width` — 半径2pxの極細線や小さなループ（「す」の結び目）でも丸端と穴が維持されることを検証。
  7. `test_pull_edge_across_circle_splits_into_two_shapes` — 円の輪郭エッジを反対側の円周上・円周外へ突っ切るようにドラッグすると2つの図形に分割されることを検証。
  8. `test_add_and_delete_anchor_on_contour` — 輪郭上のアンカー追加（`+`：De Casteljau 分割）と削除（`-`：前後アンカー間の直線化）を検証。
  9. `test_bucket_fill_enclosed_hole_and_recolor` — バケツツールによる開いた背景の無視、閉領域（穴）への色流し込み、既存図形のリカラー、同色バケツによる周囲との `Union` 結合を検証。
  10. `test_marquee_select_partial_slice_and_shift_multi_select` — 矩形選択による部分スライス、未編集選択解除での無劣化復元、`Shift` 連続スライス選択、削除と1回の Undo による完全復元を検証。
  11. `test_keep_selection_on_tool_switch_and_bucket_fill_selected_region_only` — ツール切替時の選択維持と、選択領域内のみへのバケツ色流し込みを検証。
  12. `test_free_transform_rotate_scale_and_flip` — 自由変形ツール（`Q`）の90°回転、スケール縮小、カスタムピボット基準の反転とワインディング自動補正を検証。
  13. `test_eyedropper_picks_topmost_shape_color` — スポイトツール（`I`）による最前面図形カラーの抽出を検証。
  14. `test_nested_grouping_ungrouping_and_edit_in_place` — 複数段の入れ子グループ化（`Cmd+G`）、グループ移動時の非切り抜き共存、ダブルクリックによる階層インプレイス編集、段階的グループ解除（`Cmd+B`）を検証。
  15. `test_zoom_around_cursor_and_fine_brush_at_high_zoom` — カーソル中心ズームの座標不変性、800%拡大時の0.5px極細ストローク生成、ズーム倍率に依存しないスムージング結果の一致を検証。
  16. `test_copy_cut_paste_and_paste_in_place` — カット（`Cmd+X`）、同じ位置にペースト（`Cmd+Shift+V`）と選択解除時のシームレス再結合、グループのコピー＆オフセットペースト（`Cmd+V`）での新規グループID採番を検証。
  17. `test_timeline_layer_keyframe_and_cel_lookup` — `F6`/`F7` キーフレーム進行、`F5`/`Shift+F5` フレーム延長・削除、複数レイヤー間の Merge Drawing 分離、複数レイヤー横断マーキー選択、レイヤーロック保護、レイヤーD&D並べ替え、セルクリック全選択、フレームのドラッグ移動＆`Alt` 複製、3段階メッシュキャッシュと再生停止時の復元を検証。
  18. `test_adobe_animate_click_selects_edge_and_selected_shape_disables_deformation` — 細線の頂点・エッジ上クリックでの確実な選択と、選択状態（網点表示）での変形無効化・移動優先挙動を検証。
  19. `test_g1_tangent_continuity_and_spine_smoothing` — 全アンカーでの G1 接線連続性、V字・L字コーナーの保持、「あ」の交差部ノッチの保護、波線や非対称な小円の形状維持を検証。
  20. `test_symbol_conversion_library_placement_and_inplace_sync` — `F8` シンボル変換、ライブラリからの複数インスタンス配置、ダブルクリックによるシンボル内タイムライン編集（複数フレーム化）、親シーンでのループ評価を検証。
  21. `test_classic_tween_single_symbol_condition_and_interpolation` — 単一シンボルのみでのクラシックトゥイーン（`T`）許可条件、未完トゥイーン（横点線）と完全トゥイーン（実線矢印）の遷移、中間フレームのTRS線形補間、元キーフレームの非破壊保護を検証。
  22. `test_symbol_playback_modes_loop_play_once_and_single_frame` — シンボルインスタンスごとの再生モード（`ループ` / `1回再生` / `任意のフレーム`）と開始フレーム指定の動作を検証。
