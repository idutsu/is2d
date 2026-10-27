# Clay Canvas パネル＆UIレイアウト設計ガイド (`PANEL.md`)

`Clay Canvas` の画面レイアウトとUIパネル群は、**Adobe Animate** のワークスペース構成に準拠し、すべて**角丸なし（`Rounding::ZERO`）のシャープなライトテーマ**で統一されています。

---

## 1. 全体レイアウト構成（5ペイン構成）

[`eframe::App::update`](file:///Users/dskzts/clay_canvas/src/main.rs) では、`egui` のパネル評価順序を利用して、左右のパネルが画面の上端から下端まで貫通し、中央のステージと下部タイムラインを挟み込むレイアウトを実現しています。

```text
+------+----------------------------------------------------+----------------+
|      |  ステージ上部スリムバー (シーン階層 / ズーム倍率)  |                |
|      +----------------------------------------------------+   プロパティ   |
| 左端 |                                                    |     パネル     |
|  縦  |                                                    | [ツール]       |
| ツー |                中央キャンバス (Stage)              | [オブジェクト] |
|  ル  |                                                    | [ドキュメント] |
| バー |                                                    |                |
|      +----------------------------------------------------+                |
|      |                                                    |                |
|      |            下部タイムラインパネル (X-Sheet)        |                |
+------+----------------------------------------------------+----------------+
```

| 描画順 | パネル名 | `egui` コンポーネント | 実装ファイル・メソッド |
| :---: | :--- | :--- | :--- |
| **1** | **左端縦ツールバー** | `SidePanel::left("left_vertical_toolbar")` (幅 `42px`) | [`src/panels.rs`](file:///Users/dskzts/clay_canvas/src/panels.rs) — `draw_left_toolbar` |
| **2** | **右端プロパティパネル** | `SidePanel::right("right_properties_panel")` (幅 `250px`) | [`src/panels.rs`](file:///Users/dskzts/clay_canvas/src/panels.rs) — `draw_right_properties_panel` |
| **3** | **下部タイムラインパネル** | `TopBottomPanel::bottom("timeline_panel")` (リサイズ可) | [`src/timeline.rs`](file:///Users/dskzts/clay_canvas/src/timeline.rs) — `draw_timeline_panel` |
| **4** | **ステージ上部スリムバー** | `TopBottomPanel::top("stage_header_bar")` (高さ `30px`) | [`src/panels.rs`](file:///Users/dskzts/clay_canvas/src/panels.rs) — `draw_stage_header_bar` |
| **5** | **中央キャンバス (Stage)** | `CentralPanel::default()` | [`src/main.rs`](file:///Users/dskzts/clay_canvas/src/main.rs) — `eframe::App::update` |

> [!IMPORTANT]
> `egui` では先に呼んだ `SidePanel` がウィンドウの上下端まで領域を確保するため、`draw_left_toolbar` と `draw_right_properties_panel` を `draw_timeline_panel` より前に呼ぶ順序を維持してください。

---

## 2. モジュール・クラス（構造体）構成（[`src/panels.rs`](file:///Users/dskzts/clay_canvas/src/panels.rs)）

左右のパネルとステージ上部バーのUIコードは、[`src/main.rs`](file:///Users/dskzts/clay_canvas/src/main.rs) が肥大化しないよう [`src/panels.rs`](file:///Users/dskzts/clay_canvas/src/panels.rs) に分離されています。

### ① 右側パネルの状態管理
- **`enum RightPanelTab`（上部の3タブ）**：
  - `RightPanelTab::Properties`（**プロパティ**：現在選択中のツールの設定を表示）
  - `RightPanelTab::Color`（**カラー**）
  - `RightPanelTab::Library`（**ライブラリ**）
  - `RightPanelTab::ALL` に定義された順序で上部のタブ切り替えバーが自動生成されます。
- **`struct PropertiesPanel`**：
  - `FlashBrushApp` の `properties_panel` フィールドとして保持され、現在選択中のタブ（`active_tab: RightPanelTab`、初期値は `RightPanelTab::Properties`）を保持します。

### ② セクション別に分割された描画メソッド

| パネル | メソッド名 | 役割 |
| :--- | :--- | :--- |
| **左端縦ツールバー** | `draw_left_toolbar` | 左パネル全体の枠組みと縦レイアウト |
| 〃 | `draw_left_main_tool_buttons` | `Tool::ALL` を走査してメインツールボタン群を描画 |
| 〃 | `draw_left_tool_sub_actions` | 選択中ツール固有のサブモード（アンカー追加/削除、反転/回転等）を描画 |
| 〃 | `draw_left_group_and_view_buttons` | グループ化・グループ解除・ワイヤーフレーム表示切替ボタンを描画 |
| 〃 | `draw_color_picker_controls` | 塗りカラースウォッチとプリセットカラーパレットを描画 |
| 〃 | `draw_left_history_buttons` | Undo / Redo / フレーム全消去ボタンを描画 |
| **ステージ上部バー** | `draw_stage_header_bar` | シーン/グループのパンくずナビゲーションと右端のズーム倍率（`100%`）表示 |
| 〃 | `draw_group_breadcrumb` | シーン・グループ階層表示と「1階層出る」ボタン |
| **右端パネル** | `draw_right_properties_panel` | 右パネル全体の枠組み・タブバー・各タブへのディスパッチ |
| 〃 | `draw_right_panel_tab_bar` | `RightPanelTab::ALL` を走査して上部の3タブ（`プロパティ` / `カラー` / `ライブラリ`）を描画 |
| 〃 | `draw_properties_tab` | 「**プロパティ**」タブの中身（現在選択中の `self.tool` に応じて下記のツール別関数へ振り分け） |
| 〃 | `draw_select_tool_properties` / `draw_transform_tool_properties` / `draw_brush_tool_properties` / `draw_eraser_tool_properties` / `draw_bucket_tool_properties` / `draw_eyedropper_tool_properties` | 各ツール選択時に表示するプロパティUI |
| 〃 | `draw_color_tab` | 「**カラー**」タブの中身を描画 |
| 〃 | `draw_library_tab` | 「**ライブラリ**」タブの中身を描画 |

---

## 3. 今後ツールのプロパティ項目や新しいタブを増やすときの手順

### 各ツールの「プロパティ」に設定項目を追加する場合
- [`src/panels.rs`](file:///Users/dskzts/clay_canvas/src/panels.rs) にあるツール別の専用関数（`draw_brush_tool_properties`、`draw_select_tool_properties` など）の中にそのままUI（スライダー、チェックボックス、ボタン等）を追加するだけで、そのツールを選んだときに自動で表示されます。

### 右パネル上部に新しいタブを増やす場合
1. [`src/panels.rs`](file:///Users/dskzts/clay_canvas/src/panels.rs) の `enum RightPanelTab` に新しい列挙子を追加します。
2. `RightPanelTab::ALL` と `RightPanelTab::label()` にラベル名を追加します（タブバーのボタン幅は自動で等分調整されます）。
3. `draw_right_properties_panel` の `match self.properties_panel.active_tab` に描画メソッドを追加します。
