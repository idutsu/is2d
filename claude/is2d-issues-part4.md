# is2d レビュー 第4回（ファイル入出力とUI）課題リスト

## 1. 対象と範囲

- 対象コミット：`c93b64744b2f970e89c301a61e8590d6122f7839`（`c93b647`）。`cargo build --release` は成功
- 最後まで読んだファイル（合計 6804 行）

  | ファイル | 行数 |
  | :--- | ---: |
  | `src/project_io.rs` | 1577 |
  | `src/panels.rs` | 2110 |
  | `src/timeline_ui.rs` | 2086 |
  | `src/main.rs` | 1031 |

- 照らし合わせた章：ARCHITECTURE.md 1・3・4・7章（保存に関わる 5・6章の該当文も参照）、DOCUMENT.md 第0・1・2・4・7章
- 範囲外で読んだ箇所（呼び出し先の確認のみ）：`canvas_tool.rs`（`refresh_scene_base`・`SceneBaseCache::rebuild`・`set_active_frame`・`handle_navigation`・`UndoHistory` の未保存判定）、`doc_actions.rs`（`apply` 冒頭、レイヤー操作、フレーム移動）、`timeline.rs`（上限定数、編集可否判定、`new_with_instances`）、`clipboard_ops.rs`（`max_group_id_in_timeline`）、`symbol.rs`（展開上限定数）、`vector_engine.rs`（`arc_mesh_to_shape`）
- 検証用に、プロジェクト外の作業コピーへ一時的な計測テスト（`probe_*`）を追加して `cargo test --release` で実行した。プロジェクトのソースは変更していない

## 2. 課題一覧

| ID | 状態 | 重大度 | 観点 | 場所 | 内容 | 根拠 | 検証 | 発見元 |
| :--- | :--- | :--- | :--- | :--- | :--- | :--- | :--- | :--- |
| I-4001 | 未解決 | High | 堅牢性 | project_io.rs:574-593, 619-643（描画は panels.rs:1931） | 座標が有限だが巨大な値（1e13〜1e15 程度）の図形を含むファイルを読み込むと、最初の描画で曲線の平坦化が頂点を際限なく作り、メモリ不足でアプリが落ちる。読み込み時の検査は「有限かどうか」だけで、座標の大きさに上限がない | `SavedContour → Contour` は `v.is_finite()` で絞るだけ。2セグメントの図形1つで実測：1e12 → 594万頂点・1.2秒、1e13 → 1883万頂点・9.8秒のあと 320MB の確保に失敗して abort、1e15 → SIGKILL。ファイルは数百バイト | ✔（実測） | 初回レビュー（第4回） |
| I-4002 | 未解決 | High | 堅牢性 | project_io.rs:26-27, 574-703 | 100MB の読み込み上限の内側でも、図形数によってメモリが桁違いに膨らみ、上限近くのファイルではメモリ不足で落ちる。図形数・穴の数・インスタンス数・シンボル数・名前の長さのどれにも上限がない。定数のコメント「異常な巨大ファイルによるOOM防止」は成り立っていない | 1セルに小さな図形10万個（JSON 12MB）で実測：読込直後 RSS 175MB → セルの結合メッシュ作成後 1.0GB → 図形単位メッシュ作成後のピーク 1.86GB、初回描画に計約4秒。上限 100MB はこの約8倍の図形数を許すため、ピークは十数GB 規模になる見込み | ✔（10万図形で実測）／○（上限付近での OOM は外挿） | 初回レビュー（第4回） |
| I-4003 | 未解決 | High | 仕様適合性 | project_io.rs:1289-1297, 658-673, 27 | 現実的な規模のアニメーションが保存できなくなる。JSON は f64 を全桁で書き（1セグメント約121バイト）、F6 で複製したキーフレームも図形データを丸ごと重複して書く。保存データが 100MB を超えると上書き保存・名前を付けて保存・「保存して閉じる」がすべて失敗し、作業を残す手段がなくなる（閉じるときは「破棄」しか選べない） | ブラシ60本（181図形・6644セグメント）の絵：1キーフレームで 0.80MB。F6 で 24 コマにすると 19.3MB、140 コマ（24fps で約5.8秒）で 107MB となり `Err("保存データが最大サイズ上限を超えています (107 MB > 上限 100 MB)")`、未保存のまま。読み込み側も同じ上限のため、仮に保存できても開けない | ✔（実測） | 初回レビュー（第4回） |
| I-4004 | 未解決 | Medium | パフォーマンス | timeline_ui.rs:2039-2086（2073）、canvas_tool.rs:2891-2894 | 再生中は1コマ進むたびに `set_active_frame` → `refresh_scene_base` が土台キャッシュ（Scene 用）を作り直すが、再生中の描画は Scene を使わずセル単位のメッシュキャッシュを使うため、この作り直しは丸ごと無駄になっている。トゥイーンやシンボルのループを含むレイヤーは毎コマ図形を展開し直す。`update_playback` のコメント「再生中は毎コマの FillShape クローンを行わず」とも食い違う | 1500図形のシンボルをトゥイーンさせた1レイヤーで実測：土台キャッシュの作り直し 1.8ms/コマ。実際の描画側の解決（`resolved_cel_screen_mesh`）は 2.7ms/コマで、約7割の上乗せ。トゥイーンするレイヤーの数に比例して増える。停止時は `stop_playback` が作り直すので、再生中は不要 | ✔（実測） | 初回レビュー（第4回） |
| I-4005 | 未解決 | Medium | アーキテクチャ | panels.rs:1808-1821 | キャンバスの描画が2つの読み取り経路を持つ。通常のレイヤーは Scene から描くが、ロック中のレイヤーと再生中はすべて文書から直接セル単位のメッシュを描く。ARCHITECTURE.md 4章「描画・当たり判定…はすべて Scene を読む」に反し、同じロック中レイヤーでもスポイト（Scene）と描画（文書）で読む元が違う。今は Floating がロック・再生の前に確定されるため表示は一致しているが、上乗せ（Overlay）が絡む機能を足すと表示と当たり判定がずれる不具合を生みやすい | `if timeline_ui.is_playing \|\| is_locked { tl.resolved_cel_screen_mesh(...); continue; }` で Scene を経由しない | ✔ | 初回レビュー（第4回） |
| I-4006 | 未解決 | Medium | 仕様適合性 | main.rs:733-743, 801-821 | 選択ツール・自由変形ツール・スポイトを使っているときは、非表示やロック中のレイヤーを選んでいても、未作成フレームを選んでいても、キャンバス左上の警告バナーが出ない。DOCUMENT.md 第4章①②は、非表示・ロック中のレイヤーや未作成のマス目を選んでいるときにバナーを出すと定めている | `current_state_notice` は `tool_allowed` が真なら `None` を返す。選択・自由変形の `tool_allowed` は `has_any_editable_layer()`（どれか1つでも編集可能なら真）、スポイトは常に真 | ✔ | 初回レビュー（第4回） |
| I-4007 | 未解決 | Medium | 堅牢性 | panels.rs:1573-1651、timeline_ui.rs:1765-1771 | 読み込み時にシンボル数と名前の長さに上限がなく、ライブラリ一覧は全シンボルの行（TextEdit 付き）を毎フレーム作り、名前も毎フレーム複製する（表示範囲だけ描く仕組みがない）。最小のシンボル定義は約90バイトなので、100MB 内に約100万件入り、ライブラリタブを開くと実質固まる。数十MB の名前のレイヤー・シンボルも毎フレーム文字配置の対象になる | `entries: Vec<(u64, String, usize)> = library.iter().map(\|s\| (s.id, s.name.clone(), ...)).collect()` を `ScrollArea::vertical().show` で全件描画。読込側 `SavedLayer::name`・`SavedSymbolDef::name` は無検査 | ✔（コード）／○（固まる程度は推測） | 初回レビュー（第4回） |
| I-4008 | 未解決 | Medium | テスト | tests.rs:4061, 4099, 6155, 6288, 7273 ほか | 読み込みの堅牢性テストは ID の衝突・循環参照・範囲外の小さな値・NaN に偏り、巨大な座標・件数・名前の長さ・メモリ量・保存サイズの上限を一つも試していない（I-4001〜I-4003・I-4007 を検出できない）。循環除去の「O(V+E)」テストは 60 シンボルで 200ms 未満かを見るだけで計算量は確かめていない。アプリ終了時の順次確認（`ViewportClose`）を通るテストもない | `test_dense_symbol_library_cycle_detection_runs_in_linear_time` は `n = 60` と `elapsed.as_millis() < 200` のみ。`ViewportClose` / `handle_viewport_close_request` は tests.rs に出現しない | ✔ | 初回レビュー（第4回） |
| I-4009 | 未解決 | Low | アーキテクチャ | panels.rs:1272-1277, 1433-1440、timeline_ui.rs:572-578, 595-601, 865, 994, 2020-2035、main.rs:411, 544, 589 | UI 層が `apply` を通らずに、連続操作のまとめ（`history.end_coalesce()`）、セル範囲選択、Floating の確定、ドラッグの中断を直接行っている。ARCHITECTURE.md 1章「文書を変える入口は apply だけ」と、`Action::EndCoalesce` でまとめを区切る方針に対して例外が散らばっている | `tab.history.end_coalesce()` の直接呼び出しが3ファイル計7か所、`set_active_cell_selection(None)` の直接呼び出しが2か所 | ✔ | 初回レビュー（第4回） |
| I-4010 | 未解決 | Low | アーキテクチャ | project_io.rs:64-405, 1269-1277, 1482-1577 | Layer 3 の `project_io` が、egui の Context・ウィンドウ操作（`ViewportCommand`）・入力フォーカス・ファイルダイアログ（rfd）・ウィンドウタイトル・確認モーダルの状態を扱っている。ARCHITECTURE.md 7章の層分けでは UI シェルは Layer 5（`main`）の責務 | `use eframe::egui` と `rfd::FileDialog` を使用。`egui::`/`rfd::` の参照が23か所 | ✔ | 初回レビュー（第4回） |
| I-4011 | 未解決 | Low | アーキテクチャ | timeline_ui.rs:100-111 | タイムラインのドラッグ状態が `dragging_layer: Option<usize>` と `grid_drag: TimelineGridDrag` の2つに分かれており、両方が同時に有効な状態を型で表現できてしまう。ARCHITECTURE.md 3章の「排他ステートマシン」の方針がキャンバスにしか適用されていない | 現状は押下時の分岐で排他になっているが、型では保証されない | ✔ | 初回レビュー（第4回） |
| I-4012 | 未解決 | Low | 仕様適合性 | timeline_ui.rs:2073、project_io.rs:773-785 | 再生ヘッドと選択レイヤーはファイルに保存される（ARCHITECTURE.md 3章）のに、フレーム移動・再生では未保存マーク（`*`）が付かず、閉じるときの確認も出ない。画面の状態と次に保存される内容がずれ、移動したフレーム位置は保存されずに失われる | `apply_switch_to_layer_and_frame` は Undo を積まず `current_commit_id` が変わらない。`SavedTimeline` は `current_frame` と `active_layer` を書き出す | ✔ | 初回レビュー（第4回） |
| I-4013 | 未解決 | Low | 仕様適合性 | panels.rs:1472, 1319 | ボタン名が DOCUMENT.md と違う。ライブラリタブは「選択図形をシンボル化 (F8)」（DOCUMENT.md は「シンボルに変換 (F8)」）、シンボル再生設定は「ループ再生」（DOCUMENT.md は「ループ」） | 文字列リテラルの比較 | ✔ | 初回レビュー（第4回） |
| I-4014 | 未解決 | Low | 仕様適合性 | timeline_ui.rs:809-866 | レイヤー名を変える手段がない。DOCUMENT.md に記載はないが、Adobe Animate ではレイヤー名のダブルクリックで名前を変えられる | レイヤー列のクリック処理は表示・ロック切替と選択・並べ替えドラッグのみ。名前編集の UI がない | ✔ | 初回レビュー（第4回） |
| I-4015 | 未解決 | Low | 仕様適合性 | timeline_ui.rs:1988-2008 | ループ再生が ON（初期値）のとき、`,` `.` のコマ送りが端で反対側へ回り込む（1フレーム目で `,` を押すと最終フレームへ飛ぶ）。DOCUMENT.md は「1コマずつ前後に移動」とだけあり、Adobe Animate も端で止まる | `(tl.current_frame as isize + delta).rem_euclid(total)` | ✔ | 初回レビュー（第4回） |
| I-4016 | 未解決 | Low | 仕様適合性 | timeline_ui.rs:135-166、main.rs:133 | タイムラインのスクロール位置はアプリ全体で1つだけ持ち、タブを切り替えるたびに 0 に戻る。表示設定はタブごとに独立して記憶するという DOCUMENT.md 第0章③の考え方と揃っていない | `ensure_timeline_context` がタブID の変化で `reset_for_timeline_switch`（`scroll_x = 0.0; scroll_y = 0.0`） | ✔ | 初回レビュー（第4回） |
| I-4017 | 未解決 | Low | 堅牢性 | project_io.rs:1299-1332 | 一時ファイルを作って rename で置き換える保存方式のため、元のファイルの権限・所有者・拡張属性が引き継がれない。ファイルには書き込めてもフォルダに書き込めない場所では保存できない | `File::create(&tmp_path)` → `rename(&tmp_path, &target_path)`。元ファイルのメタデータを複製する処理がない | ✔ | 初回レビュー（第4回） |
| I-4018 | 未解決 | Low | 堅牢性 | project_io.rs:946-980, 1288 | 文書のどこか1か所にでも NaN / Infinity が入ると、文書全体が保存できなくなる。不正な図形だけを除くなどの逃げ道がないため、幾何計算側の不具合1つが「保存不能」に直結する | `proj_doc.validate_finite()?` が `Err` を返すと書き出しを中止 | ✔ | 初回レビュー（第4回） |
| I-4019 | 未解決 | Low | 堅牢性 | project_io.rs:159-185 | macOS の `Cmd+Q`（アプリメニューの「終了」）が `close_requested` を経由せず、未保存確認なしで終了する可能性がある。コメントは `Cmd+Q` を処理すると書いているが、確かめるテストはない | `handle_viewport_close_request` は `i.viewport().close_requested()` だけを見る | ○（要実機確認） | 初回レビュー（第4回） |
| I-4020 | 未解決 | Low | パフォーマンス | timeline_ui.rs:1291-1300 | フレームグリッドの各行で、表示範囲外も含めたそのレイヤーの全キーフレームを毎フレーム走査している。1レイヤーあたり最大10万キーフレームまで読み込めるので、二分探索で表示範囲だけに絞れる | `for (kf_idx, kf) in layer.keyframes.iter().enumerate()` で範囲判定 | ✔ | 初回レビュー（第4回） |
| I-4021 | 未解決 | Low | 堅牢性 | project_io.rs:1160-1203（制限は symbol.rs:95） | 読み込み時にシンボルの入れ子の深さを確かめないため、9段以上の入れ子は通知なく描画されない。DOCUMENT.md 第5章③は「何段階でも入れ子」 | 30段の入れ子を作ったファイルで実測：表示される図形は8段分の8個だけ。`MAX_SYMBOL_NEST_DEPTH = 8` | ✔（実測） | 初回レビュー（第4回） |
| I-4022 | 未解決 | Low | テスト | tests.rs:9134-9215 | 「シンボルに入ったまま保存しても外で保存したものと同じファイル」のテストは、Floating がない状態で、シンボル内の再生ヘッドも動かしていない場合しか確かめていない | テストは塗り1回のあと保存するだけ | ✔ | 初回レビュー（第4回） |

## 3. 範囲外のメモ

| 場所 | メモ |
| :--- | :--- |
| vector_engine.rs:1118 付近（図形メッシュ生成） | 小さな三角形1つあたりのメッシュキャッシュが非常に大きい（10万図形でセルのメッシュ作成時に +840MB、頂点1つあたり約1.4KB）。確保した容量の切り詰めがないなどの可能性 |
| vector_engine.rs（平坦化） | 平坦化の頂点数に、画面上の大きさに応じた上限がない（I-4001 の直接の原因） |
| symbol.rs:95 | `MAX_SYMBOL_NEST_DEPTH = 8` は DOCUMENT.md 第5章③「何段階でも入れ子」と食い違う |
| vector_engine.rs:1208（`arc_mesh_to_shape`） | ARCHITECTURE.md 8章は「Arc::clone の O(1)」とするが、キャッシュを保持したまま `Arc::unwrap_or_clone` するため、描画のたびに頂点配列を丸ごと複製している（8章自身も注記している） |
| canvas_tool.rs:2891-2894 | `set_active_frame` は常に土台キャッシュを作り直す（I-4004 の原因箇所） |

## 4. 今回の範囲に関係するドキュメントの主張と裏付け

| 主張（出典） | コードでの確認 | テスト・計測での裏付け |
| :--- | :--- | :--- |
| 文書を変える入口は `apply` だけ（ARCHITECTURE 1・4章） | 一部不成立。UI 層から履歴のまとめ・セル選択・Floating 確定を直接変更（I-4009） | なし |
| 描画・当たり判定はすべて Scene を読む（ARCHITECTURE 4章） | 不成立。ロック中のレイヤーと再生中は文書から直接描く（I-4005） | なし（`test_stage4_scene_resolve_and_hit_testing_via_scene` は当たり判定のみ） |
| 再生ヘッド等の変更ではリビジョンを上げず、キャッシュを無駄に作り直さない（ARCHITECTURE 4章） | リビジョンは上げない。ただし土台キャッシュは毎コマ作り直す（I-4004） | リビジョンは `test_stage7_set_active_frame_and_per_layer_backdrop_cache`。作り直しの無駄は計測で確認 |
| 土台キャッシュは文書から作り直した結果とビット単位で一致（ARCHITECTURE 4章） | 成立（輪郭・穴・外接矩形をビット比較） | `test_stage7_scene_unchanged_after_rebuild_overlay_and_base_cache_on_every_action`（列挙した Action のみ） |
| `apply` は再生を止める（FPS 変更を除く）（ARCHITECTURE 4章） | 成立（doc_actions.rs:419-421） | 直接のテストなし |
| View は Undo で巻き戻らず、タブごとに独立（ARCHITECTURE 3章、DOCUMENT 第0章③） | 成立 | `test_untitled_tab_numbering_and_per_tab_wireframe_state`、`test_onion_skin_per_tab_isolation_and_shift_marquee_no_premature_merge` |
| 再生ヘッドと選択レイヤーはファイルに保存する（ARCHITECTURE 3章） | 成立。ただし未保存マークに反映されない（I-4012） | なし（往復テストは `current_frame` を確かめていない） |
| Undo は最大100ステップ（ARCHITECTURE 3・9章、DOCUMENT 第3章） | 成立 | `test_compact_json_serialization_nan_rejection_and_undo_history_cap` |
| 保存は文書のコピー上で Floating を確定し、前後でセッションと文書を変えない（ARCHITECTURE 5章、DOCUMENT 第0章①） | 成立 | `test_save_while_selected_flushes_merge_drawing_and_preserves_selection` |
| シンボルに入ったまま保存しても外で保存したものと同じファイル（ARCHITECTURE 6章、DOCUMENT 第0章①） | 成立と判断 | `test_stage6b_save_inside_two_level_nested_symbol_matches_save_after_exit_to_root`（条件が限定的、I-4022） |
| 5層の厳格一方向・逆参照ゼロ・層内循環ゼロ（ARCHITECTURE 7章） | `use crate::` の依存を全ファイル確認し、下位から上位への参照・層内循環はなし。責務の置き場所には問題あり（I-4010） | 自動チェックなし |
| ドラッグを単一 enum にして同時有効を型で排除（ARCHITECTURE 3章） | キャンバスは成立。タイムラインは2つに分かれる（I-4011） | `test_active_canvas_drag_state_machine_and_symbol_inplace_roundtrip_cache`（キャンバスのみ） |
| 100MB 上限で異常な巨大ファイルによる OOM を防ぐ（project_io.rs:26 のコメント） | 不成立 | 計測で反証（I-4001, I-4002） |
| 循環参照の除去は 3色DFS による O(V+E)（project_io.rs:1161 のコメント） | アルゴリズムとしては成立 | テストは n=60 で 200ms 未満を見るだけで計算量は未検証（I-4008） |
| u64::MAX 付近の細工IDでも衝突しない（project_io.rs:30） | 成立 | `test_u64_max_entity_ids_in_crafted_json_remapped_without_collision`、`test_property_geometry_strokes_booleans_and_random_json_invariants` |
| 一時ファイル経由のアトミック保存（project_io.rs:1279） | 成立（fsync → rename → フォルダの fsync） | 失敗時に壊さないことは `test_save_io_failure_rollback_and_marquee_slice_push_undo`。権限の引き継ぎは未対応（I-4017） |
| `Cmd+Z` で保存時点に戻ると `*` が消える（DOCUMENT 第0章①） | 成立（世代IDの復元） | `test_multitab_open_switch_symbol_copy_paste_and_close` |
| 未保存タブを閉じるときの確認（はい／いいえ／キャンセル）（DOCUMENT 第0章①） | タブ単位は成立。アプリ終了時の順次確認はテストなし、macOS の `Cmd+Q` は未確認（I-4019） | `test_unsaved_tab_close_modal_non_blocking_save_discard_and_cancel`、`test_raw_input_transform_handles_vertex_move_wheel_zoom_pan_and_save_on_close`（単一タブのみ） |
| 再生中は3段階メッシュキャッシュにより再テッセレーションなしで滑らか（DOCUMENT 第4章④） | 描画は成立。ただし土台キャッシュの再評価が毎コマ走る（I-4004） | `test_resolved_cel_screen_and_onion_mesh_cache_and_pan_invalidation`、`test_time_based_playback_advance_and_onion_skin_rendering` |
| 再生中は毎コマの FillShape クローンを行わない（timeline_ui.rs:2040 のコメント） | 不成立 | 計測で反証（I-4004） |
| Tier 3 キャッシュの取得は `Arc::clone` の O(1)（ARCHITECTURE 8章、panels の描画経路） | 取得は O(1) だが、egui に渡すたびに頂点配列を複製するので描画は O(頂点数) | なし |
| ズーム範囲 15%〜1600%（DOCUMENT 第2章③） | 成立 | `test_zoom_around_cursor_preserves_world_point_and_clamps_bounds` |
