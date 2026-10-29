# `src/` ディレクトリ完全リファレンス（全13ファイル・全型・全関数一覧）

`is2d` のソースコードは、UIパネル・キャンバスツール・ベクター幾何エンジン（コア／ストローク平滑化／ブーリアン演算）・タイムラインデータ・タイムラインUI・シンボル＆トゥイーン・テストの責務ごとに **全13ファイル** に分割されています。
本ドキュメントでは、`src/` 内のすべてのファイル・構造体・列挙型・定数・メソッド・ヘルパー関数・ユニットテストを漏れなく記載します。

---

## ファイル構成一覧（早見表）

| # | ファイル | 行数目安 | 主な責務 |
| :-: | :--- | :---: | :--- |
| 1 | **[`main.rs`](./main.rs)** | ~850行 | **Layer 5 (App Shell)**：アプリ起動、フォント・テーマ初期化、アプリ全体状態（`Is2dApp` / `AppUiState` / `UndoHistory`）、唯一の `impl Is2dApp`（トランザクション型キャンバス更新API・共通ステータス通知バナー・ショートカット）、メインループ |
| 2 | **[`panels.rs`](./panels.rs)** | ~1,260行 | **Layer 4 (UI Presentation)**：Material Icons 定数（`mod icons`）、OS別修飾キー表記定数（`MOD_KEY`）、左端縦ツールバー、ステージ上部スリムバー（パンくずリスト）、右端4タブパネル（`プロパティ` / `カラー` / `ライブラリ` / `キャンバス`、およびトゥイーン設定・シンボル再生オプション・ライブラリ一覧＆ミニプレビューUI描画）のモジュール関数群 |
| 3 | **[`canvas_tool.rs`](./canvas_tool.rs)** | ~1,570行 | **Layer 3 (Tool Logic)**：`Tool` 列挙型、共通ドラッグ状態型（`ActiveCanvasDrag` / `MoveSelectionDrag` / `MarqueeDrag`）、唯一の `impl ViewportState`（ズーム・パン・ステージ枠）＆唯一の `impl CanvasState`（空白フレームシンボルマーカー統合表現・`shape_id` 二分探索ベース選択追従・非公開 `loaded_signatures` / `unsliced_canvas` 不変条件カプセル化）、キャンバスとタイムラインの同期・トランザクション（`sync_shapes_to_timeline` / `load_shapes_from_timeline` / `mutate_timeline_with_undo`）、シンボル基準点十字（`draw_registration_cross`）・キャンバス合成描画関数群 |
| 4 | **[`tool_paint.rs`](./tool_paint.rs)** | ~420行 | **Layer 3 (Tool Logic) — 描画・塗りツール**（ブラシ `B`・消しゴム `E`・バケツ塗り `K`・スポイト `I`）の入力処理、チャンク増分プレビュー（`STROKE_TAIL_CHUNK_LEN` / `draw_active_stroke_preview`）、最前面カラー抽出（`topmost_visible_color_at`）、スポイトカーソルバッジ（`draw_eyedropper_cursor_badge`）・コミット関数群 |
| 5 | **[`tool_select.rs`](./tool_select.rs)** | ~1,810行 | **Layer 3 (Tool Logic) — 選択ツール（`V`）**、選択サブモード・ホバー・ドラッグ状態型（`SelectSubMode` / `HoverTarget` / `SelectDragState`）、ヒット判定＆頂点スナップ（`compute_hover_target` / `find_snap_vertex` / `collect_marquee_hit_groups`）、マーキー範囲選択（部分スライス）、輪郭の粘土変形（Reshape）、アンカー追加/削除（`+`/`-`）、グループ階層＆インプレイス編集、コピー＆ペースト、選択ツールカーソル・バッジ描画のモジュール関数群 |
| 6 | **[`tool_transform.rs`](./tool_transform.rs)** | ~1,230行 | **Layer 3 (Tool Logic) — 自由変形ツール（`Q`）＆シンボル/トゥイーン操作コントローラ**、変形ハンドル・ホバー・ドラッグ状態型（`ScaleHandle` / `TransformHoverTarget` / `TransformDragState`）、自由変形バウンディングボックス＆カーソル描画（`draw_free_transform_box` / `draw_transform_tool_cursor`）、ピボット設定・リセット、8方向スケール・回転・平行移動・反転、および `F8` シンボル変換・ライブラリ配置/改名/削除・`T` トゥイーン切替・シンボル内タイムライン出入りコントローラ関数群 |
| 7 | **[`timeline.rs`](./timeline.rs)** | ~1,700行 | **Layer 2 (Domain / Model) — タイムラインの純粋データモデル**（UI・アプリ状態への逆依存ゼロ：唯一の `impl Timeline` / `impl Layer` / `impl Keyframe` / `impl Cel`（`Arc<Vec<FillShape>>` ＋ `Arc<Vec<SymbolInstance>>` CoW共有）、衝突ゼロ署名 `ResolvedCelSig` による `SyncCache<CelMeshCache>` 3階層共有キャッシュ、`LoadedShapeSig`、シンボル・トゥイーン評価、キーフレーム・フレーム・レイヤー操作メソッド群） |
| 8 | **[`timeline_ui.rs`](./timeline_ui.rs)** | ~1,895行 | **Layer 4 (UI Presentation) — タイムライン（X-Sheet）UI状態・描画＆コントローラ**（唯一の `impl TimelineUiState`、`TimelineGridDrag`、`TimelineLayout`、再生制御・キーフレーム/フレーム/レイヤー操作の `&mut Is2dApp` コントローラ関数群、コントロールバー、ルーラー、フレームグリッド、キーフレーム区間描画、レイヤー一覧、D&D操作関数群） |
| 9 | **[`symbol.rs`](./symbol.rs)** | ~920行 | **Layer 2 (Domain / Model) — シンボル＆ライブラリ＆クラシックトゥイーンの純粋データモデル**（UI・アプリ状態への逆依存ゼロ：唯一の `impl SymbolManager` / `impl SymbolDef`（内部に `timeline: Timeline` とミニプレビューメッシュキャッシュ `mini_preview_cache` を保持）、`SymbolInstance`（`shape_order_index` 付き）、穴輪郭ハッシュを含む複合キー `StageShapeCacheKey` による2階層 $O(1)$ ローカル復元キャッシュ `StageToLocalCache`、入れ子シンボル再帰評価、衝突ゼロ署名 `ResolvedCelSig` ＆ ヒープ確保ゼロの遅延署名照合クエリ `ResolvedSigQuery`） |
| 10 | **[`vector_engine.rs`](./vector_engine.rs)** | ~1,285行 | **Layer 1 (Geometry) — 2Dベクター幾何コア＆メッシュ描画エンジン**（他モジュールへの依存ゼロ：スレッドセーフな `SyncCache<T>`、`SymbolLoopMode` / `TweenRotate` / `SymbolInstanceBinding`、`Contour` / `FillShape`（不変エンティティID `shape_id`＋`OnceLock` 遅延メッシュキャッシュ＋スクリーンメッシュキャッシュ＋選択ドット柄スクリーンメッシュキャッシュ＋`geometry_bounding_box()`）、CoW `Arc` キャッシュ＆異常値ガード付きアフィン変換、`union_bounding_boxes`、`arc_mesh_to_shape` ゼロコピー `Arc::unwrap_or_clone`、`lyon` 三角形メッシュ化） |
| 11 | **[`vector_stroke.rs`](./vector_stroke.rs)** | ~1,545行 | **Layer 1 (Geometry) — アルゴリズム 1 & 2：ストローク輪郭生成＆100%スムージング曲線フィット**（`vector_engine` のみに依存：スケール不変ブラシ輪郭生成、特徴点ロック、ゼロ収縮平滑化、G1連続2次ベジェ曲線フィット） |
| 12 | **[`vector_boolean.rs`](./vector_boolean.rs)** | ~985行 | **Layer 1 (Geometry) — アルゴリズム 3：マージ描画＆ブーリアン演算エンジン**（`vector_engine` と `vector_stroke` のみに依存：`BoolOp`、事前フラットニングキャッシュ `PreparedClip`、`i_overlay` ブーリアン演算、`QuadSpatialGrid` 2D空間インデックスによる元の `QuadBez` タグ照合＆De Casteljau 部分曲線再構築、AABBクラスタ事前カリング付き閉領域バケツ検出、自己交差解消） |
| 13 | **[`tests.rs`](./tests.rs)** | ~2,905行 | 全機能および異常値・クラッシュ耐性を検証する自動ユニットテストスイート（全24テスト） |

---

## 1. [`main.rs`](./main.rs) — エントリポイント・アプリ状態・Undo/Redo・ショートカット・メインループ

### 定数・通知型・履歴型・初期化関数
- **`MAX_UNDO_HISTORY: usize`** (`100`) — Undo スタックが保持する最大スナップショット数。
- **`struct UndoEntry`** — Undo / Redo スタックが保持する1ステップ分の状態スナップショット（`timeline: Timeline`, `library: Arc<Vec<SymbolDef>>`, `symbol_edit_stack: Arc<Vec<SymbolEditSession>>`, `editing_group_path: Vec<u64>`, `stage_size: Vec2`, `symbols_revision: u64`）。作業キャンバス `shapes` は復元時にタイムラインから再展開することで重複保持を排除し、IDカウンタ（`next_symbol_id` / `next_group_id`）は単調増加を維持するため履歴対象外とする。さらに `UndoHistory::snapshot_symbols` によりシンボル未変更の連続操作間では `library` と `symbol_edit_stack` の `Arc` を $O(1)$ で構造共有（CoW）し、`Timeline::strip_transient_caches_with_prev` により未変更キーフレームの `Cel::shapes`（`Arc<Vec<FillShape>>`）および `Cel::instances`（`Arc<Vec<SymbolInstance>>`）も直前のスナップショットと $O(1)$ で構造共有して、実際に変更されたセルのみ一時キャッシュを破棄して軽量保持する。
- **`enum NoticeKind`** — キャンバス左上に表示するステータスメッセージの種類（`Warning` / `Error` / `Info`）と配色・アイコン定義（`colors(self)`）。
- **`struct StatusToast`** — 一定時間（デフォルト3秒）で自動消滅するワンショット通知メッセージ（`kind: NoticeKind`, `message: String`, `expires_at: Instant`）。
- **`main() -> eframe::Result<()>`** — ウィンドウサイズ（`1280 × 820`）とタイトル（`"is2d"`）を設定し、フォント・テーマを初期化して `eframe::run_native` を起動。
- **`configure_japanese_fonts(ctx: &egui::Context)`** — `fonts/NotoSansJP-Medium.ttf`（日本語表示）および `fonts/MaterialIcons-Regular.ttf`（UIアイコン）を読み込み、`Proportional` / `Monospace` フォントファミリーへ登録。
- **`configure_sharp_light_theme(ctx: &egui::Context)`** — すべてのウィジェット・パネル・メニューの角丸を `Rounding::ZERO` に統一したシャープなライトテーマを設定。

### 構造体 `Is2dApp` および責務別7サブ構造体
アプリケーション全体の状態を保持するルート構造体 `Is2dApp`（`#[derive(Default)]`）は、直下に約30個のフラットなフィールドを持つ God Object を廃し、責務ごとの独立した **7つのドメイン構造体** に分割されている（Rust のフィールド単位の借用分割により `&mut self.timeline` と `&self.canvas` / `&self.symbols` などの同時借用もゼロコストで両立）：

1. **`ui: AppUiState`**（[`main.rs`](./main.rs)）— ツール選択・ブラシ設定・表示オプション・ステータストースト通知のUI状態：
   - `tool: Tool` — 現在選択中のメインツール（初期値 `Tool::Brush`）。
   - `select_sub_mode: SelectSubMode` — 選択ツール（`V`）のサブモード（`Reshape` / `AddAnchor` / `DeleteAnchor`）。
   - `right_panel_tab: RightPanelTab` — 右端パネルのアクティブタブ状態（初期値 `RightPanelTab::Properties`）。
   - `brush_radius: f32` — ブラシ半径（ワールド座標px、初期値 `8.0`）。
   - `eraser_radius: f32` — 消しゴム半径（ワールド座標px、初期値 `22.0`）。
   - `brush_color: Color32` — 現在の描画・塗りつぶしカラー。
   - `show_wireframe: bool` — アンカーポイントとベジェ曲線パスの常時オーバーレイ表示フラグ。
   - `inspect_timeline_frame: bool` — プロパティパネル下部の表示対象フラグ（`true`: タイムラインのフレーム／トゥイーン属性、`false`: キャンバス上のシンボル属性）。
   - `status_toast: Option<StatusToast>` — 操作エラー・警告などを一定時間表示するワンショット通知。
2. **`viewport: ViewportState`**（[`canvas_tool.rs`](./canvas_tool.rs)）— キャンバスカメラ（ズーム・パン）およびステージ枠設定：
   - `zoom: f32` — キャンバスのズーム倍率（`MIN_VIEW_ZOOM: 0.15`〜`MAX_VIEW_ZOOM: 16.0`、初期値 `1.0`）。
   - `pan: Vec2` — キャンバスのパン（平行移動）オフセット。
   - `pan_last_screen_pos: Option<Pos2>` — スペースキー／中ボタンドラッグ中の直前スクリーン座標。
   - `stage_size: Vec2` — ステージ（カメラ出力枠）の幅・高さ（初期値 `800.0 × 450.0`、背景色は常時 `Color32::WHITE`）。
   - `clip_outside_stage: bool` — ステージ枠外の絵を隠すクリッピングマスク有効フラグ。
   - `last_canvas_rect: egui::Rect` — 直近フレームの中央キャンバス描画領域（「ウィンドウに合わせる」計算用）。
   - `editing_stage_size: bool` — ステージサイズ `DragValue` の連続ドラッグ・入力中フラグ（Undo重複記録防止用）。
3. **`canvas: CanvasState`**（[`canvas_tool.rs`](./canvas_tool.rs)）— 作業キャンバスの展開図形（空白フレームシンボルインスタンスのゼロ面積マーカーを含む統合表現）・選択・排他ドラッグ状態マシン・グループ階層スコープ・クリップボード・自由変形状態：
   - `shapes: Vec<FillShape>` — 現在フレームの可視・非ロックレイヤーから作業用に展開されたベクター図形リスト。現在フレームで内部図形が0個（空白キーフレーム等）のシンボルインスタンスも `FillShape::new_empty_symbol_marker` として正確な重ね順（`shape_order_index`）に展開されるため、通常のシンボル・グループと完全に単一の選択・移動・アフィン変換・コピー＆ペースト・グループ化/解除パイプラインで扱われる。
   - `loaded_signatures: Vec<LoadedShapeSig>` — `load_shapes_from_timeline` または `sync_shapes_to_timeline` 時点の `shapes` 署名一覧（Undo スナップショットに含めず、編集発生時のレイヤー別差分検知に使用）。
   - `drag: ActiveCanvasDrag` — キャンバス上で進行中のドラッグ操作（`None` / `PaintStroke { stroke, committed_mesh, tail_outline, tail_mesh, tail_start_idx }` / `Select(SelectDragState)` / `Transform(TransformDragState)`）を単一の排他的状態マシンとして保持。
   - `selected_shapes: Vec<usize>` — `canvas.shapes` 内で現在選択されている図形のインデックス一覧。
   - `unsliced_canvas: Option<Vec<FillShape>>` — マーキー選択で生シェイプを部分スライスした際、未編集で選択解除したときに無劣化復元するためのスライス前スナップショット。
   - `selection_modified: bool` — 選択中の図形に対して移動・変形・色変更・削除が行われたかどうかのフラグ。
   - `hover_target: HoverTarget` — 選択ツールでマウス直下にヒットしている要素（頂点・エッジ・塗り）。
   - `editing_group_path: Vec<u64>` — 現在インプレイス編集中のグループ階層パス（ルート階層では空）。
   - `clipboard: Vec<FillShape>` / `paste_count: usize` — コピー＆ペースト用内部クリップボードと連続ペーストのオフセット回数。
   - `transform_pivot: Option<Point>` / `transform_hover: TransformHoverTarget` — 自由変形ツール（`Q`）の基準点・ホバー状態。
4. **`timeline: Timeline`**（[`timeline.rs`](./timeline.rs)）— 現在アクティブなタイムラインのドキュメント状態（シーン1、またはインプレイス編集中のシンボル内タイムライン）。
5. **`timeline_ui: TimelineUiState`**（[`timeline_ui.rs`](./timeline_ui.rs)）— タイムラインの再生タイマー・ループ再生設定・オニオンスキン表示設定・スクロール位置・ドラッグ状態・総フレーム数編集フラグなどのUI表示・再生状態（ドキュメント本体の `Timeline` および Undo/Redo スナップショットから完全分離）。
6. **`symbols: SymbolManager`**（[`symbol.rs`](./symbol.rs)）— シンボルライブラリ・シンボル編集階層スタック・グループ/シンボルID採番カウンタ：
   - `library: Vec<SymbolDef>` — プロジェクト共通のシンボルライブラリ一覧。
   - `next_symbol_id: u64` — 次に新規作成するシンボル定義の単調増加連番ID（初期値 `1`）。
   - `selected_library_symbol: Option<u64>` — ライブラリパネルで現在選択されているシンボルのID。
   - `actively_renaming_symbol: Option<u64>` — ライブラリ一覧で現在テキスト入力中のシンボルID（連続入力中のUndo重複記録防止用）。
   - `edit_stack: Vec<SymbolEditSession>` — シンボルをダブルクリックして内部タイムラインへ潜っている際の親シーン退避スタック。
   - `next_group_id: u64` — グループID／シンボルインスタンスIDの単調増加連番採番カウンタ（初期値 `1`）。
7. **`history: UndoHistory`**（[`main.rs`](./main.rs)）— Undo / Redo スナップショット履歴管理構造体：
   - `undo_stack: VecDeque<UndoEntry>` / `redo_stack: Vec<UndoEntry>` — タイムライン・ライブラリ・シンボル編集スタック・グループ編集パスのスナップショット履歴（上限超過時は `pop_front` で $O(1)$ 破棄）。

### `AppUiState` / `UndoHistory` / `Is2dApp` のメソッド（`main.rs` 内）
- **`AppUiState::show_notice(&mut self, kind: NoticeKind, message: impl Into<String>)`** — ステータス通知トーストを3秒間の有効期限付きで登録。
- **`UndoHistory` のメソッド**：
  - `snapshot_symbols(&self, symbols: &SymbolManager) -> (Arc<Vec<SymbolDef>>, Arc<Vec<SymbolEditSession>>, u64)` — シンボル世代番号 `symbols.revision` が直近エントリと同一なら既存の `Arc` を $O(1)$ 共有し、変更時のみクローン・一時キャッシュ除去を行う。
  - `push(&mut self, entry: UndoEntry)` — 操作前スナップショットを `undo_stack` に積み（上限 `MAX_UNDO_HISTORY`）、`redo_stack` をクリア。
  - `pop_undo(&mut self) -> Option<UndoEntry>` / `pop_redo(&mut self) -> Option<UndoEntry>` / `push_redo(&mut self, entry: UndoEntry)` / `push_undo_from_redo(&mut self, entry: UndoEntry)` — Undo/Redo スタックのカプセル化された出し入れ操作。
- **Undo / Redo 履歴管理＆トランザクション型キャンバス更新（`Is2dApp`）**：
  - `restore_state_snapshot(&mut self, entry: UndoEntry) -> UndoEntry` — 指定した `UndoEntry`（タイムライン・ライブラリ・シンボル編集スタック・グループ編集パス）を `std::mem::replace` で無コピー復元し、`load_shapes_from_timeline` で作業図形を再展開（＋`symbols.sync_after_restore()` と `sanitize_editing_group_path()` で整合性を保証）して置換前の `UndoEntry` を返す。
  - `record_undo_snapshot(&mut self, snapshot: Vec<FillShape>)` — 渡された操作前スナップショットからタイムラインを組み立てて `self.history.push(...)` に記録。
  - `push_undo(&mut self)` — 現在の状態をスナップショット化して `self.history` に記録。
  - `mutate_canvas_with_undo(&mut self, clear_sel_first: bool, mutate: impl FnOnce(&mut Self) -> bool) -> bool` — キャンバス図形を変更する全ツール操作の共通トランザクションAPI。事前選択解除（オプション）→操作前スナップショット退避→クロージャ実行→変更成立時の `record_undo_snapshot`＋`sync_shapes_to_timeline` を一括保証し、手動の `push_undo` / `sync_shapes_to_timeline` 呼び出し漏れを根絶。
  - `commit_canvas_mutation(&mut self, undo_snapshot: Vec<FillShape>)` — ドラッグ開始時やスライス前に退避しておいた `undo_snapshot` を用いて、キャンバス変更の Undo 記録とタイムライン同期を不可分に確定する共通ヘルパー。
  - `undo(&mut self)` / `redo(&mut self)` — `Cmd/Ctrl + Z` および `Cmd/Ctrl + Y`（`Cmd+Shift+Z`）による履歴の巻き戻し・やり直し（実行前に `flush_canvas_before_timeline_op` で進行中ドラッグのロールバックと未確定選択のコミットを実行）。
- **キーボードショートカット処理**：
  - `handle_shortcuts(&mut self, ctx: &egui::Context)` — 再生中のショートカット制限と、単一キー／修飾キー付きショートカットの振り分け。
  - `handle_single_key_shortcuts(&mut self, ctx: &egui::Context)` — `Enter`（再生/停止）、`,` / `.`（コマ送り）、`F5` / `Shift+F5`（フレーム延長/削除）、`F6` / `Shift+F6`（複製KF/KF削除）、`F7`（空白KF）、`F8`（シンボル変換）、`T`（クラシックトゥイーン切替）、`O`（オニオンスキン）、`V`/`Q`/`B`/`E`/`K`/`I`（ツール切替）、`+`/`-`（アンカー追加/削除）、`Delete`/`Backspace`（削除）、`Esc`（選択解除またはグループ/シンボル階層から出る）を処理。
  - `handle_command_shortcuts(&mut self, ctx: &egui::Context)` — `Cmd/Ctrl + 1`（および `0`：100%表示）、`Cmd/Ctrl + 2`（ウィンドウに合わせる）、`Cmd/Ctrl + 3`（枠外を隠す）、`Cmd/Ctrl + Z/Y`（Undo/Redo）、`Cmd/Ctrl + G/B`（グループ化/解除）、`Cmd/Ctrl + C/X/V` 及び `Cmd/Ctrl + Shift + V`（コピー/カット/ペースト/同じ位置にペースト）を処理。
- **キャンバス入力振り分け・共通ステータス通知・カーソル表示**：
  - `is_current_tool_allowed(&self) -> bool` — 再生中でなく、かつ対象レイヤーが編集可能（可視・非ロック）かどうかを判定（スポイト・選択・自由変形は他レイヤーが編集可能なら許可）。
  - `handle_canvas_tool_input(&mut self, response: &egui::Response, painter: &egui::Painter, space_down: bool, shift_down: bool, alt_down: bool)` — パン中でない場合に現在のツールに応じた入力ハンドラを呼び出す。
  - `show_notice(&mut self, kind: NoticeKind, message: impl Into<String>)` / `notify_warning(&mut self, message: impl Into<String>)` / `notify_error(&mut self, message: impl Into<String>)` / `notify_info(&mut self, message: impl Into<String>)` — キャンバス左上への一時通知トースト登録ヘルパー。
  - `current_state_notice(&self) -> Option<(NoticeKind, &'static str)>` — 現在のレイヤー・フレーム状態（非表示・ロック・未作成フレーム）に応じた常駐警告メッセージを返す。
  - `draw_canvas_notices(&mut self, ctx: &egui::Context, painter: &egui::Painter, canvas_rect: egui::Rect, current_tool_allowed: bool, hover_pos: Option<Pos2>)` — 常駐ステータス警告・一時トースト通知をキャンバス左上に縦積みバナー描画。
  - `update_canvas_cursor(&self, ctx: &egui::Context, response: &egui::Response, painter: &egui::Painter, is_panning: bool, space_down: bool)` — パン中・ロック中（`CursorIcon::NotAllowed`）・通常操作中のOSカーソルとカスタムツールカーソルを更新。
- **メインループ**：
  - `eframe::App::update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame)` — 毎フレームの再生タイマー進行、ショートカット処理、左ツールバー・右プロパティパネル・下部タイムライン・上部ステージバー・中央キャンバスの描画とイベント処理を実行。

---

## 2. [`panels.rs`](./panels.rs) — アイコン定数・左ツールバー・上部ステージバー・右プロパティパネル

### モジュール `icons`・定数
- **`MOD_KEY: &str`** — プラットフォーム標準の修飾キー表記定数（macOS では `"Cmd"`、Windows / Linux では `"Ctrl"`）。
- `MaterialIcons-Regular.ttf` の Unicode コードポイント定数（全22種）：
- `ADD` (`\u{e145}`), `REMOVE` (`\u{e15b}`), `FLIP_H` (`\u{e8d4}`), `FLIP_V` (`\u{e8d5}`), `ROTATE_LEFT` (`\u{e419}`), `ROTATE_RIGHT` (`\u{e41a}`), `PIVOT_RESET` (`\u{e3b4}`), `GROUP` (`\u{e1bd}`), `SYMBOL` (`\u{e87b}`), `WIREFRAME` (`\u{e922}`), `DELETE` (`\u{e872}`), `SCENE_MOVIE` (`\u{e02c}`), `ARROW_BACK` (`\u{e5c4}`), `VISIBILITY` (`\u{e8f4}`), `LOCK` (`\u{e897}`), `SKIP_PREV` (`\u{e045}`), `STEP_PREV` (`\u{e408}`), `PLAY` (`\u{e037}`), `PAUSE` (`\u{e034}`), `STEP_NEXT` (`\u{e409}`), `SKIP_NEXT` (`\u{e044}`), `ONION_SKIN` (`\u{e3e0}`).

### 型定義
- **`enum RightPanelTab`** — 右端パネルの4タブ列挙型（`Default` は `Properties`）：
  - `Properties`（`プロパティ`）、`Color`（`カラー`）、`Library`（`ライブラリ`）、`Canvas`（`キャンバス`）。
  - `RightPanelTab::ALL` — 全4タブの定数配列。
  - `label(self) -> &'static str` — タブの表示名を返す。

### モジュール関数（`panels.rs` 内）
- **左端縦ツールバー**：
  - `draw_left_toolbar(app: &mut Is2dApp, ctx: &egui::Context)` — 幅 `44px` の左端縦ツールバーパネルを描画。
  - `add_left_selectable_icon(ui: &mut egui::Ui, btn_size: Vec2, selected: bool, icon: &str, tooltip: &str) -> bool` — ツールバー用の正方形アイコンボタンを描画するヘルパー。
  - `draw_left_main_tool_buttons(app: &mut Is2dApp, ui: &mut egui::Ui, btn_size: Vec2)` — `Tool::ALL` を走査して6つのメインツールボタンを描画。
  - `draw_color_picker_controls(app: &mut Is2dApp, ui: &mut egui::Ui)` — 左ツールバー下部の塗りカラーピッカーボタンを描画。
- **ステージ上部スリムバー**：
  - `draw_stage_header_bar(app: &mut Is2dApp, ctx: &egui::Context)` — 上部バーを描画し、左側に階層パンくずリスト（`draw_group_breadcrumb`）、右端に現在のズーム倍率ラベル（`100%`）を配置。
  - `draw_group_breadcrumb(app: &mut Is2dApp, ui: &mut egui::Ui)` — `シーン 1 › シンボル名 › Group #N` の階層パンくずリストと1階層戻るボタンを描画し、クリックで任意の親階層へ直接ジャンプ可能にする。
- **右端プロパティパネル（4タブ構成）**：
  - `draw_right_properties_panel(app: &mut Is2dApp, ctx: &egui::Context)` — 幅 `280px` の右端パネルを描画。
  - `draw_right_panel_tab_bar(app: &mut Is2dApp, ui: &mut egui::Ui)` — 上部の4タブ（`プロパティ` / `カラー` / `ライブラリ` / `キャンバス`）切り替えバーを描画。
  - `draw_properties_tab(app: &mut Is2dApp, ui: &mut egui::Ui)` — 現在のツールに応じたプロパティセクションを描画。
  - `draw_select_tool_properties(app: &mut Is2dApp, ui: &mut egui::Ui)` — 選択ツール時のプロパティ（アンカー追加 `+` / 削除 `-`、アンカーを表示、`draw_symbol_and_group_section`、選択削除、フレーム全消去、および単一シンボル選択時の再生設定 `draw_selected_symbol_properties`）を描画。
  - `draw_symbol_and_group_section(app: &mut Is2dApp, ui: &mut egui::Ui)` — 選択ツール（`V`）と自由変形ツール（`Q`）で共有する「シンボル・グループ」操作セクション（`シンボルに変換 (F8)`、`グループ化`、`解除 (Cmd+B)`）を描画。
  - `draw_transform_tool_properties(app: &mut Is2dApp, ui: &mut egui::Ui)` — 自由変形ツール時のプロパティ（左右反転・上下反転・-90°回転・+90°回転・ピボットリセット・`draw_symbol_and_group_section`・単一シンボル再生設定）を描画。
  - `draw_radius_slider_section(ui: &mut egui::Ui, label: &str, radius: &mut f32, min_r: f32, max_r: f32, step: f64)` — ブラシ（`0.5..=70.0`, `step 0.5`）・消しゴム（`1.0..=80.0`, `step 1.0`）で共有する太さスライダーを描画。
  - `draw_color_palette_section(app: &mut Is2dApp, ui: &mut egui::Ui)` — カラータブ・バケツ・スポイトで共有する塗り色ピッカー（16進数RGB表示付き）と10色プリセットパレットスウォッチを描画。
  - `draw_canvas_tab(app: &mut Is2dApp, ui: &mut egui::Ui)` — `キャンバス` タブ（キャンバス情報：ステージ幅×高さ `DragValue`、キャンバス操作：`枠外を隠す` ボタン・`ウィンドウに合わせる` ボタン・`100%表示` ボタン）を描画。
  - `draw_selected_frame_properties(app: &mut Is2dApp, ui: &mut egui::Ui)` — タイムライン上でトゥイーンキーフレームを選択した際に右プロパティパネル下部へ表示される「クラシックトゥイーン」（回転の向き：`自動` / `時計回り (CW)` / `反時計回り (CCW)`）を描画。
  - `draw_selected_symbol_properties(app: &mut Is2dApp, ui: &mut egui::Ui)` — キャンバス上で単一シンボル選択時に右プロパティパネル下部へ表示される「シンボル再生オプション」（`ループ再生` / `1回再生` / `任意のフレーム` 切り替え、フレーム番号 `DragValue`、シンボル編集ボタン）を描画。
  - `draw_library_tab(app: &mut Is2dApp, ui: &mut egui::Ui)` / `draw_selected_library_symbol_controls(app: &mut Is2dApp, ui: &mut egui::Ui, sel_id: u64)` / `draw_library_symbol_list(app: &mut Is2dApp, ui: &mut egui::Ui)` — 右パネルの `ライブラリ` タブ（選択図形のシンボル登録ボタン、選択中シンボルのミニプレビュー・名前変更・配置/編集/削除アクション、登録済みシンボル一覧リスト）を責務別に描画。
  - `draw_symbol_mini_preview(app: &Is2dApp, ui: &mut egui::Ui, sym: &SymbolDef)` — ライブラリパネル上部にシンボルの縮小ベクタープレビューを描画。
- **共通UIヘルパー関数（`panels.rs` 内）**：
  - `draw_panel_section_label(ui: &mut egui::Ui, title: &str, bottom_space: f32)` — 右パネル内の各セクション見出しラベルと下部余白を描画する共通ヘルパー。

---

## 3. [`canvas_tool.rs`](./canvas_tool.rs) — ツール定義・ビューポート・作業キャンバス状態・タイムライン同期・キャンバス合成描画

### 列挙型・状態型
- **`enum Tool`** — メインツール6種（`Select`, `Transform`, `Brush`, `Eraser`, `Bucket`, `Eyedropper`）。
  - `Tool::ALL: [Tool; 6]` — 全ツールの定数配列。
  - `icon(self) -> &'static str` — Material Icon 文字を返す。
  - `label(self) -> &'static str` — 日本語ツール名を返す。
  - `tooltip(self) -> &'static str` — ショートカット付きのツールチップ文字列を返す。
  - `shortcut_key(self) -> egui::Key` — 対応するショートカットキー（`V`, `Q`, `B`, `E`, `K`, `I`）。
  - `from_shortcut_input(i: &egui::InputState) -> Option<Self>` — 押されたキーから対応する `Tool` を返す。
- **`struct MoveSelectionDrag`** — 選択ツール（`V`）と自由変形ツール（`Q`）で共通化された選択移動ドラッグ状態（`start_pos`, `last_pos`, `alt_duplicated`, `undo_snapshot`）。
- **`struct MarqueeDrag`** — 選択ツール（`V`）と自由変形ツール（`Q`）で共通化された矩形マーキー選択ドラッグ状態（`start_pos`, `current_pos`）。
- **`enum ActiveCanvasDrag`** — キャンバス上の全ドラッグ操作を統合した排他的状態マシン（`None`, `PaintStroke { stroke, committed_mesh, tail_outline, tail_mesh, tail_start_idx }`, `Select(SelectDragState)`, `Transform(TransformDragState)`）。
- **`struct ViewportState`** — キャンバスのカメラ（`zoom`, `pan`, `pan_last_screen_pos`）およびステージ枠設定（`stage_size`, `clip_outside_stage`, `last_canvas_rect`）を保持する構造体。座標変換・ズーム・パン・ステージ枠描画・ハードウェアシザークリップの固有メソッド（`world_to_screen`, `screen_to_world`, `zoom_around_screen_pos`, `reset_view`, `fit_stage_to_window`, `stage_center_world`, `stage_world_rect`, `world_rect_to_screen`, `stage_screen_rect`, `visible_world_rect`, `draw_stage_background`, `artwork_painter`, `draw_stage_foreground_overlay`, `handle_navigation`）を自己完結で実装（`clip_outside_stage` 有効時は `Painter::with_clip_rect` によりステージ矩形へ直接クリッピングし、選択枠や変形ハンドルは非クリップの `painter` 上に維持）。
- **`struct CanvasState`** — 作業キャンバスの展開図形（`shapes`：空白フレームシンボルインスタンスのゼロ面積マーカーを含む統合表現）・非公開カプセル化された不変条件フィールド（`loaded_signatures`, `unsliced_canvas`）・排他ドラッグ状態（`drag`）・選択状態（`selected_shapes`, `selection_modified`, `hover_target`）・グループ編集パス（`editing_group_path`）・クリップボード（`clipboard`, `paste_count`）・自由変形状態（`transform_pivot`, `transform_hover`）を保持する構造体。全モジュールに分散していた `impl CanvasState` を本ファイル1箇所に集約し、1行委譲ラッパーを排してキャンバス固有の選択・`shape_id` ベース選択追従・ピボット取得・分割借用対応アフィン変換・署名検証・シンボルインスタンス走査・ドラッグロールバック・状態遷移不変条件メソッド（`loaded_signatures`, `has_unsliced_snapshot`, `take_unsliced_snapshot`, `restore_unsliced_snapshot`, `clear_unsliced_snapshot`, `selected_shape_mask`, `selected_shape_ids`, `restore_selection_by_shape_ids`, `take_or_clone_unsliced_snapshot`, `snapshot_for_drag`, `restore_drag_snapshot`, `scope_insert_index`, `get_contour_segment`, `topmost_shape_at`, `expand_to_group_unit`, `selection_has_group`, `selected_shapes_snapshot`, `selection_bounding_box`, `translate_selected`, `transform_selected`, `transform_selected_from_origins`, `sanitize_editing_scope`, `record_loaded_signatures`, `reset_selection_state`, `replace_loaded_shapes`, `select_shapes_in_layers`, `is_all_shapes_unchanged`, `extract_selected_relative_to_scope`, `select_symbol_instance_by_gid`, `selected_single_symbol_binding`, `update_symbol_instance_binding`, `rollback_shape_drag`, `sort_shapes_by_layer_order`, `active_transform_pivot`, `has_custom_transform_pivot`）を自己完結で実装。

### モジュール関数（`canvas_tool.rs` 内）
- **レイヤースコープ同期・タイムライン同期＆トランザクション**：
  - `active_scope_layer_id(app: &Is2dApp) -> u64` — 現在の編集スコープ（グループ編集中ならそのグループが属するレイヤー、通常時はタイムラインのアクティブレイヤー）のレイヤーIDを返す。
  - `topmost_layer_id_for_indices(app: &Is2dApp, indices: &[usize]) -> u64` — 指定された図形インデックス群の中で最前面にある図形の `layer_id` を返す。
  - `focus_active_layer_to_shape(app: &mut Is2dApp, shape_idx: usize)` — 指定図形が属するレイヤーをタイムラインのアクティブレイヤーに設定。
  - `sync_timeline_layer_from_canvas_selection(app: &mut Is2dApp)` — キャンバス上の選択図形に合わせてタイムラインのアクティブレイヤーとセル選択を自動同期。
  - `stop_playback(app: &mut Is2dApp)` — 再生中であれば停止して現在フレームの図形を作業キャンバスへ展開。
  - `flush_canvas_before_timeline_op(app: &mut Is2dApp)` — タイムライン操作前に進行中ドラッグのロールバックと選択解除を実行し、作業キャンバスをタイムラインへ同期。
  - `mutate_timeline_with_undo(app: &mut Is2dApp, mutate: impl FnOnce(&mut Timeline))` — 作業キャンバスの確定同期・Undo履歴記録・タイムライン変更・再読み込みを一括実行する共通トランザクションヘルパー。
  - `sync_shapes_to_timeline(app: &mut Is2dApp)` — 現在の作業キャンバスをタイムラインへ書き戻し、シンボル編集中なら `symbols.sync_active_edit(&app.timeline)` で親シーンのライブラリ定義へも即座に反映。
  - `load_shapes_from_timeline(app: &mut Is2dApp)` — 進行中ドラッグをリセットした上で、現在フレームの全可視・非ロックレイヤーの図形（トゥイーン補間・シンボル評価済み・空白フレームシンボルマーカー含む）を `app.canvas.shapes` に展開。
  - `select_shapes_for_timeline_selection(app: &mut Is2dApp)` — タイムライン上で選択されたセル範囲（レイヤー群）に属するキャンバス上の図形をすべて選択状態にする。
- **ツールモード切り替え＆進行中ドラッグのロールバック**：
  - `rollback_shape_drag(app: &mut Is2dApp)` / `cancel_active_drag(app: &mut Is2dApp)` — `app.canvas.rollback_shape_drag()` を呼び出し、進行中の図形変形・移動ドラッグを安全にロールバック・リセットする。
  - `set_tool(app: &mut Is2dApp, tool: Tool)` — `cancel_active_drag(app)` で進行中のドラッグを安全に中断・ロールバックした上で、選択状態を維持したままメインツールを切り替え。
  - `toggle_select_sub_mode(app: &mut Is2dApp, target_mode: SelectSubMode)` — 選択ツールへ切り替えつつ、アンカー追加（`+`）・削除（`-`）サブモードをトグル切り替え。
- **カスタムカーソル振り分け＆キャンバス合成描画**：
  - `draw_tool_cursor_and_feedback(app: &Is2dApp, ctx: &egui::Context, painter: &egui::Painter, pos: Pos2)` — 現在のツールに応じたカーソルオーバーレイを各ツールモジュール（`tool_paint`, `tool_select`, `tool_transform`）へ振り分け。
  - `draw_canvas_contents(app: &mut Is2dApp, painter: &egui::Painter)` / `draw_editable_layer_shapes(app: &mut Is2dApp, painter: &egui::Painter, range: Range<usize>, view_world_rect: Rect, (is_sel, has_sel): (&[bool], bool), selected_group_boxes: &mut Vec<(u64, Rect, Option<Point>)>)` / `draw_selected_group_and_symbol_boxes(app: &Is2dApp, painter: &egui::Painter, selected_group_boxes: &[(u64, Rect, Option<Point>)])` — シンボル編集背景は `backdrop_cel.cached_screen_mesh`（またはシンボル展開）、再生中・ロック中レイヤーは `resolved_cel_screen_mesh`、編集可能レイヤーは図形メッシュ・選択網点・ワイヤーフレーム・グループ緑枠／シンボル水色枠＋基準十字マーク・自由変形枠を責務別に重ねて描画。
  - `draw_onion_skins(app: &Is2dApp, painter: &egui::Painter)` / `draw_onion_skin_frame(app: &Is2dApp, painter: &egui::Painter, frame: usize, tint_rgb: [u8; 3], alpha: u8)` — 前後フレームのオニオンスキン（過去フレーム＝青系、未来フレーム＝緑系、距離に応じた段階的アルファ減衰）を描画。
  - `draw_active_marquee_box(app: &Is2dApp, painter: &egui::Painter)` — 選択ツール／自由変形ツールでドラッグ中の矩形選択ボックスを描画。
  - `draw_registration_cross(painter: &egui::Painter, center: Pos2, arm: f32, with_white_halo: bool)` — シンボル基準点（`+` 十字マーク）を描画する共通フリー関数。

---

## 4. [`tool_paint.rs`](./tool_paint.rs) — ブラシ・消しゴム・バケツ・スポイト操作ロジック＆プレビュー描画

### 定数＆モジュール関数（`tool_paint.rs` 内）
- **`STROKE_TAIL_CHUNK_LEN: usize`** (`24`) — ブラシ・消しゴムのドラッグ中に末尾チャンクのみを増分テッセレーションして `committed_mesh` へ確定追記する閾値点数（長尺ストローク時の $O(N^2)$ 頂点再生成を $O(1)$/イベントに抑える）。
- **`handle_brush_or_eraser_input(app: &mut Is2dApp, response: &egui::Response, mouse_pt: Option<Point>)`** — ブラシ（`B`）および消しゴム（`E`）のドラッグ開始・ドラッグ中（点列追加と `STROKE_TAIL_CHUNK_LEN` 単位の増分チャンクプレビューメッシュ生成）・ドラッグ終了（ストローク全体から最終輪郭を生成して `commit_raw_outline` 呼び出し）を処理。
- **`commit_raw_outline(app: &mut Is2dApp, raw_outline: &[Point])`** — 描かれた生アウトライン点列を `build_smoothed_stroke_shapes_at_zoom` で滑らかな2次ベジェ図形に変換し、現在の編集スコープとアクティブレイヤーに対してブラシなら `merge_shapes_into_canvas`（同色結合・異色切り抜き）、消しゴムなら `erase_from_canvas`（減算）を実行してタイムラインへ同期。
- **`bucket_fill_at(app: &mut Is2dApp, pt: Point) -> bool`** — バケツツール（`K`）の塗りつぶし処理：
  1. 選択中の図形がある場合：グループ・シンボルが含まれていれば無効化し、選択中の生シェイプ内部をクリックしたときだけ選択領域の色を変更。
  2. 未選択時：編集スコープ内の既存生シェイプ上ならその図形をリカラーして同色マージ。空白部分なら全可視レイヤーの輪郭を境界壁として `find_enclosed_region_at` で閉領域（穴）を検出し、アクティブレイヤーへ新しい塗り図形を流し込む。
- **`topmost_visible_color_at(app: &Is2dApp, pt: Point) -> Option<Color32>`** — レイヤーのロック状態やグループ階層を問わず、画面上で見えている最前面図形の色を返す（スポイト用）。
- **`pick_color_at(app: &mut Is2dApp, pt: Point) -> Option<Color32>`** — スポイトツール（`I`）でクリック／ドラッグ位置の最前面可視図形（シンボル・グループ・ロック中レイヤー含む）の塗りカラーを取得し、`app.ui.brush_color` に設定。
- **`draw_active_stroke_preview(app: &Is2dApp, painter: &egui::Painter)`** — ブラシ／消しゴムでドラッグ中の `committed_mesh` および末尾チャンク `tail_mesh` をスクリーン座標へ変換してリアルタイムプレビュー描画。
- **`draw_eyedropper_cursor_badge(painter: &egui::Painter, pos: Pos2, sample_color: Option<Color32>, current_color: Color32)`** — スポイトツールの十字照準と抽出色プレビュー円形バッジを描画。

---

## 5. [`tool_select.rs`](./tool_select.rs) — 選択ツール・ヒット判定・輪郭粘土変形・グループ階層・クリップボード

### 列挙型・状態型（`tool_select.rs` 内）
- **`enum SelectSubMode`** — 選択ツールのサブモード（`Reshape`, `AddAnchor`, `DeleteAnchor`）。
- **`enum HoverTarget`** — 選択ツールのホバー対象（`None`, `Vertex { shape_idx, contour_idx, seg_idx }`, `Edge { shape_idx, contour_idx, seg_idx, t, closest_pos }`, `Fill { shape_idx }`）。
  - `shape_idx(&self) -> Option<usize>` — ホバー中の図形インデックスを返す。
- **`enum SelectDragState`** — 選択ツールのアクティブドラッグ状態（`MoveShape(MoveSelectionDrag)`, `MarqueeSelect(MarqueeDrag)`, `PullEdge`, `MoveVertex`）。

### ヒット判定・頂点スナップ・輪郭変形フリー関数（`tool_select.rs` 内）
- `quad_control_bbox_contains(seg: QuadBez, pt: Point, tol: f64) -> bool` / `point_within_shape_contours(shape: &FillShape, pt: Point, hit_tol: f64) -> bool` — 2次ベジェ制御点AABBおよび輪郭線周辺での高速ヒット判定補助関数。
- `collect_marquee_hit_groups(canvas: &CanvasState, marquee_rect: Rect, marquee_shape: &FillShape) -> Vec<u64>` — マーキー矩形と交差するグループ／シンボルインスタンスのID一覧を収集。
- `compute_hover_target(canvas: &CanvasState, pt: Point, zoom: f32) -> HoverTarget` — 選択ツールでマウス直下にヒットしている要素（頂点・エッジ・塗り）を優先順位に従って判定。
- `find_snap_vertex(canvas: &CanvasState, pt: Point, zoom: f32, exclude: Option<(usize, usize, usize)>) -> Option<Point>` — 頂点ドラッグ時に近傍の他頂点へ吸着するスナップ先座標を探索。
- `punch_through_opposite_edge(shape: &FillShape, contour_idx: usize, ignore_segs: &[usize], (anchor_a, anchor_b): (Point, Point), (start_pt, drag_pt): (Point, Point)) -> Point` — エッジを対向側の輪郭線ギリギリまたは向こう側へドラッグした際、確実に2図形へ分割されるよう微小に切り込みを貫通させる補正処理。
- `set_pulled_edge(shape: &mut FillShape, (contour_idx, seg_idx): (usize, usize), grab_t: f64, orig_seg: QuadBez, target_pt: Point)` / `apply_pull_edge_to_shape(shape: &mut FillShape, (contour_idx, seg_idx): (usize, usize), grab_t: f64, orig_seg: QuadBez, drag_pt: Point)` — 2次ベジェ曲線の式 `B(t) = (1-t)^2 P0 + 2t(1-t) P1 + t^2 P2` から制御点 `P1` を逆算し、掴んだ点 `grab_t` が目標座標を通るように変形（リアルタイムプレビュー用および貫通補正込み確定用）。
- `set_moved_vertex(shape: &mut FillShape, (contour_idx, seg_idx): (usize, usize), orig_vertex: Point, orig_ctrls: (Point, Point), target_pt: Point)` / `apply_move_vertex_to_shape(shape: &mut FillShape, (contour_idx, seg_idx): (usize, usize), orig_vertex: Point, orig_ctrls: (Point, Point), drag_pt: Point)` — 掴んだ頂点（アンカーポイント）を移動し、前後のセグメントの制御点を自然な比率で追従させる（リアルタイムプレビュー用および貫通補正込み確定用）。

### モジュール関数（`tool_select.rs` 内）
- **選択・移動・マーキースライス・削除**：
  - `clear_selection(app: &mut Is2dApp)` — 選択状態を解除。マーキースライス後に未編集なら元の図形を無劣化復元し、移動・色変更された生シェイプがあれば同じレイヤー・同じスコープの生シェイプに対して Merge Drawing（結合・切り抜き）を確定。
  - `toggle_shape_selection(app: &mut Is2dApp, shape_idx: usize)` — `Shift` クリックで図形（またはグループ塊）の選択状態をトグル切り替え。
  - `select_single_shape_at(app: &mut Is2dApp, pt: Point) -> bool` — 単一クリックで指定座標の図形（またはグループ塊）を選択し、タイムラインのレイヤー選択も同期。
  - `nudge_selected_shapes(app: &mut Is2dApp, delta: Vec2) -> bool` — 選択中の全図形を十字キー等で `delta` だけ移動し、Undo 履歴記録・最前面整列・タイムライン同期を実行。
  - `apply_marquee_selection(app: &mut Is2dApp, p0: Point, p1: Point, additive: bool)` — 矩形ドラッグによる範囲選択を実行。グループやシンボルは `collect_marquee_hit_groups` で交差判定して塊全体を選択し、生シェイプは矩形境界で `Intersection` / `Difference` による部分スライス（ナイフ切断選択）を行う（`Shift` 押下時は既存の選択スライスを維持したまま追加スライス）。
  - `delete_selected_shapes(app: &mut Is2dApp) -> bool` — 選択中の図形・スライス領域を一括削除し、Undo 記録とタイムライン同期を実行。
  - `clear_active_frame_shapes(app: &mut Is2dApp) -> bool` — 現在の編集スコープ（グループ編集中ならそのグループ内、通常時はアクティブレイヤー）の現在フレーム図形をすべて消去。
  - `reorder_selected_to_scope_front(app: &mut Is2dApp) -> bool` / `commit_moved_selection(app: &mut Is2dApp, undo_snapshot: Vec<FillShape>)` — 選択図形を各レイヤー・スコープ内の最前面へ並べ替え、Undo記録およびタイムライン同期を実行。
  - `finish_move_selection_drag(app: &mut Is2dApp, total_delta: Vec2, undo_snapshot: Vec<FillShape>)` — 選択移動ドラッグの終了処理と Undo 記録。
  - `finish_deform_shape_drag(app: &mut Is2dApp, shape_idx: usize, drag_dist: f64, undo_snapshot: Vec<FillShape>, apply_deform: impl FnOnce(&mut FillShape))` — エッジ引き伸ばし／頂点移動ドラッグの終了時に `resolve_deformed_shape` で自己交差解消・横断分割を実行し、同一レイヤー・同一スコープ内へマージ。
- **グループ化・グループ解除・インプレイス階層編集**：
  - `group_selected_shapes(app: &mut Is2dApp) -> Option<u64>` — 選択中の図形群に新しいグループIDを付与して1つのグループにまとめる（`Cmd/Ctrl + G`、ネストグループ対応）。
  - `ungroup_selected_shapes(app: &mut Is2dApp) -> bool` — 選択中のグループまたはシンボルの現在の階層レベルを1段階解除して展開する（`Cmd/Ctrl + B` / `Cmd/Ctrl + Shift + G`）。
  - `enter_group_at(app: &mut Is2dApp, pt: Point) -> bool` — ダブルクリック位置にあるシンボルインスタンス（`enter_symbol_timeline` へ委譲）またはグループ（`editing_group_path` にプッシュ）の内部へインプレイス編集で潜る。
  - `exit_current_group(app: &mut Is2dApp) -> bool` — 現在のグループ階層から1つ上の親階層へ出る（ルート階層でシンボル編集中なら `exit_symbol_timeline` で親シーンへ戻る）。
- **コピー・カット・ペースト・Alt＋ドラッグ複製（クリップボード＆複製操作）**：
  - `instantiate_relative_shapes_in_scope(app: &mut Is2dApp, rel_shapes: &[FillShape], offset: Vec2, preserve_original_layer: bool) -> (Vec<FillShape>, bool)` — 相対パスを持つシェイプ群に対し、新しいグループID・シンボルインスタンスIDを採番して現在の編集スコープ用に変換する共通ヘルパー。
  - `copy_selected_shapes(app: &mut Is2dApp) -> bool` — 選択中の図形群を内部クリップボードに保存（`Cmd/Ctrl + C`）。
  - `cut_selected_shapes(app: &mut Is2dApp) -> bool` — 選択中の図形群をコピーして削除（`Cmd/Ctrl + X`）。
  - `paste_clipboard(app: &mut Is2dApp, in_place: bool) -> bool` — クリップボードの図形群を貼り付け（`in_place = false` なら `(+16, +16)` ずつオフセット、`in_place = true` なら元と同じ座標に配置）。グループやシンボルインスタンスには新しい一意なIDを再採番。
  - `duplicate_selected_shapes_for_alt_drag(app: &mut Is2dApp) -> Vec<FillShape>` — 選択中のオブジェクト（生シェイプ・グループ・シンボル）をその場に複製し、複製側を選択状態に切り替えた上でドラッグ移動用の `undo_snapshot` を返す（`Alt + ドラッグ` 複製用）。
  - `insert_and_select_shapes_in_scope(app: &mut Is2dApp, new_shapes: Vec<FillShape>, mark_modified: bool)` — 新規生成・ペースト・複製された図形群を現在の編集スコープへ挿入し、それらを選択状態にしてレイヤー順へ整列する共通ヘルパー。
  - `begin_move_selection_drag(app: &mut Is2dApp, pt: Point, alt_held: bool) -> MoveSelectionDrag` — 選択オブジェクトの移動ドラッグ（`Alt` 複製移動を含む）の初期状態を構築する共通ヘルパー。
- **選択オーバーレイ・カーソルバッジ描画＆選択ツール入力処理**：
  - `draw_flash_selection_overlay(app: &mut Is2dApp, painter: &egui::Painter, shape_idx: usize)` — 選択中の生シェイプに白い網点（Polka-Dot）パターンと青輪郭線を描画。
  - `draw_shape_contour_lines(app: &Is2dApp, painter: &egui::Painter, shape: &FillShape, stroke: Stroke, show_anchors: bool)` — 図形の外周・穴のベジェ曲線をスクリーン座標でストローク描画。
  - `draw_select_tool_cursor(app: &Is2dApp, ctx: &egui::Context, painter: &egui::Painter, pos: Pos2)` / `draw_select_sub_mode_cursor` / `draw_reshape_hover_cursor` / `draw_highlighted_quad_segment` / `draw_flash_curve_cursor_badge` / `draw_flash_corner_cursor_badge` / `draw_anchor_edit_cursor_badge` — 選択ツールのカーソル・頂点スナップ二重丸・エッジハイライト・各種カーソルバッジを描画。
  - `handle_select_tool_input(app: &mut Is2dApp, ui: &egui::Ui, response: &egui::Response, mouse_pt: Option<Point>)` — 選択ツール（`V`）のメイン入力ハンドラ（ダブルクリック階層移動、サブモード振り分け、ドラッグ開始・更新・終了、クリック選択）。
  - `handle_anchor_edit_input(app: &mut Is2dApp, ui: &egui::Ui, response: &egui::Response, mouse_pt: Option<Point>, is_add: bool)` — アンカー追加（`+`：De Casteljau 分割で曲線形状を変えずにアンカー挿入）およびアンカー削除（`-`：アンカーを消去して前後を直線化）のクリック処理を実装。
  - `start_reshape_drag(app: &mut Is2dApp, pt: Point, shift_held: bool, alt_held: bool)` / `update_reshape_drag(app: &mut Is2dApp, pt: Point)` / `finish_reshape_drag(app: &mut Is2dApp, mouse_pt: Option<Point>)` / `handle_reshape_click(app: &mut Is2dApp, pt: Point, shift_held: bool)` — 選択ツールのドラッグ開始・ドラッグ中・ドロップ確定・単発クリックの各フェーズ処理。

---

## 6. [`tool_transform.rs`](./tool_transform.rs) — 自由変形ツール（`Q`）＆シンボル/トゥイーン操作コントローラ

### 列挙型・状態型（`tool_transform.rs` 内）
- **`enum ScaleHandle`** — 自由変形バウンディングボックスの8方向スケールハンドル（`TopLeft`, `Top`, `TopRight`, `Right`, `BottomRight`, `Bottom`, `BottomLeft`, `Left`）。
  - `ScaleHandle::ALL: [ScaleHandle; 8]`
  - `position(self, bbox: Rect) -> Point` / `opposite(self) -> Self` / `opposite_position(self, bbox: Rect) -> Point` / `affects_x(self) -> bool` / `affects_y(self) -> bool` / `is_corner(self) -> bool`
- **`enum TransformHoverTarget`** — 自由変形ツールのホバー対象（`None`, `Pivot`, `Scale(ScaleHandle)`, `Rotate`, `Move`）。
- **`enum TransformDragState`** — 自由変形ツールのアクティブドラッグ状態（`MovePivot`, `Move(MoveSelectionDrag)`, `Scale`, `Rotate`, `MarqueeSelect(MarqueeDrag)`）。

### モジュール関数（`tool_transform.rs` 内）
- **自由変形ギズモ・ホバー・カーソル・アフィン変形**：
  - `compute_transform_hover(canvas: &CanvasState, mouse_pt: Point, zoom: f32) -> TransformHoverTarget` — 自由変形ハンドル・ピボット・回転帯・移動領域のホバー判定。
  - `draw_free_transform_box(canvas: &CanvasState, painter: &egui::Painter, viewport: &ViewportState)` / `draw_transform_tool_cursor(app: &Is2dApp, ctx: &egui::Context, painter: &egui::Painter, pos: Pos2)` / `scale_handle_cursor_icon(handle: ScaleHandle) -> egui::CursorIcon` / `draw_transform_rotate_cursor_badge(painter: &egui::Painter, pos: Pos2)` — 自由変形バウンディングボックス・8ハンドル・ピボット白丸・カーソルバッジを描画。
  - `set_selected_symbol_world_pivot(app: &mut Is2dApp, world_pivot: Option<Point>, commit_to_timeline: bool) -> bool` / `flush_transform_pivot_to_symbol(app: &mut Is2dApp)` — 単一シンボルインスタンス選択時にワールド座標ピボットをローカル座標に変換して `SymbolInstanceBinding::pivot` に保存・同期する。
  - `reset_selected_transform_pivot(app: &mut Is2dApp) -> bool` — 選択オブジェクトの変形中心（ピボット）を初期位置（選択中央またはシンボル基準点 `(0, 0)`）へリセットする。
  - `apply_affine_to_selected(app: &mut Is2dApp, affine: Affine) -> bool` / `flip_selected_shapes(app: &mut Is2dApp, flip_x: bool, flip_y: bool) -> bool` / `rotate_selected_shapes(app: &mut Is2dApp, angle_rad: f64) -> bool` / `scale_selected_shapes(app: &mut Is2dApp, sx: f64, sy: f64) -> bool` — ピボット点を基準に選択図形を反転・回転・拡大縮小。
  - `handle_transform_tool_input` / `start_transform_drag` / `update_transform_drag` / `finish_transform_drag` / `handle_transform_click` — 自由変形ツール（`Q`）のマウス入力・ドラッグ更新（`&mut app.canvas.shapes` と `&app.canvas.drag` のフィールド分割借用により `orig_shapes` の毎フレームクローンをゼロ化）・確定処理。
- **シンボル変換・ライブラリ管理・クラシックトゥイーン・インプレイス編集コントローラ**：
  - `convert_selection_to_symbol(app: &mut Is2dApp) -> Option<u64>` — 選択中の図形群の中心を `(0, 0)` 基準に正規化して新規シンボルとしてライブラリへ登録し、ステージ上の選択図形をそのシンボルインスタンスへ置き換える（`F8`）。
  - `place_symbol_instance_from_library(app: &mut Is2dApp, symbol_id: u64) -> bool` — ライブラリ内の指定シンボル（Frame 0 空白シンボルや全フレーム白紙の空シンボル含む）をステージ中央へ新しいインスタンスとして配置。
  - `rename_symbol_in_library(app: &mut Is2dApp, symbol_id: u64, new_name: impl Into<String>) -> bool` — ライブラリ内の指定シンボル名を変更し、Undo履歴を記録。
  - `delete_symbol_from_library(app: &mut Is2dApp, symbol_id: u64) -> bool` — 指定シンボルをライブラリから削除し、タイムライン上の全インスタンスを通常のグループ（ベクター図形）へ自動分解。
  - `timeline_toggle_tween(app: &mut Is2dApp) -> bool` / `set_active_keyframe_tween_rotate(app: &mut Is2dApp, rotate_mode: TweenRotate) -> bool` — クラシックトゥイーン（`T` キー）の ON/OFF 切り替えと回転方向設定。
  - `selected_single_symbol_info(app: &Is2dApp) -> Option<(SymbolInstanceBinding, String, usize)>` / `set_selected_symbol_playback(app: &mut Is2dApp, loop_mode: SymbolLoopMode, first_frame: usize) -> bool` — 選択中の単一シンボルインスタンスの情報取得と再生モード設定。
  - `enter_symbol_timeline(app: &mut Is2dApp, binding: SymbolInstanceBinding) -> bool` / `enter_symbol_from_library(app: &mut Is2dApp, symbol_id: u64) -> bool` / `exit_symbol_timeline(app: &mut Is2dApp) -> bool` — シンボル内タイムラインへの出入り（インプレイス編集）と親シーンへの保存復帰。

---

## 7. [`timeline.rs`](./timeline.rs) — タイムラインの純粋データモデル・3階層メッシュキャッシュ

### 構造体・列挙型（`timeline.rs` 内）
- **`MAX_RESOLVED_CEL_CACHE_ENTRIES: usize`** (`64`) / **`type LruEntry<K, V> = (K, V, u64)`** / **`lru_get`** / **`lru_insert`** — シンボル・トゥイーンを含むセルの解決済みメッシュキャッシュ（最大64フレーム分）を、キャッシュヒット時の配列要素ムーブが一切発生しない `last_used_tick` タイムスタンプ方式の LRU で管理する定数とヘルパー。
- **`struct CelMeshCache`** — セル単位のキャッシュ構造体（`shape_revisions`, `world_batch_mesh: Arc<egui::Mesh>`, `screen_cache`, `onion_cache`, およびシンボル・トゥイーンセル用の64エントリLRUキャッシュ `resolved_sig: Option<ResolvedCelSig>`, `resolved_world_batches`, `resolved_screen_caches`, `resolved_onion_caches`。Clone 時は `Arc::clone` による `O(1)` 共有）。
- **`struct Cel`** — 1つのキーフレームが保持する描画セル（`shapes: Arc<Vec<FillShape>>`, `instances: Arc<Vec<SymbolInstance>>`, `revision: u64`, `mesh_cache: SyncCache<CelMeshCache>`）。`shapes` と `instances` を `Arc` による Copy-on-Write で保持し、`Timeline::strip_transient_caches_with_prev` によって Undo 履歴スナップショット間で未変更キーフレームの `Arc` を $O(1)$ で構造共有する。さらに `shapes` には非シンボル図形（生シェイプおよび通常グループ図形：`s.symbol_instance.is_none()`）のみを保持し、シンボルインスタンスは挿入順序インデックス `shape_order_index` 付きの正規化エンティティ一覧 `instances` として完全分離保持する。全モジュールに分散していた `impl Cel` を本ファイル1箇所に集約：
  - `new(shapes: Vec<FillShape>) -> Self` / `new_with_instances(shapes: Vec<FillShape>, instances: Vec<SymbolInstance>) -> Self` / `is_empty(&self) -> bool`
  - `shapes_mut(&mut self) -> &mut Vec<FillShape>` / `instances_mut(&mut self) -> &mut Vec<SymbolInstance>` — `Arc::make_mut` で CoW 参照を取得し、`self.revision` を新しい一意な世代番号に更新。
  - `split_shapes_and_instances(shapes: Vec<FillShape>) -> (Vec<FillShape>, Vec<SymbolInstance>)` — 作業キャンバスの展開図形配列（空白フレームシンボルマーカー `is_empty_symbol_marker()` を含む）から非シンボル図形配列と正規化 `SymbolInstance` 一覧を1パスで分離。
  - `strip_transient_caches(&mut self)` — セル内の全図形の一時メッシュキャッシュと `CelMeshCache` を破棄してメモリを解放。
  - `ensure_world_batch_mesh(&self, cache: &mut CelMeshCache)` / `ensure_resolved_world_batch(&self, ...)` — セル内の図形リビジョンまたは衝突ゼロのシンボル・トゥイーン署名クエリ `ResolvedSigQuery` をヒープ確保ゼロで検証し、変化があれば新しい `ResolvedCelSig` とワールド座標結合メッシュを再構築（変化がなければ $O(1)$ でスキップ）。
  - `cached_screen_mesh(&self, zoom: f32, pan: Vec2, dim_backdrop: bool) -> Arc<egui::Mesh>` / `cached_resolved_screen_mesh(&self, (sig_query, eval_frame_key): (ResolvedSigQuery<'_>, usize), zoom: f32, pan: Vec2, dim_backdrop: bool, build_fn: impl FnOnce() -> Option<Mesh>) -> Option<Arc<egui::Mesh>>` / `cached_backdrop_screen_mesh(&self, frame: usize, zoom: f32, pan: Vec2, library: &[SymbolDef]) -> Option<Arc<egui::Mesh>>` — 純粋図形セル、シンボル・トゥイーンセル、およびシンボルインプレイス編集時の背景セル（`backdrop_cel`）のスクリーン座標メッシュをすべて `Arc<egui::Mesh>` 共有キャッシュからヒープ確保ゼロの $O(1)$ で返す。
  - `cached_onion_mesh(&self, zoom: f32, pan: Vec2, tint_rgb: [u8; 3], alpha: u8) -> Arc<egui::Mesh>` / `cached_resolved_onion_mesh(&self, (sig_query, eval_frame_key): (ResolvedSigQuery<'_>, usize), zoom: f32, pan: Vec2, (tint_rgb, alpha): ([u8; 3], u8), build_fn: impl FnOnce() -> Option<Mesh>) -> Option<Arc<egui::Mesh>>` — 純粋図形セルおよびシンボル・トゥイーンセル双方のオニオンスキン用メッシュを `Arc<egui::Mesh>` 共有キャッシュからヒープ確保ゼロで返す。
  - `detach_symbol_id(&mut self, symbol_id: u64, kf_start_frame: usize, library: &[SymbolDef])` — 指定された `symbol_id` を参照するシンボルインスタンスバインディングを解除（通常のグループ図形へ展開して `shape_order_index` の位置へ挿入）。
  - `single_symbol_instance(&self) -> Option<SymbolInstanceBinding>` — `self.shapes.is_empty() && self.instances.len() == 1` の1行判定により、セル内の図形が生の絵や別グループを含まず**ただ1つのシンボルインスタンスのみ**で構成されている場合にその `SymbolInstanceBinding` を返す（クラシックトゥイーンの成立条件判定）。
  - `update_symbol_instances_with(&mut self, mut map_binding: impl FnMut(SymbolInstanceBinding) -> Option<SymbolInstanceBinding>) -> bool` / `has_dynamic_symbol(&self, library: &[SymbolDef]) -> bool`
- **`struct Keyframe`** — キーフレーム構造体（`start_frame: usize`, `cel: Cel`, `tween: bool`, `tween_rotate: TweenRotate`）。
  - `new_blank(start_frame: usize) -> Self` — 指定フレーム開始の空キーフレームを生成。
  - `clear_cel(&mut self)` — キーフレームを空のセル＆`tween = false` に初期化。
- **`struct TimelineCellSelection`** — タイムライン上の矩形セル選択範囲（`min_layer`, `max_layer`, `min_frame`, `max_frame`）。
  - `single(layer: usize, frame: usize) -> Self` / `from_corners(l0: usize, f0: usize, l1: usize, f1: usize) -> Self`
  - `contains(&self, layer: usize, frame: usize) -> bool` / `contains_layer(&self, layer: usize) -> bool`
- **`struct Layer`** — タイムラインの1レイヤー（`id: u64`, `name: String`, `visible: bool`, `locked: bool`, `keyframes: Vec<Keyframe>`, `frame_count: usize`）。全モジュールに分散していた `impl Layer` を本ファイル1箇所に集約：
  - `new_with_id_and_frames(id: u64, name: impl Into<String>, frame_count: usize) -> Self` / `new_with_id(id: u64, name: impl Into<String>) -> Self`
  - `keyframe_index_at(&self, frame: usize) -> Option<usize>` — 指定フレームが属するキーフレームのインデックスを返す。
  - `span_end_for_keyframe(&self, kf_idx: usize) -> usize` — 指定キーフレームの保持スパン終了フレーム（排他的上限）を返す。
  - `cel_at(&self, frame: usize) -> Option<&Cel>` / `cel_at_mut(&mut self, frame: usize) -> Option<&mut Cel>` — 指定フレームで有効な `Cel` への参照を返す。
  - `insert_keyframe(&mut self, frame: usize, clone_prev: bool) -> usize` / `delete_keyframe_at(&mut self, frame: usize) -> bool` / `insert_frame_at(&mut self, frame: usize)` / `remove_frame_at(&mut self, frame: usize) -> bool`
  - `extract_keyframes_in_range(&self, min_f: usize, max_f: usize) -> Vec<(usize, Cel, bool, TweenRotate)>` / `remove_keyframes_in_range(&mut self, min_f: usize, max_f: usize)` / `place_keyframes_at(&mut self, base_frame: usize, extracted: Vec<(usize, Cel, bool, TweenRotate)>, min_required_end: usize)`
  - `detach_symbol_id(&mut self, symbol_id: u64, library: &[SymbolDef])` / `tween_state_at_keyframe(&self, kf_idx: usize) -> TweenState` / `interpolated_tween_binding_at(&self, frame: usize) -> Option<(SymbolInstanceBinding, usize)>` / `evaluate_shapes_at(&self, (frame, child_eval_frame): (usize, usize), out_layer_id: u64, ctx: SymbolEvalCtx<'_>) -> Vec<FillShape>`
- **`struct LoadedShapeSig`** — キャンバス展開時の図形署名（`#[derive(Clone, Debug, PartialEq)]`：`shape_id`, `layer_id`, `mesh_revision`, `color`, `group_path: Vec<u64>`, `symbol_instance`）。ハッシュ衝突リスクのない完全比較を行い、未編集のトゥイーン補間図形やシンボル展開図形で元のキーフレームを上書き破壊しないためのレイヤー別変更検知に使用。
  - `from_shape(s: &FillShape) -> Self` / `matches(&self, s: &FillShape) -> bool` / `matches_with_layer(&self, s: &FillShape) -> bool`
- **`struct Timeline`** — タイムライン全体のドキュメント状態（`layers`, `next_layer_number`, `next_layer_id`, `active_layer`, `current_frame`, `total_frames`, `fps`, `cell_selection`）。全モジュールに分散していた `impl Timeline` を本ファイル1箇所に集約：
  - `Default` 実装（24fps、1フレーム、`Layer 1` 1枚）
  - `new_for_symbol_edit(&self, sym_def: &SymbolDef, sym_layers: Vec<Layer>) -> Self` — シンボル内部のインプレイス編集用タイムラインを構築。
  - `sync_total_frames(&mut self)` / `resize_total_frames(&mut self, next_total: usize)` / `select_single_cell(&mut self, layer: usize, frame: usize)`
  - `active_layer_id(&self) -> u64` / `layer_index_by_id(&self, id: u64) -> Option<usize>` / `is_layer_editable_by_id(&self, id: u64) -> bool` / `has_any_editable_layer(&self) -> bool` / `is_active_layer_editable(&self) -> bool`
  - `target_layers_for_frame_op(&self) -> Vec<usize>` / `active_cel_shapes(&self) -> &[FillShape]`
  - `is_layer_unchanged_since_load(loaded_signatures: &[LoadedShapeSig], layer_id: u64, current_layer_shapes: &[FillShape]) -> bool` — ロード時の図形署名スライスと現在レイヤーの図形列を照合し、ヒープ確保ゼロでレイヤー別未変更判定を実行。
  - `normalize_shape_layer_ids(&self, shapes: &mut [FillShape])` — 存在しないレイヤーIDを持つ図形をアクティブレイヤーのIDへ正規化。
  - `strip_transient_caches(&mut self)` / `strip_transient_caches_with_prev(&mut self, prev: Option<&Timeline>)` — 全レイヤー・全キーフレームの一時メッシュキャッシュを破棄（`prev` 指定時は `cel.revision == p_kf.cel.revision` の未変更キーフレームについて既にキャッシュ除去済みの `Arc<Vec<FillShape>>` / `Arc<Vec<SymbolInstance>>` を $O(1)$ で引き継ぐ）。
  - `write_shapes_to_editable_cels(&mut self, shapes: &[FillShape], library: &[SymbolDef], loaded_signatures: &[LoadedShapeSig]) -> bool` — 作業キャンバス `shapes` を各レイヤーの `layer_id` ごとに振り分け、実際に編集されたレイヤーのみセルへ書き戻す。
  - `insert_keyframe_on_targets` / `can_delete_keyframe_on_targets` / `delete_keyframe_on_targets` / `extend_frame_on_targets` / `can_remove_frame_on_targets` / `remove_frame_on_targets` / `resolve_frame_move_bounds` / `move_or_copy_frames` / `add_layer_above_active` / `delete_active_layer` / `reorder_layer`
  - `can_toggle_tween_at_current_frame(&self) -> (bool, bool)` / `active_tween_keyframe_info(&self) -> Option<(usize, TweenRotate, bool)>` / `evaluate_layer_shapes_at(&self, layer_idx: usize, frame: usize, library: &[SymbolDef]) -> Vec<FillShape>` / `build_layer_resolved_world_batch_mesh(&self, layer_idx: usize, frame: usize, library: &[SymbolDef]) -> Option<egui::Mesh>` / `resolved_cel_screen_mesh(&self, ...)` / `resolved_cel_onion_mesh(&self, ...)`

---

## 8. [`timeline_ui.rs`](./timeline_ui.rs) — タイムライン（X-Sheet）UI表示状態・描画・コントローラ操作

### 定数・UI状態構造体・レイアウト構造体
- **レイアウト定数**：
  - `LAYER_COL_W: f32` (`190.0`) — 左側のレイヤー一覧カラム幅。
  - `RULER_H: f32` (`24.0`) — 上部のフレーム番号ルーラー高さ。
  - `ROW_H: f32` (`26.0`) — 各レイヤー行の高さ。
  - `CELL_W: f32` (`16.0`) — 1フレームセルの横幅。
  - `LAYER_VIS_HIT_RANGE: Range<f32>` (`6.0..30.0`) / `LAYER_LOCK_HIT_RANGE: Range<f32>` (`30.0..54.0`) / `LAYER_VIS_ICON_X: f32` (`18.0`) / `LAYER_LOCK_ICON_X: f32` (`42.0`) / `LAYER_NAME_X: f32` (`60.0`) — レイヤー行内のアイコン・ラベル描画位置およびヒット判定で共有するX座標定数。
- **`enum TimelineGridDrag`** — タイムライングリッド上のドラッグ状態（`None`, `ScrubRuler`, `SelectCells`, `MoveFrames`）。
- **`struct TimelineUiState`** — タイムラインのUI表示・再生状態（`is_playing`, `loop_playback`, `onion_skin_enabled`, `onion_prev_frames`, `onion_next_frames`, `last_frame_advance_time`, `scroll_x`, `scroll_y`, `last_ensured_frame`, `dragging_layer`, `grid_drag`, `editing_total_frames`, `editing_fps`）。ドキュメント本体の `Timeline` から表示設定・一時状態を完全分離することで、Undo/Redo スナップショットやシンボル編集セッションへの混入および復元時のアドホックな上書きコピーを排除。唯一の `impl TimelineUiState` を本ファイルに集約：
  - `cancel_drag(&mut self)` — タイムライン上のレイヤーD&Dおよびグリッドドラッグ状態をクリア。
  - `reset_for_timeline_switch(&mut self, current_frame: usize)` — シンボル内部タイムラインへの出入り時に再生・スクロール・ドラッグ状態をリセット。
  - `ensure_current_frame_visible(&mut self, current_frame: usize, grid_width: f32)` — 再生ヘッド（`current_frame`）が表示領域外に出た際、`scroll_x` を自動スクロールして追従させる。
- **`struct TimelineLayout`** — タイムラインパネル内の領域座標・可視フレーム範囲・ヘッダーミニボタン矩形（`panel_rect`, `grid_left`, `grid_top`, `grid_w`, `grid_h`, `max_frames_shown`, `first_vis_frame`, `last_vis_frame`, `add_hdr_rect`, `del_hdr_rect`）とスクロール座標変換を提供するヘルパー構造体。
  - `new(panel_rect: Rect, max_reach_frames: usize, scroll_x: f32) -> Self`
  - `frame_at_x(&self, x: f32, scroll_x: f32) -> usize` / `row_at_y(&self, y: f32, scroll_y: f32) -> isize` / `layer_at_y(&self, y: f32, scroll_y: f32, num_layers: usize) -> Option<usize>` / `clamped_layer_at_y(&self, y: f32, scroll_y: f32, num_layers: usize) -> usize` / `frame_x(&self, frame: usize, scroll_x: f32) -> f32` / `row_y(&self, row: usize, scroll_y: f32) -> f32`

### フリー関数（`timeline_ui.rs` 内）
- **`draw_floating_drag_badge(painter: &egui::Painter, badge_rect: Rect, accent_color: Color32, text: &str, font_size: f32)`** — フレーム移動／複製ドラッグ中やレイヤー並べ替えドラッグ中にカーソル横へ追従するフローティングバッジを描画。
- **`draw_header_mini_button(painter: &egui::Painter, rect: Rect, label: &str, font_size: f32, enabled: bool, hovered: bool, hover_fill: Color32)`** — レイヤー一覧左上ヘッダーの `[＋]` / `[🗑]` ミニボタンを描画。

### モジュール関数（`timeline_ui.rs` 内）
- **セル選択・移動・再生制御・キーフレーム・フレーム・レイヤー操作コントローラ**：
  - `select_timeline_cells(app: &mut Is2dApp, start_layer: usize, start_frame: usize, end_layer: usize, end_frame: usize)` — タイムラインのセル範囲を選択し、キャンバス上の対応図形も自動選択。
  - `scrub_ruler_to_frame(app: &mut Is2dApp, frame_idx: usize)` — ルーラーのスクラブ等でレイヤー選択を解除したまま指定フレームへ移動。
  - `switch_to_layer_and_frame(app: &mut Is2dApp, layer_idx: usize, frame_idx: usize)` — 指定レイヤー・指定フレームへ移動して単一セル選択状態にする。
  - `step_frame(app: &mut Is2dApp, delta: isize)` — 前後のフレームへ1コマ移動（ループ設定対応）。
  - `toggle_playback(app: &mut Is2dApp)` / `update_playback(app: &mut Is2dApp, ctx: &egui::Context)` — 再生開始・停止と、`1.0 / fps` 秒ごとのフレーム進行・再描画要求。
  - `timeline_insert_keyframe(app: &mut Is2dApp, clone_prev: bool) -> bool` — `F6`（複製KF）・`F7`（空白KF）の挿入（既存KF先頭で押した場合は自動的に次フレームへ進んで作成する Adobe Animate 準拠挙動）。
  - `timeline_delete_keyframe(app: &mut Is2dApp) -> bool` — `Shift+F6` によるキーフレーム削除。
  - `timeline_extend_frame(app: &mut Is2dApp) -> bool` / `timeline_remove_frame(app: &mut Is2dApp) -> bool` — `F5`（フレーム延長：未作成フレーム選択時はそのフレームまで一括延長）および `Shift+F5`（フレーム短縮・削除）。
  - `timeline_move_or_copy_frames(app: &mut Is2dApp, sel: TimelineCellSelection, delta_layer: isize, delta_frame: isize, copy_mode: bool) -> bool` — 選択フレーム範囲のドラッグ移動および `Alt`+ドラッグ複製。
  - `set_layer_visible(app: &mut Is2dApp, layer_idx: usize, visible: bool)` / `set_layer_locked(app: &mut Is2dApp, layer_idx: usize, locked: bool)` — レイヤーの表示/非表示（👁）とロック（🔒）の切り替え。
  - `timeline_add_layer(app: &mut Is2dApp)` / `timeline_delete_active_layer(app: &mut Is2dApp) -> bool` / `timeline_reorder_layer(app: &mut Is2dApp, from_idx: usize, to_idx: usize) -> bool` / `timeline_move_active_layer(app: &mut Is2dApp, up: bool) -> bool` — レイヤーの追加・削除・重ね順変更。
- **パネル全体＆コントロールバー描画**：
  - `draw_timeline_panel(app: &mut Is2dApp, ctx: &egui::Context)` — 下部タイムラインパネル全体（コントロールバー＋X-Sheet本体）を描画。
  - `draw_timeline_control_bar(app: &mut Is2dApp, ui: &mut egui::Ui)` — 上部コントロールバーを描画（以下4つのセクション関数を呼び出し）。
  - `draw_control_bar_layer_buttons(app: &mut Is2dApp, ui: &mut egui::Ui)` — レイヤー追加（`+`）・削除（`🗑`）・上へ移動（`▲`）・下へ移動（`▼`）ボタンを描画。
  - `draw_control_bar_transport_buttons(app: &mut Is2dApp, ui: &mut egui::Ui)` — 先頭フレーム・前コマ・再生/一時停止・次コマ・最終フレーム・ループ切替・現在フレーム番号・FPS `DragValue` を描画。
  - `draw_control_bar_keyframe_buttons(app: &mut Is2dApp, ui: &mut egui::Ui)` — `F6 KF`（複製キーフレーム）、`F7 空KF`（空白キーフレーム）、`+F5`（延長）、`-F5`（短縮）、`KF削除`、`T Tween`（クラシックトゥイーン切替）ボタンを描画。
  - `draw_control_bar_onion_controls(app: &mut Is2dApp, ui: &mut egui::Ui)` — オニオンスキン有効トグルと前後参照フレーム数（`前` / `後` `DragValue`）を描画。
- **スクロール＆ポインタ（マウス・ドラッグ）操作**：
  - `handle_timeline_scroll(app: &mut Is2dApp, ctx: &egui::Context, body_rect: egui::Rect, grid_w: f32, grid_h: f32, max_cols: usize, total_layers: usize)` — ホイール・トラックパッドによる水平/垂直スクロール制御。
  - `handle_timeline_pointer_input(app: &mut Is2dApp, ctx: &egui::Context, response: &egui::Response, layout: &TimelineLayout)` — タイムライン上のポインタ入力を統合処理（以下の開始・ドラッグ更新・カーソル更新関数へ振り分け）。
  - `update_timeline_cursor_icon(app: &Is2dApp, ctx: &egui::Context)` — タイムラインのドラッグ状態（フレーム移動・レイヤー並べ替え）に応じたOSカーソル更新（状態遷移ロジックからUIカーソル副作用を分離）。
  - `handle_timeline_click_start(app: &mut Is2dApp, pos: Pos2, shift_held: bool, layout: &TimelineLayout)` — ルーラークリック、フレームセルクリック（`Shift` 範囲選択・選択済みセルの再ドラッグによる移動開始）、レイヤー行クリック（👁表示切替・🔒ロック切替・レイヤーD&D開始）を処理。
  - `update_timeline_grid_drag(app: &mut Is2dApp, pointer_pos: Option<Pos2>, primary_down: bool, primary_released: bool, alt_held: bool, layout: &TimelineLayout)` — ルーラースクラブ、セル矩形範囲選択、フレーム移動／`Alt` 複製ドラッグの状態更新と確定。
  - `update_timeline_layer_drag(app: &mut Is2dApp, pointer_pos: Option<Pos2>, primary_released: bool, layout: &TimelineLayout)` — レイヤー重ね順ドラッグ＆ドロップのドロップ確定。
- **X-Sheet グリッド・ルーラー・レイヤー一覧描画**：
  - `draw_timeline_frame_grid(app: &Is2dApp, painter: &egui::Painter, layout: &TimelineLayout, max_cols: usize, alt_held: bool, pointer_pos: Option<Pos2>)` / `draw_timeline_layer_row(app: &Is2dApp, painter: &egui::Painter, layout: &TimelineLayout, max_cols: usize, ui_row: usize, layer_idx: usize)` / `draw_move_frames_ghost(app: &Is2dApp, painter: &egui::Painter, layout: &TimelineLayout, alt_held: bool, pointer_pos: Option<Pos2>)` — フレームグリッド背景、5フレームごとの目盛り列、各レイヤーのキーフレーム行、選択セルの青ハイライト、フレーム移動／複製のゴースト枠・バッジ、赤い再生ヘッド縦線を描画。
  - `draw_keyframe_span(app: &Is2dApp, painter: &egui::Painter, layout: &TimelineLayout, layer: &Layer, kf_idx: usize, kf_end: usize, row_y: f32)` / `draw_keyframe_tween_or_end_marker(painter: &egui::Painter, tween_state: TweenState, span_rect: egui::Rect, kf_len: usize, row_y: f32, text_col: Color32)` — 1つのキーフレームスパンの背景色（通常＝白/グレー、トゥイーン有効＝薄紫）、クラシックトゥイーンの実線矢印／未完横点線、先頭丸印（塗りあり＝黒丸 `●`、空＝白抜き丸 `○`）、保持スパン末尾の白抜き長方形マーカー `▯` を描画。
  - `draw_timeline_ruler(app: &Is2dApp, painter: &egui::Painter, layout: &TimelineLayout, max_cols: usize)` — 上部ルーラーのフレーム番号目盛り、オニオンスキン範囲帯、赤の再生ヘッドマーカーを描画。
  - `draw_timeline_layer_list(app: &mut Is2dApp, painter: &egui::Painter, layout: &TimelineLayout, pointer_pos: Option<Pos2>, primary_clicked: bool)` — 左側カラムの各レイヤー行（アクティブ強調、レイヤー名、👁表示アイコン、🔒ロックアイコン）を描画。

---

## 9. [`symbol.rs`](./symbol.rs) — シンボル・ライブラリ・クラシックトゥイーンの純粋ドメインモデル

### 構造体・列挙型（`symbol.rs` 内）
- **`struct SymbolInstance`** — セル（`Cel::instances`）内に正規化して保持されるシンボルインスタンスエンティティ（`binding: SymbolInstanceBinding`, `layer_id: u64`, `parent_scope: Vec<u64>`, `shape_order_index: usize`）。非シンボル図形（`Cel::shapes`）とは完全分離して1インスタンス＝1エントリとして管理され、`shape_order_index` によって非シンボル図形との前後重ね順を保持する。
- **`enum TweenState`** — キーフレーム区間のクラシックトゥイーン状態：
  - `None`（トゥイーン無効）
  - `Incomplete`（トゥイーンONだが条件未達 → タイムライン上に横点線を表示）
  - `Complete { start_frame, end_frame, start_binding, end_binding, rotate_mode }`（開始・終了ともに同一シンボルの単一インスタンスのみ → タイムライン上に実線矢印を表示しTRS線形補間を実行）
- **`type SymbolMiniPreviewCacheEntry`** / **`struct SymbolDef`** — ライブラリに登録されるシンボル定義（`id: u64`, `name: String`, `timeline: Timeline`, `revision: u64`, `mini_preview_cache: SyncCache<Option<SymbolMiniPreviewCacheEntry>>`）。内部タイムライン状態として `Timeline` 構造体を直接保持することで、`Timeline` と `SymbolDef` のフィールド重複と手動コピーを完全に排除。
  - `new(id: u64, name: String, local_shapes: Vec<FillShape>) -> Self` — ローカル図形群から1レイヤー・1フレームの初期 `Timeline` を持つシンボル定義を作成。
  - `cached_mini_preview(&self, rect: egui::Rect, library_revision: u64, library: &[SymbolDef]) -> Option<(Arc<egui::Mesh>, egui::Pos2)>` — ライブラリパネルのミニプレビュー矩形 `rect` に対応するスクリーン変換済み結合 `Arc<egui::Mesh>` と基準点スクリーン座標 `Pos2` を `SyncCache` 経由で $O(1)$ で返す。
  - `has_dynamic_animation(&self, library: &[SymbolDef], depth: usize) -> bool` — シンボル自身または入れ子の子シンボルが複数フレームや先頭空白フレームを持つかを再帰判定。
  - `first_non_empty_local_shapes(&self, library: &[SymbolDef]) -> Vec<FillShape>` — シンボルの全フレームを先頭から走査し、最初に図形が存在するフレームのローカル図形列を返す（1コマ目が空白のマルチフレームシンボルの配置・プレビュー用）。
  - `collect_visible_local_shapes_at(&self, frame: usize, child_eval_frame: usize, ctx: SymbolEvalCtx<'_>) -> Vec<FillShape>` — 指定した内部フレームにおける全可視レイヤーのローカル図形を収集（入れ子シンボル・内部トゥイーンも再帰評価）。
  - `resolve_internal_frame(&self, binding: SymbolInstanceBinding, kf_start_frame: usize, current_frame: usize) -> usize` — インスタンスの再生モード（`Loop` / `PlayOnce` / `SingleFrame`）と `first_frame`、および親キーフレーム先頭からの経過フレーム数から表示すべき内部フレーム番号（0始まり）を算出。
  - `evaluated_local_shapes_for_instance(&self, binding: SymbolInstanceBinding, kf_start_frame: usize, current_frame: usize, ctx: SymbolEvalCtx<'_>) -> Vec<FillShape>` — インスタンス設定に基づいて評価されたローカル図形群を返す。
  - `evaluated_local_shapes_at(&self, offset_frame: usize, library: &[SymbolDef]) -> Vec<FillShape>` — デフォルトのループ評価でローカル図形群を返す（ライブラリのミニプレビュー等で使用）。
  - `update_from_working_timeline(&mut self, working_tl: &Timeline, inv_xform: Affine, stage_to_local_cache: &mut StageToLocalCache)` — インプレイス編集中の作業タイムラインをクローンし、未変更キーフレームは `StageToLocalCache::cels` の論理リビジョン `Cel::revision` 一致により $O(1)$ でローカル `Cel` を共有復元、編集されたキーフレーム内でも未編集図形は複合キー `StageShapeCacheKey`（`(contour_id, color, holes.len(), holes_hash)`）による `StageToLocalCache::shapes` から $O(1)$ でローカル原本を無劣化復元し、新規・変形図形のみ逆アフィン変換 `inv_xform` を適用して `self.timeline` へ反映。
  - `strip_transient_caches(&mut self)` — `self.timeline.strip_transient_caches()` および `mini_preview_cache` のクリアにより一時メッシュキャッシュを破棄。
- **`struct SymbolEvalCtx<'a>`** — シンボル再帰評価時の設定コンテキスト（`library`, `depth`）。
- **`type StageShapeCacheKey = (u64, [u8; 4], usize, u64)`** / **`struct StageToLocalEntry`** / **`struct StageCelToLocalEntry`** / **`struct StageToLocalCache`** — シンボルインプレイス編集時に未変更キーフレームのローカル `Cel`（`cels: HashMap<u64, StageCelToLocalEntry>`、`Cel::revision` キー）および未変形図形のローカル `FillShape`（`shapes: HashMap<StageShapeCacheKey, StageToLocalEntry>`、`(contour_id, color, holes.len(), holes_hash)` 複合キー）を生メモリアドレスに依存しない論理IDの2階層すべて $O(1)$ で無劣化逆引きするキャッシュ構造体。
- **`struct SymbolEditSession`** — シンボルをダブルクリックしてインプレイス編集する際の親タイムライン退避セッション（`symbol_id`, `instance_gid`, `instance_xform`, `stage_to_local_cache: StageToLocalCache`, `parent_timeline`, `parent_editing_group_path`, `backdrop_cel: Cel`）。`strip_transient_caches(&mut self)` により Undo 履歴保存時に一時キャッシュを破棄。
- **`struct SymbolManager`** — シンボルライブラリ（`library`）・選択中シンボル（`selected_library_symbol`）・連続リネーム中シンボル（`actively_renaming_symbol`）・シンボル内部編集スタック（`edit_stack`）・単調増加IDカウンタ（`next_symbol_id`, `next_group_id`）・CoW履歴共有世代番号（`revision`）を保持する構造体。ID採番・世代番号更新・シンボル登録/検索/改名・循環参照防止（DAGサイクル検査）・削除＆非破壊デタッチ・インプレイス編集同期の固有メソッド（`alloc_group_id`, `bump_revision`, `register_new_symbol`, `find_symbol`, `find_symbol_mut`, `would_create_symbol_cycle`, `can_place_symbol_in_current_context`, `rename_symbol`, `sync_after_restore`, `remove_symbol_and_detach`, `sync_active_edit`）を自己完結で実装。
- **`struct ResolvedCelSig`** / **`enum ResolvedSigQuery<'a>`** — 解決済みセルおよび背景セルキャッシュの衝突ゼロ構造化署名（`cel_revision`, `start_frame`, `tween`, `tween_rotate`, `next_kf`, `referenced_symbols: Vec<(u64, u64)>`）と、既存のキャッシュ済み署名に対してヒープアロケーション・再帰走査を一切行わずに整合性を判定するクエリ型（`matches` / `compute` / `matches_layer_keyframe` / `matches_cel`）。64bitハッシュ値ではなく構造体の完全一致比較を行うことでハッシュ衝突リスクを排除し、かつ再生中・オニオンスキン描画中の毎フレーム一時 `Vec` 確保もゼロ化。

### フリー関数（`symbol.rs` 内）
- **`resolve_cel_with_library(cel: &Cel, kf_start_frame: usize, frame: usize, ctx: SymbolEvalCtx<'_>) -> Vec<FillShape>`** — セル内の図形配列と正規化インスタンス一覧（`cel.instances`）に対し、シンボルインスタンス（入れ子シンボル・空白フレーム後の復活含む）を現在フレーム・再生モードに応じた内部フレームの図形へ再帰展開（`ctx.depth == 0` かつ現在フレームの内部図形が0個の場合は `FillShape::new_empty_symbol_marker` を `shape_order_index` 位置に出力）。
- **`collect_referenced_symbol_revisions(sym_id: u64, library: &[SymbolDef], visited: &mut Vec<(u64, u64)>, depth: usize)`** / **`compute_resolved_cel_signature(layer: &Layer, kf_idx: usize, library: &[SymbolDef]) -> ResolvedCelSig`** / **`compute_cel_signature(cel: &Cel, library: &[SymbolDef]) -> ResolvedCelSig`** — セルの論理リビジョン・トゥイーン状態に加え、当該セル（およびトゥイーン終端セル）が直接・間接に参照しているシンボル定義の `(id, revision)` リストを再帰収集して衝突ゼロの `ResolvedCelSig` 署名を構築。
- **`stage_shape_cache_key(s: &FillShape) -> StageShapeCacheKey`** / **`lookup_untouched_local_shape<'a>(s: &FillShape, primary_cache: &'a HashMap<StageShapeCacheKey, StageToLocalEntry>, fallback_cache: &'a HashMap<StageShapeCacheKey, StageToLocalEntry>) -> Option<&'a StageToLocalEntry>`** / **`transform_layers_with_local_cache(local_layers: &[Layer], xform: Affine) -> (Vec<Layer>, StageToLocalCache)`** — シンボル定義のローカルレイヤー配列を `xform` でステージ座標へ変換し、同時に2階層 $O(1)$ 無劣化逆引き用の `StageToLocalCache` を構築・検索。

---

## 10. [`vector_engine.rs`](./vector_engine.rs) — 2Dベクター幾何コア・アフィン変換・メッシュ化エンジン

### 定数・アトミックカウンタ・スレッドローカル変数
- **`REFERENCE_BRUSH_RADIUS: f64`** (`8.0`) — ブラシ太さに依存せず一定のスムージング品質を保つための基準ブラシ半径。
- **`MIN_VIEW_ZOOM: f32`** (`0.15`) / **`MAX_VIEW_ZOOM: f32`** (`16.0`) — ビューポートの最小・最大ズーム倍率定数（全モジュール共通）。
- **`NEXT_MESH_REVISION: AtomicU64`** / **`next_mesh_revision() -> u64`** — 図形メッシュが更新されるたびに一意なリビジョン番号を発行するアトミックカウンタ。
- **`NEXT_SHAPE_ID: AtomicU64`** / **`next_shape_id() -> u64`** — 図形インスタンスごとに一意なエンティティID（`shape_id`）を発行するアトミックカウンタ。
- **`NEXT_CONTOUR_ID: AtomicU64`** / **`next_contour_id() -> u64`** — 輪郭セグメント内容ごとに一意な論理ID（`contour_id`）を発行するアトミックカウンタ。
- **`TESSELLATOR: RefCell<FillTessellator>`** — `lyon` の内部作業バッファをスレッドローカルで再利用し、毎回の `FillTessellator::new()` ヒープ再確保をゼロにするキャッシュ。

### 構造体・列挙型（`vector_engine.rs` 内）
- **`struct SyncCache<T>`** — `Arc` で共有されるデータ構造内の一時メッシュキャッシュを `Send + Sync` かつ `Clone` / `Debug` / `Default` 対応で保持するためのスレッドセーフな内部可変キャッシュラッパー（`with_mut` / `clear`）。
- **`enum SymbolLoopMode`** — グラフィックシンボルの再生モード列挙型（`Loop`, `PlayOnce`, `SingleFrame`）。
- **`enum TweenRotate`** — クラシックトゥイーンの回転方向列挙型（`Auto`, `Clockwise`, `CounterClockwise`）。
- **`struct SymbolInstanceBinding`** — シンボルインスタンスの幾何・再生紐付け情報（`instance_gid: u64`, `symbol_id: u64`, `xform: Affine`, `pivot: Point`, `loop_mode: SymbolLoopMode`, `first_frame: usize`）。
  - `new(instance_gid: u64, symbol_id: u64, xform: Affine) -> Self` / `world_pivot(self) -> Point` / `instantiate_shapes(&self, local_shapes: &[FillShape], layer_id: u64, parent_scope: &[u64]) -> Vec<FillShape>` / `parent_scope_from_path<'a>(&self, group_path: &'a [u64]) -> &'a [u64]`
- **`struct Contour`** — 閉じた2次ベジェ曲線（`contour_id: u64`, `segments: Arc<Vec<QuadBez>>`、Copy-on-Write）の輪郭ループ（外周または穴）。
  - `new(segments: Vec<QuadBez>) -> Self` / `segments_mut(&mut self) -> &mut Vec<QuadBez>` / `ptr_eq(&self, other: &Self) -> bool`
  - `translate(&mut self, delta: Vec2)` / `transform(&mut self, affine: Affine)` — 非有限座標・特異行列（面積ゼロ潰れ）をガードした輪郭の平行移動およびアフィン変換。
  - `bounding_box(&self) -> kurbo::Rect` — `BezPath` ヒープアロケーションなしで各 `QuadBez` から直接バウンディングボックスを算出。
  - `append_to_bez_path(&self, path: &mut BezPath)` — `kurbo::BezPath` へ閉輪郭パスを追記。
  - `signed_area(&self) -> f64` — 輪郭の符号付き面積を計算（スクリーン座標系で外周時計回り `> 0`、穴反時計回り `< 0`）。
  - `cleanup_degenerate(&mut self, min_len: f64)` — 長さほぼゼロの退化セグメントや非有限座標を除去し、端点の連続性を保証。
  - `from_segments_oriented(segments: Vec<QuadBez>, min_seg_len: f64, min_area: f64, expect_positive_area: bool) -> Option<Self>` / `reverse(&mut self)` / `enforce_continuity(&mut self)` — ワインディング向きを揃えた `Contour` の構築・反転・接続点補正。
  - `insert_anchor_at(&mut self, seg_idx: usize, t: f64) -> bool` — De Casteljau 分割（`subsegment`）により曲線形状を保ったまま指定パラメータ `t` にアンカーポイントを挿入。
  - `delete_anchor_at(&mut self, seg_idx: usize) -> bool` — 指定アンカーポイントを削除し、前後のアンカー間を直線セグメント（中点制御点）で繋ぐ。
- **`type ScreenMeshCacheEntry`** / **`type OnionMeshCacheEntry`** / **`type SelectionScreenMeshCacheEntry`** — スクリーン座標変換済みメッシュキャッシュ、オニオンスキン変換済みメッシュキャッシュ、および選択時ドット柄オーバーレイのスクリーン座標変換済みメッシュキャッシュのキーと `Arc<egui::Mesh>` のペア。
- **`struct FillShape`** — 単一カラーの塗りベクター図形（`shape_id: u64`, `color: Color32`, `exterior: Contour`, `holes: Vec<Contour>`, `layer_id: u64`, `group_path: Vec<u64>`, `symbol_instance: Option<SymbolInstanceBinding>`, `mesh_revision: u64`, `bbox: Rect`, `fill_mesh: OnceLock<Arc<egui::Mesh>>`, `screen_mesh_cache: SyncCache<Option<ScreenMeshCacheEntry>>`, `selection_mesh: Option<(u32, Arc<Mesh>)>`, `selection_screen_mesh: Option<SelectionScreenMeshCacheEntry>`）。`fill_mesh` を `OnceLock` による遅延生成キャッシュ、`screen_mesh_cache` を `SyncCache` とすることで、幾何計算中の中間図形での不要なテッセレーションと Undo 履歴での頂点バッファ保持を排除しつつ `Send + Sync` を満たす。
  - `new(color: Color32, exterior: Contour, holes: Vec<Contour>) -> Self`
  - `new_empty_symbol_marker(binding: SymbolInstanceBinding, layer_id: u64, parent_scope: &[u64]) -> Self` / `is_empty_symbol_marker(&self) -> bool` — 現在フレームで内部図形が0個（空白キーフレーム等）のシンボルインスタンスを `canvas.shapes` 上の正確なZ順序位置で表現するゼロ面積（`exterior.segments.is_empty()`）マーカーの生成・判定。
  - `fill_mesh(&self) -> &Arc<egui::Mesh>` — 三角形メッシュを `OnceLock::get_or_init` で遅延テッセレーション・キャッシュ取得（空白シンボルマーカーの場合は空メッシュを即座に返す）。
  - `cached_screen_mesh(&self, zoom: f32, pan: Vec2, dim_backdrop: bool) -> Arc<egui::Mesh>` — 編集可能レイヤー図形のスクリーン座標変換済みメッシュを `Arc<egui::Mesh>` キャッシュ経由で取得し、静止時の毎フレーム頂点座標変換と `Vec<Vertex>` / `Vec<u32>` クローンを回避。
  - `strip_transient_caches(&mut self)` — `fill_mesh`・`screen_mesh_cache`・`selection_mesh`・`selection_screen_mesh` を破棄し、Undo スナップショット保存時のメモリ消費を最小化するとともにアクティブ図形側の `Arc::make_mut` 頂点バッファ複製を防止。
  - `with_layer_id(self, layer_id: u64) -> Self` / `with_group_path(self, group_path: Vec<u64>) -> Self` / `with_scope(self, layer_id: u64, group_path: &[u64]) -> Self`
  - `has_same_contour_arcs(&self, exterior: &Contour, holes: &[Contour]) -> bool` / `restore_from_local_template(&self, local_template: &FillShape, inv_xform: Affine) -> FillShape` — `Arc::ptr_eq` で輪郭データの無変更を $O(1)$ 判定し、属性（色・レイヤー・グループパス・シンボル紐付け）を引き継ぎつつローカル頂点とキャッシュ済みメッシュを完全無劣化で復元するヘルパー。
  - `contours(&self) -> impl Iterator<Item = &Contour>` — 実体のある外周とすべての穴を順に走査するイテレータ（空白フレームのシンボルマーカーの場合は 0 件の空イテレータを返し、呼び出し側の個別ガードを不要化）。
  - `is_in_editing_scope(&self, scope: &[u64]) -> bool` — 図形が現在のグループ編集スコープ内に属するか判定。
  - `is_raw_at_scope(&self, scope: &[u64]) -> bool` — 現在の編集スコープにおいてグループ・シンボルに属さない「生のシェイプ（Merge Drawing 対象）」であるか判定。
  - `active_group_id_at_scope(&self, scope: &[u64]) -> Option<u64>` — 現在の編集スコープ直下の子グループID（またはシンボルインスタンスID）を返す。
  - `rebuild_cache(&mut self)` — バウンディングボックス、`Arc<Mesh>`、メッシュリビジョンを再計算し、選択メッシュキャッシュを無効化。
  - `set_color(&mut self, color: Color32)` — 再テッセレーションを行わずに塗り色とキャッシュ済み頂点カラーのみを高速更新。
  - `bounding_box(&self) -> Rect` / `geometry_bounding_box(&self) -> Option<Rect>`（実体のあるベクター輪郭を持つ場合のみ `Some(bbox)`、空白フレームのシンボルマーカーでは `None`） / `contains_point(&self, pt: Point) -> bool`（AABB 早期棄却＋`kurbo::BezPath::winding` による EvenOdd 内外判定）
  - `translate(&mut self, delta: Vec2)` — 制御点とキャッシュ済み `fill_mesh`・`selection_mesh` の頂点座標を直接平行移動（再テッセレーション不要）。
  - `transform(&mut self, affine: Affine)` — 非有限値・ゼロ面積潰れをガードした上で輪郭にアフィン変換を適用。等倍回転・縮小時はキャッシュ済み `fill_mesh` の頂点にアフィン変換を直接適用して高速化し、拡大変形時（`max_scale_sq > 1.25 * 1.25`）は曲線のカクつきを防ぐため `rebuild_cache()` で再テッセレーションを実行。
  - `transform_with_cached_mesh(&mut self, affine: Affine)` — シンボルインスタンス展開やトゥイーン補間時に、既にテッセレーション済みのローカル `fill_mesh` を破棄せず頂点アフィン変換で維持し、再生・オニオンスキン描画での毎フレームの再テッセレーションを回避。
  - `selection_overlay_mesh(&mut self, zoom: f32) -> Arc<egui::Mesh>` / `cached_selection_screen_mesh(&mut self, zoom: f32, pan: Vec2) -> Arc<egui::Mesh>` — 選択時の白い網点（Polka-Dot）ワールドメッシュおよびスクリーン変換済みメッシュを遅延生成・キャッシュ取得。
  - `get_contour(&self, idx: usize) -> Option<&Contour>` / `get_contour_mut(&mut self, idx: usize) -> Option<&mut Contour>` — インデックス（`0`＝外周、`1..`＝穴）による輪郭アクセス。

### フリー関数（`vector_engine.rs` 内）
- **アフィン変換・マーキー矩形生成・バウンディングボックス合成**：
  - `affine_scale_around(anchor: Point, sx: f64, sy: f64) -> Affine` — 指定アンカー中心のスケール行列を生成。
  - `affine_rotate_around(pivot: Point, angle_rad: f64) -> Affine` — 指定ピボット中心の回転行列を生成。
  - `interpolate_affine_with_pivot(start: Affine, start_pivot: Point, end: Affine, end_pivot: Point, rotate_mode: TweenRotate, t: f64) -> (Affine, Point)` — 2つのアフィン変換行列をローカルピボット `start_pivot`, `end_pivot` を軸として TRS（ピボットのワールド平行移動・回転角度・スケール・シアー）に分解し、滑らかに補間（クラシックトゥイーン用）。
  - `decompose_affine_trs(affine: Affine) -> (f64, f64, f64, f64, f64, f64)` — 2Dアフィン行列を `(tx, ty, angle, sx, sy, shear)` へ極分解。
  - `make_marquee_rect_shape(rect: Rect) -> FillShape` — 矩形 `Rect` からマーキースライス・交差判定用の `FillShape` を生成。
  - `union_bounding_boxes(shapes: &[FillShape]) -> Rect` — 図形スライスの外接矩形の合併（Union）を計算（空スライスの場合は `Rect::ZERO`）。
- **幾何ユーティリティ関数**：
  - `ring_f64_to_points(ring: &[[f64; 2]]) -> Vec<Point>` / `normalize_vec(v: Vec2) -> Vec2` / `left_normal(v: Vec2) -> Vec2` / `signed_angle(a: Vec2, b: Vec2) -> f64` / `to_pos2(p: Point) -> Pos2`
  - `fill_cumulative_arc_lengths(pts: &[Point], cum: &mut Vec<f64>)` / `dedup_closed_ring(pts: impl IntoIterator<Item = Point>, min_dist: f64) -> Vec<Point>`
  - `signed_area_f64(pts: &[[f64; 2]]) -> f64` / `ring_area_and_perimeter_f64(ring: &[[f64; 2]]) -> (f64, f64)` / `polygon_signed_area(pts: &[Point]) -> f64`
  - `point_to_segment_dist(pt: Point, a: Point, b: Point) -> f64` / `rects_overlap(a: Rect, b: Rect) -> bool`
- **`lyon` テッセレーション＆スクリーンメッシュ変換**：
  - `tessellate_contours_to_mesh(exterior: &Contour, holes: &[Contour], color: Color32) -> egui::Mesh` — 外周と穴の2次ベジェ輪郭をスレッドローカルの `TESSELLATOR`（`FillRule::EvenOdd`）で三角形メッシュ化。
  - `build_polka_dot_selection_mesh(fill_mesh: &egui::Mesh, zoom: f32) -> egui::Mesh` — 図形の三角形メッシュとグリッド状の小さな正方形群の交差から、Flash 風の白い網点（Polka-Dot）選択オーバーレイメッシュを生成。
  - `tessellate_raw_polygon(pts: &[Point], color: Color32) -> egui::Mesh` — ドラッグ中の生アウトラインポリゴンをスレッドローカルの `TESSELLATOR` で三角形メッシュへ分割。
  - `tessellate_lyon_path(tessellator: &mut FillTessellator, path: &LyonPath, color: Color32, fill_rule: FillRule) -> egui::Mesh` — `lyon::path::Path` から中間バッファなしで `egui::epaint::Vertex` 配列を直接出力する `egui::Mesh` 変換コア。
  - `transform_mesh_to_screen(mesh: &egui::Mesh, zoom: f32, pan: Vec2, dim_backdrop: bool) -> egui::Mesh` / `transform_mesh_to_screen_onion(mesh: &egui::Mesh, zoom: f32, pan: Vec2, tint_rgb: [u8; 3], alpha: u8) -> egui::Mesh` — ワールド座標メッシュの頂点配列を現在のズーム・パンでスクリーン座標へ一括変換（通常用およびオニオンスキン単色用）。
  - `append_mesh(dst: &mut egui::Mesh, src: &egui::Mesh)` — 複数の `egui::Mesh` の頂点・インデックス配列を1つのバッチメッシュへ結合。
  - `arc_mesh_to_shape(mesh: Arc<egui::Mesh>) -> egui::Shape` — `Arc::unwrap_or_clone(mesh)` により単一所有時はムーブ、共有時はクローンで `egui::Shape::Mesh` に変換する共通ヘルパー。

---

## 11. [`vector_stroke.rs`](./vector_stroke.rs) — アルゴリズム 1 & 2：ストローク輪郭生成＆100%スムージング曲線フィット

- **アルゴリズム1：ブラシ軌跡からの閉輪郭（アウトライン）生成**：
  - `generate_brush_outline(points: &[Point], radius: f64) -> Vec<Point>` / `generate_brush_outline_at_zoom(points: &[Point], radius: f64, view_zoom: f64) -> Vec<Point>` — ブラシ中心軌跡点列から進行方向左右の法線オフセット点列と始点・終点の半円キャップを計算し、単一の閉じた輪郭ポリゴンを生成（微小手ブレ除去フィルタ＋中心スパイン重み付け平均を含む）。
- **アルゴリズム2：100%スムージング＆G1連続2次ベジェ曲線フィッティング**：
  - 特徴点検出・G1連続化の幾何しきい値定数群（`CAP_SCAN_*`, `CAP_NOTCH_*`, `RIGHT_CORNER_*`, `JUNCTION_*`, `ROUND_ARC_*`, `PROTECTED_CAP_*`, `LOCAL_ROUND_CORNER_*`, `STRONG_ROUND_CORNER_*`, `G1_*`）を名前付き定数として定義。
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

---

## 12. [`vector_boolean.rs`](./vector_boolean.rs) — アルゴリズム 3：マージ描画＆ブーリアン演算エンジン

### 構造体・列挙型（`vector_boolean.rs` 内）
- **`enum BoolOp`** — ブーリアン演算種別（`Union`, `Difference`, `Intersection`）。
- **`type OverlayShape`** (`Vec<Vec<[f64; 2]>>`) — `i_overlay` へ渡す外周・穴リング集合の型エイリアス。
- **`struct PreparedClip`** — 複数の既存シェイプに対して繰り返し `Difference`（減算）を行う際、クリップ側図形群（新規ストロークや消しゴム輪郭）のフラットニング結果と `QuadRecord` をループ前に1回だけ事前構築して使い回すキャッシュ構造体（`new` / `subtract_from`）。
- **`struct QuadRecord`** / **`struct QuadSpatialGrid`** / **`struct TaggedEdge`** — ブーリアン演算後のポリゴン辺から元の2次ベジェ曲線セグメントを逆引き・De Casteljau 部分抽出するための内部レコードと2D均一グリッド空間インデックス（$O(N \times M)$ 線形探索を $O(N)$ セル検索へ高速化）。

### フリー関数（`vector_boolean.rs` 内）
- `merge_shapes_into_canvas(canvas: &mut Vec<FillShape>, new_shapes: Vec<FillShape>)` — 新規図形群を同じレイヤー・同じグループスコープ内の生シェイプに対して同色 `Union`・異色 `Difference` でマージ統合。
- `merge_disjoint_fill_into_canvas(canvas: &mut Vec<FillShape>, fill_shape: FillShape)` — バケツ塗りで検出された閉領域を周囲の同色生シェイプとのみ `Union` 統合（異色シェイプのDifference切り抜きを省略する高速パス）。
- `merge_shapes_into_canvas_inner(canvas: &mut Vec<FillShape>, incoming: Vec<FillShape>, subtract_diff_color: bool)` / `merge_homogeneous_batch_into_canvas(canvas: &mut Vec<FillShape>, incoming: Vec<FillShape>, subtract_diff_color: bool)` — 連続する同一レイヤー・同一スコープ・同色の図形群を1バッチにまとめ、1回の `FloatOverlay` パスで結合・切り抜きを実行するマージ描画の内部共通実装。
- `erase_from_canvas(canvas: &mut Vec<FillShape>, eraser_shapes: &[FillShape], target_layer_id: u64, editing_scope: &[u64])` — 消しゴム図形で対象スコープ内の生シェイプを `Difference` 減算。
- `slice_shape_with_marquee_rect(shape: &FillShape, clip_rect_shape: &FillShape) -> (Vec<FillShape>, Vec<FillShape>)` — 矩形マーキー部分選択によるスライス分割時に、対象図形と矩形のポリゴン分割および `QuadRecord` テーブル構築を1回だけ行い、1パスで `(inside, outside)` 両方の `FillShape` 群を返す高速スライサー。
- `adaptive_overlay_min_area(shapes_a: &[FillShape], shapes_b: &[FillShape]) -> f64` — 入力図形の最小バウンディングボックス短辺から `i_overlay` の最小面積閾値 `min_area` をスケール適応的に算出（1600%拡大表示での最小半径 `0.5px` 細密加筆・消しゴムでも微小図形が消失しないよう動的調整）。
- `boolean_operation(subject: &[FillShape], clip: &[FillShape], op: BoolOp, result_color: Color32) -> Vec<FillShape>` — `subject` と `clip` の2次ベジェ図形群に対して `Union` / `Difference` / `Intersection` を実行し、結果を再び滑らかな2次ベジェ `FillShape` 群として再構築。
- `run_float_overlay(subj_shapes: &[OverlayShape], clip_shapes: &[OverlayShape], rule: OverlayRule, fill_rule: OverlayFillRule, min_area: f64) -> Vec<OverlayShape>` — `i_overlay::FloatOverlay` の実行ラッパー。
- `reconstruct_fill_shape_from_rings(shape_rings: &[Vec<[f64; 2]>], source_quads: &[QuadRecord], grid: &QuadSpatialGrid, result_color: Color32, filter_sliver_holes: bool) -> Option<FillShape>` / `reconstruct_shapes_from_overlay(overlay_result: &[OverlayShape], quad_table: &[QuadRecord], color: Color32, filter_sliver_holes: bool) -> Vec<FillShape>` — ブーリアン演算結果のポリゴンリング群からスケール適応許容差を用いて `FillShape` を復元。
- `poly_shape_contains_point(poly_shape: &[Vec<[f64; 2]>], pt: Point) -> bool` — ポリゴンリング群に対する高速な 2D Even-Odd レイキャスト内外判定（バケツ塗りでクリック座標を含まない全候補領域のベジェ再構築・テッセレーションをスキップするために使用）。
- `cull_enclosed_region_candidates(canvas: &[FillShape], target_pt: Point) -> Option<(Vec<&FillShape>, Rect)>` — バケツツール用：キャンバス内の図形群を微膨張AABB（`inflate(1.0, 1.0)`）の交差による連結成分（クラスタ）へ分類し、クリック座標 `target_pt` がどのクラスタのAABBにも含まれない開いた余白であれば `FloatOverlay` を呼ばずに $O(N)$ で即座に `None` を返し、含まれる場合も該当クラスタと交差する候補図形のみに絞り込む事前カリング関数。
- `find_enclosed_region_at(canvas: &[FillShape], target_pt: Point, fill_color: Color32) -> Option<FillShape>` — バケツツール用：`cull_enclosed_region_candidates` で抽出した候補図形群と局所バウンディングボックスのみに対して `FloatOverlay` を実行し、`poly_shape_contains_point` を通過した唯一の閉領域のみをベクター図形化。
- `flatten_shape_for_overlay(shape: &FillShape, quad_table: &mut Vec<QuadRecord>) -> OverlayShape` / `flatten_contour_for_overlay(contour: &Contour, is_outer: bool, quad_table: &mut Vec<QuadRecord>) -> Option<Vec<[f64; 2]>>` / `flatten_contour_preserving_direction(contour: &Contour, quad_table: &mut Vec<QuadRecord>) -> Option<Vec<[f64; 2]>>` — `FillShape` の各 `QuadBez` をタグ付き折れ線に細分化して `i_overlay` 入力形式へ変換。
- `resolve_deformed_shape(shape: &FillShape) -> Vec<FillShape>` — エッジ引き伸ばし・頂点移動後の輪郭に対して `OverlayFillRule::Positive` を実行し、自己交差の解消や2図形への横断分割を解決。
- `quad_subdivision_steps(q: QuadBez) -> usize` / `sample_contour_ring_for_overlay(contour: &Contour, quad_table: &mut Vec<QuadRecord>) -> Option<Vec<[f64; 2]>>` — `QuadBez` の長さと曲率に応じたスケール適応的な分割ステップ数算出と輪郭ポリゴンサンプリング。
- `reconstruct_or_refit_quad_contour(ring: &[[f64; 2]], quad_table: &[QuadRecord], grid: &QuadSpatialGrid, is_outer: bool) -> Option<Contour>` / `match_edge_to_quad(a: Point, b: Point, quad_table: &[QuadRecord], grid: &QuadSpatialGrid) -> Option<(usize, f64)>` / `refit_open_polyline_to_quads(pts: &[Point], epsilon: f64) -> Vec<QuadBez>` / `extract_sub_quad(q: QuadBez, p_start: Point, p_end: Point) -> QuadBez` — ブーリアン演算後の各ポリゴン辺を辺長に応じた動的距離許容差 `dist_tol` で元の `QuadBez` セグメントへ照合し、De Casteljau 法（`extract_sub_quad`）で無劣化に部分曲線を切り出して閉じた2次ベジェ輪郭へ再構築。

---

## 13. [`tests.rs`](./tests.rs) — 自動ユニットテストスイート（全24件）

`cargo test` で実行される全24件のユニットテストとテスト用ヘルパー関数：

- **ヘルパー関数**：
  - `make_stroke(p0: Point, p1: Point, radius: f32, color: Color32) -> Vec<FillShape>` — 2点間を結ぶ直線ブラシストロークを生成・スムージングして返す。
- **ユニットテスト一覧（全24件）**：
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
  23. `test_nested_symbol_evaluation_and_library_delete_detach` — 入れ子シンボル（親シンボル内の子シンボルアニメーション）の再帰評価と、ライブラリからシンボル削除時にステージ上の絵が消えず通常グループとしてデタッチされることを検証。
  24. `test_crash_resilience_against_degenerate_inputs_and_rapid_state_transitions` — `NaN`・無限大・空配列・ゼロ面積アフィン変換・スライス選択とシンボル出入り・Undo/Redo・範囲外レイヤー指定などの異常値や連続操作に対してパニックせず安全に動作することを検証。
