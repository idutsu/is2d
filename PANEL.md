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

### ① 右側プロパティパネルの状態管理
- **`enum PropertyTab`**：
  - 現在切り替え可能なタブを定義しています：
    - `PropertyTab::Tool`（**ツール**）
    - `PropertyTab::Object`（**オブジェクト**）
    - `PropertyTab::Document`（**ドキュメント**）
  - `PropertyTab::ALL` に定義された順序でタブ切り替えボタンが自動生成され、ボタン幅もタブ数に応じて自動等分されます。
- **`struct PropertiesPanel`**：
  - `FlashBrushApp` の `properties_panel` フィールドとして保持され、現在選択中のタブ（`active_tab: PropertyTab`、初期値は `PropertyTab::Document`）や、将来追加されるパネル内の折りたたみ状態・サブ設定を保持します。

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
| 〃 | `draw_group_breadcrumb` | `🎬 シーン 1 ▸ 📦 Group #N` の階層表示と「1階層出る」ボタン |
| **右端プロパティパネル** | `draw_right_properties_panel` | 右パネル全体の枠組み・「プロパティ」見出し・タブディスパッチ |
| 〃 | `draw_property_tab_bar` | `PropertyTab::ALL` を走査して上部のタブ切り替えボタンを描画 |
| 〃 | `draw_tool_property_tab` | 「**ツール**」タブの中身を描画 |
| 〃 | `draw_object_property_tab` | 「**オブジェクト**」タブの中身を描画 |
| 〃 | `draw_document_property_tab` | 「**ドキュメント**」タブの中身を描画 |

---

## 3. 今後タブや設定項目を増やすときの手順

### 各タブの中身に機能を追加する場合
- **「ツール」タブに設定を足す**：[`src/panels.rs`](file:///Users/dskzts/clay_canvas/src/panels.rs) の `draw_tool_property_tab` 内で `match self.tool` を書き、ブラシサイズ・スムージング・消しゴムサイズなどのUIを追加します。
- **「オブジェクト」タブに設定を足す**：`draw_object_property_tab` 内に、選択中シェイプのバウンディングボックス座標（`X`, `Y`, `W`, `H`）や塗りカラー変更UIを追加します。
- **「ドキュメント」タブに設定を足す**：`draw_document_property_tab` 内に、ステージサイズ（`W`, `H`）・ステージ背景色・FPS設定などを追加します。

### 新しいタブ（例：「フレーム」タブなど）を増やす場合
1. [`src/panels.rs`](file:///Users/dskzts/clay_canvas/src/panels.rs) の `enum PropertyTab` に新しい列挙子（例：`Frame`）を追加します。
2. `PropertyTab::ALL` と `PropertyTab::label()` に `"フレーム"` を追加します（タブバーのボタン幅は自動で4等分に調整されます）。
3. `draw_right_properties_panel` の `match self.properties_panel.active_tab` に `PropertyTab::Frame => self.draw_frame_property_tab(ui)` を追加します。
