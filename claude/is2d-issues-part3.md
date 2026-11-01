# is2d 課題リスト 第3回（Action・シンボル・クリップボード）

## 1. 対象と範囲

- 対象コミット：`c93b64744b2f970e89c301a61e8590d6122f7839`（`c93b647`）
- ビルド：`cargo build --release` 成功
- 最後まで読んだファイル（合計 4783 行）

| ファイル | 行数 |
|---|---|
| `src/doc_actions.rs` | 2412 |
| `src/symbol.rs` | 1401 |
| `src/symbol_ops.rs` | 412 |
| `src/clipboard_ops.rs` | 558 |

- 照らし合わせた章：ARCHITECTURE.md 1・3・4・5・6・9章、DOCUMENT.md 第3・4・5章（参照先として第1章「各操作でできたものの重なり順」も確認）
- 範囲外は呼び出し先の確認に必要な箇所だけ読んだ（`canvas_tool.rs` の `edit_layers`／書き戻し／`UndoHistory`、`timeline.rs` の `resize_total_frames`／`strip_transient_caches_with_prev`、`panels.rs`・`timeline_ui.rs`・`main.rs` の Coalesce 終了箇所）
- 裏付けのため、一時的なテスト関数（`zz_review3_probe_*`）を `src/tests.rs` に追加して `cargo test --release` で実行し、結果を記録したあと削除した（リポジトリには残していない）。下表で「実測」と書いたものはこの結果

## 2. 課題の表

| ID | 状態 | 重大度 | 観点 | 場所 | 内容 | 根拠 | 検証 | 発見元 |
|---|---|---|---|---|---|---|---|---|
| I-3001 | 未解決 | High | 堅牢性 | `symbol.rs:96-97, 171-173, 180-190, 200-214`（書き戻し側：`canvas_tool.rs:3396-3397`） | シンボルを含むセルの展開結果は 10,000 図形（`MAX_EXPANDED_SYMBOL_SHAPES`）で黙って打ち切られ、上限に達した後のインスタンスと生図形は土台キャッシュに入らない。`edit_layers` の書き戻しは土台キャッシュの図形列だけからセルを作り直す（`kf.cel.set_parts(non_symbol_shapes, next_instances)`）ため、そのレイヤーを少しでも編集すると、表示されていなかったインスタンスや生図形が文書から消える。画面にも出ないので利用者は気づかず、そのまま保存される。展開回数上限（`MAX_SYMBOL_EXPANSION_STEPS`）に達した場合と、ライブラリにないシンボルを参照するインスタンス（`continue`）も同じ経路で消える | `if out.len() >= MAX_EXPANDED_SYMBOL_SHAPES \|\| !ctx.has_remaining_steps() { break; }`。実測：100 図形のシンボルのインスタンス 120 個を1レイヤーに置き、離れた場所にブラシを1回引くと `instances 120 -> 100`。6,000 図形のシンボル2個＋前面の生図形1個では、ブラシ1回で前面の生図形が消えた（残ったのはブラシの図形だけ） | ✔（上限到達の2ケースは実測。ステップ上限と欠落シンボルは○） | 初回レビュー（第3回） |
| I-3002 | 未解決 | Medium | パフォーマンス | `doc_actions.rs:360-376`（→ `canvas_tool.rs:2700-2711` → `timeline.rs:1302-1319`） | Undo を1件積むたびに、ルートタイムライン全体を複製し、各キーフレームについて直前エントリの同じレイヤーのキーフレーム列を先頭から線形探索している。1件あたり O(Σ レイヤーごとのキーフレーム数²)。コマ打ちのアニメでは、ブラシ1回・矢印キー1回・F6 1回ごとに体感できる遅延が出る。ARCHITECTURE.md 9章の「未変更のキーフレームは O(1) でポインタ共有」は、1セルの共有としては正しいが、操作1回のコストとしては成り立っていない | `pl.keyframes.iter().find(\|pkf\| pkf.cel.revision == kf.cel.revision)` をすべての `kf` について実行。実測（キーフレームごとに図形1個、ブラシ1回の `apply` 時間）：1レイヤー×300KF 0.12ms、1×3,000KF 2.4ms、10×3,000KF 24.7ms、Undo 18.4ms | ✔ | 初回レビュー（第3回） |
| I-3003 | 未解決 | Medium | 仕様適合性 | `doc_actions.rs:1820-1866`（書き戻し側：`canvas_tool.rs:3237-3247`） | クラシックトゥイーンの途中のフレームでシンボル再生設定（ループ／1回再生の開始フレーム）を変えると、入力した値に「キーフレーム開始からの経過フレーム数」が勝手に加算される。トゥイーン途中の書き戻しは位相を保つために `first_frame += elapsed` を行うが、利用者が明示的に指定した値にもこれがかかる。あわせてトゥイーンがそのフレームで分割される。DOCUMENT.md 5章④の「開始フレームを指定できる」とずれる | `b.first_frame = clamped_first;` のあと書き戻しで `b.first_frame = b.first_frame.saturating_add(elapsed)`。実測：0F〜8F のトゥイーンの 3F で開始フレームを 1（0始まり）に設定すると、3F に新しいキーフレームができ、その `first_frame` は 4（パネル表示は 5F） | ✔ | 初回レビュー（第3回） |
| I-3004 | 未解決 | Medium | 仕様適合性 | `symbol.rs:1136-1175`（呼び出し：`symbol.rs:665-685`、`doc_actions.rs:1715-1739`） | ライブラリからシンボルを削除したときの分解は、各キーフレームの開始フレームで評価した1コマだけを図形にする。そのコマが空白のインスタンスは、図形も残らずインスタンスごと消える（例：最初のコマが空のエフェクトシンボル）。ほかにも、アニメーションするシンボルは保持区間の全体が静止画になり、中に入っている子シンボルのインスタンスも通常のグループへ平らにされて子のアニメーションも失われる。DOCUMENT.md 5章②「絵が消えてしまわないよう自動的に通常のグループ図形へ分解されてステージ上に残る」とずれる | `local_shapes` が空なら何も `out_shapes` に積まず、インスタンスも `out_instances` に残さない。`instantiate_shapes` が子のバインディングを外側のもので上書きし、直後に `s.symbol_instance = None`。実測：1F目が空・2F目に絵がある3フレームのシンボルを置き、2Fで絵が見えている状態で削除すると、ルートのセルは図形0・インスタンス0 | ✔ | 初回レビュー（第3回） |
| I-3005 | 未解決 | Medium | アーキテクチャ | `doc_actions.rs:2112-2128`（→ `timeline.rs:1191-1199`） | 総フレーム数のドラッグは、値が変わるたびに文書を直接書き換える。短くする方向に動かした瞬間に範囲外のキーフレームが削除されるため、同じドラッグの中で値を戻しても絵は戻らない（Undo 1回で戻るのみ）。ARCHITECTURE.md 3章の「ドラッグ中は文書を書き換えず、離したときに Action を1回だけ発行する」という方針から外れており、操作中のプレビューが破壊的になっている | 毎回 `tl.resize_total_frames(next_total)` を実行し、短縮時は `layer.keyframes.retain(\|kf\| kf.start_frame < next_total)` | ✔ | 初回レビュー（第3回） |
| I-3006 | 未解決 | Medium | アーキテクチャ | `doc_actions.rs:333-353, 379-395, 838-841`、`panels.rs:1100, 1271-1277, 1432-1439`、`timeline_ui.rs:572-603, 2023`、`main.rs:432-437, 543-545` | ARCHITECTURE.md 4章は「`apply` が連続操作をまとめる」としているが、`apply` が行うのは「同じキーなら積まない」ことだけで、まとめの終了は UI の各ウィジェットが `tab.history.last_coalesce_key` を直接読み書きして判断している。`apply` は別の操作が来ても `last_coalesce_key` を消さない。新しい連続操作を足すたびに、UI 側で終了条件を正しく書かないと、別々の操作が1つの Undo にまとまる不具合が起きやすい | `panels.rs` の描画関数内で `tab.history.end_coalesce()`、`main.rs` で `self.tab_mut().history.end_coalesce()` を直接呼ぶ。文書側で `last_coalesce_key` を変えるのは `push_undo_entry_from_doc`・pop_undo/pop_redo・`restore_state_snapshot`・`clear_selection` だけで、それ以外の終了はすべて UI 側の判断 | ✔ | 初回レビュー（第3回） |
| I-3007 | 未解決 | Medium | アーキテクチャ | `doc_actions.rs:702, 1711-1712, 1731, 2107-2108, 2140-2145`、`clipboard_ops.rs:521-529`、`symbol_ops.rs:290-296, 384-389`、`canvas_tool.rs:2640-2654` | ARCHITECTURE.md 4章の「文書を書き換える経路は4つに限る」が守られていない。シンボルの登録・名前変更・削除、ペースト時のシンボル移植、ステージサイズ、FPS、シンボル編集への出入り（`fps`・`total_frames`・`next_layer_*` の補正）が、それぞれ文書を直接書き換えている。Undo のスナップショットは「`doc.revision` と各シンボルの `revision` が同じなら前回の `Arc` を使い回す」設計のため、直接書き換えでリビジョンを上げ忘れると、Undo 履歴に古いライブラリが記録される。現に FPS 変更と編集への出入りはシンボルのリビジョンを上げない（例：シンボル内で FPS を変えたあとステージサイズを変えて Undo すると、そのシンボルの FPS だけが古い値に戻る） | `tab.doc.root.fps = next_fps; ... sym.timeline.fps = next_fps;`（リビジョン更新なし）、`sym.timeline.fps = fps;`（enter/exit）、`snapshot_symbols` の再利用条件に `fps` が含まれない | ✔（例の Undo 挙動は○） | 初回レビュー（第3回） |
| I-3008 | 未解決 | Low | 堅牢性 | `doc_actions.rs:1343, 1652-1653, 1759-1780, 1842-1846`、`symbol_ops.rs:33, 109-111` | 変更があるか確かめる前に Undo を積む箇所があり、何も変わらない Undo 1手と未保存マークが残る（ロックされたレイヤーの図形への変形、ペースト、トゥイーン切替、再生設定でシンボルが見つからない場合、シンボル変換で対象が0件だった場合） | 例：`push_undo(tab, coalesce); if tab.doc.find_symbol(sym_id).is_none() { return false; }` | ✔ | 初回レビュー（第3回） |
| I-3009 | 未解決 | Low | アーキテクチャ | `doc_actions.rs:379-395` | `push_canvas_undo` と `push_undo` の中身が完全に同じで、使い分けに意味がない | 2関数の本体が同一 | ✔ | 初回レビュー（第3回） |
| I-3010 | 未解決 | Low | パフォーマンス | `doc_actions.rs:1861-1864` | シンボル再生設定を1回変えるたびに土台キャッシュ全体を捨てて全レイヤーを作り直す。開始フレームのドラッグ中は値が変わるたびに走る。差分更新で足りない理由が見当たらない | `tab.session.base_cache = Default::default(); tab.refresh_scene_base();` | ✔（影響の大きさは○） | 初回レビュー（第3回） |
| I-3011 | 未解決 | Low | パフォーマンス | `symbol.rs:651-662` | シンボルの名前変更でシンボルの `revision` を上げるため、名前は描画に関係ないのに、参照するすべてのセルの解決済みメッシュと土台キャッシュが1文字ごとに作り直される。`revision` が「描画内容の変更」と「Undo 用の変更検出」を兼ねていることが原因 | `sym.revision = next_mesh_revision();`。実測：100 図形のシンボル×95 インスタンスで1文字あたり 7.9ms（土台キャッシュ再構築まで。描画側の再構築は含まない） | ✔ | 初回レビュー（第3回） |
| I-3012 | 未解決 | Low | パフォーマンス | `symbol.rs:680` | シンボル削除時、削除したシンボルを参照していないものも含めて全シンボルの `revision` を上げるため、全セルのキャッシュが無効になる | ループ内で無条件に `sym_def.revision = next_mesh_revision()` | ✔ | 初回レビュー（第3回） |
| I-3013 | 未解決 | Low | パフォーマンス | `symbol.rs:754-765, 316-346, 1102-1108` | `matches_layer_keyframe` のコメントは「O(参照シンボル数)」だが、参照ごとにライブラリを線形探索するので O(参照数×ライブラリ数)。`has_dynamic_animation` もキャッシュ確認の前に毎回ライブラリ全体の署名を計算し、これが描画のたびにセル内のインスタンスごとに呼ばれる | `library.iter().any(\|s\| s.id == sym_id && s.revision == rev)`、`let lib_sig = library.iter().fold(...)` | ✔ | 初回レビュー（第3回） |
| I-3014 | 未解決 | Low | 堅牢性 | `symbol.rs:826-855, 863-873` | 参照シンボルの収集は `visited` をセル全体で共有し、深さ8で打ち切る。同じシンボルが先に深い位置で見つかると、浅い位置で再度出てきても子を辿らないため、深い入れ子でキャッシュが古いまま使われる可能性がある | `if depth >= MAX_SYMBOL_NEST_DEPTH \|\| visited.contains(&sym_id) { return; }` | ○ | 初回レビュー（第3回） |
| I-3015 | 未解決 | Low | パフォーマンス | `doc_actions.rs:917-948, 1003` | バケツ塗りは1クリックごとに、表示中の全レイヤーの図形を複製し、ロックされたレイヤーは土台キャッシュにあるのに文書から評価し直している | `out.extend(eval)`／`tab.scene().iter()...cloned()` | ✔ | 初回レビュー（第3回） |
| I-3016 | 未解決 | Low | パフォーマンス | `clipboard_ops.rs:382-391, 435-443` | シンボル編集中のペーストでは、循環参照の判定を、展開済みの図形1個ごとに（インスタンスごとではなく）行う。判定はライブラリの DFS を含み、さらに `would_clipboard_paste_all_cycle` と `resolve_clipboard_for_paste` で2回行う | `clipboard.shapes.iter().all(...)` と `for shape in &clipboard.shapes { ... can_paste_clipboard_symbol(...) }` | ✔（体感への影響は○） | 初回レビュー（第3回） |
| I-3017 | 未解決 | Low | アーキテクチャ | `symbol_ops.rs:35`、`doc_actions.rs:698-711`、`symbol.rs:572-577` | シンボル変換は、インスタンスに付ける ID を `next_symbol_id` から先に決め、登録時は `register_new_symbol` が衝突回避込みで別に決める。2つが一致する保証がなく、一致を確かめていない（現状は ID が単調増加なので一致する） | `let sym_id = tab.doc.next_symbol_id;` と `let registered_id = tab.doc.register_new_symbol(local_shapes);` | ✔ | 初回レビュー（第3回） |
| I-3018 | 未解決 | Low | 堅牢性 | `clipboard_ops.rs:83-105, 124-139` | ペースト時に参照先シンボルが見つからないインスタンスは、新しい gid のバインディングを付けたまま図形として入る。I-3001 の経路で、次の編集のときに消える | `if let Some(sym) = tab.doc.find_symbol(...) { ... continue; }` のあと通常図形の経路で `binding.instance_gid = mapped_inst_gid` | ○ | 初回レビュー（第3回） |
| I-3019 | 未解決 | Low | 仕様適合性 | `clipboard_ops.rs:255-267, 289-305` | 別ドキュメントへ一度移植したシンボルは `imported_map` で記憶され、コピー元でそのシンボルを修正してから再度コピー＆ペーストしても古い移植版が使われる。DOCUMENT.md に記載がなく、Animate のような競合確認もない | 同じコピー元ドキュメントからのコピーでは `imported_map` を消さない | ○ | 初回レビュー（第3回） |
| I-3020 | 未解決 | Low | 仕様適合性 | `symbol.rs:578-586` | 新しいシンボルの名前はシンボル ID の番号で付けるため、別ドキュメントからの移植などで ID が進んだあとは番号が飛ぶ（例：移植で ID 2〜4 が使われたあとに F8 で作ると「Symbol 5」）。DOCUMENT.md 5章②は「Symbol 1, Symbol 2…」 | `let mut name_num = sym_id;` | ✔ | 初回レビュー（第3回） |
| I-3021 | 未解決 | Low | アーキテクチャ | `doc_actions.rs:2130-2149`、`symbol_ops.rs:290-293, 384-389` | FPS がルートと各シンボルのタイムラインに重複して保存され、シンボルへの出入りのたびに上書きして揃えている。また `apply_set_fps` の `commit_floating` は「`floating.is_none()` のときだけ呼ぶ」条件になっており、決して何もしない | `&& tab.session.floating.is_none() { commit_floating(tab); }` | ✔ | 初回レビュー（第3回） |
| I-3022 | 未解決 | Low | 仕様適合性 | `doc_actions.rs:793-796, 2021-2026` | レイヤー数が上限のとき、レイヤー追加は何もせず通知も出さないが、`Applied(true)` を返す | `if ... >= MAX_LAYERS_PER_TIMELINE { return; }` のあと `ApplyResult::Applied(true)` | ✔ | 初回レビュー（第3回） |
| I-3023 | 未解決 | Low | 堅牢性 | `doc_actions.rs:448-450, 863-914` | `CommitRawOutline` は現在のツールで分岐し、ブラシ以外ならすべて消しゴムとして扱う。ツールが切り替わった直後に届くと、描いたつもりの線で消去が起きうる | `if active_tool == Tool::Brush { ... } else { /* 消去 */ }` | ○ | 初回レビュー（第3回） |
| I-3024 | 未解決 | Low | アーキテクチャ | `doc_actions.rs:127-132, 458-465` | `ToggleSelectionAt`・`RetainSelection` は `Scene` の配列番号を運ぶ。ARCHITECTURE.md 2章の「選択は ID で持ち、配列の番号では持たない」という方針と合わず、Action を作ってから適用するまでに `Scene` が変わると別の図形を指す | `ToggleSelectionAt { scene_idx: usize }` | ✔ | 初回レビュー（第3回） |
| I-3025 | 未解決 | Low | 堅牢性 | `symbol.rs:688-722` | Undo/Redo 後の `EditPath` 検証は、インスタンスが親タイムラインの「どこかのキーフレーム」にあれば有効とみなし、入る前のフレームに存在するかは確かめない | `l.keyframes.iter().any(...)` | ○ | 初回レビュー（第3回） |
| I-3026 | 未解決 | Low | テスト | `tests.rs:10117`（`test_stage7_scene_unchanged_after_rebuild_overlay_and_base_cache_on_every_action`） | ARCHITECTURE.md 4章は「各 Action のあとに作り直しても Scene が変わらないことをテストで確認している」とするが、テストが流す Action は約 70 種のうち約 22 種。`SetSymbolPlayback`・`MoveOrCopyFrames`・`DeleteSymbol`・`ReshapeEdge`・`Cut`・`CommitTransformDrag` などは対象外 | テスト本文の `Action::` の出現を集計 | ✔ | 初回レビュー（第3回） |
| I-3027 | 未解決 | Low | テスト | `tests.rs:6731`（`test_nested_symbol_expansion_memoization_and_step_cap`） | 展開上限のテストは「時間内に終わる」ことだけを確かめ、上限に達したときに文書のインスタンスや図形が失われないことは確かめていない（I-3001 を見逃している） | 2つ目のケースは `elapsed < 500ms` と `first_non_empty.is_empty()` のみを検査 | ✔ | 初回レビュー（第3回） |
| I-3028 | 未解決 | Low | テスト | `tests.rs:10554`（`test_collapsed_symbol_instance_blocks_enter_and_timeline_mut_bumps_revision`） | ARCHITECTURE.md 6章「変換に有限でない値を含むインスタンスには入れない」のうち、有限でない値のケースはテストされていない（行列式0のケースのみ） | `Affine::scale_non_uniform(1.0, 0.0)` のみ | ✔ | 初回レビュー（第3回） |

## 3. 範囲外のメモ

| 場所 | 一言 |
|---|---|
| `canvas_tool.rs:3302-3397`（`writeback_edited_layer_shapes`） | セルを土台キャッシュの図形列だけから作り直す。I-3001 の根本原因 |
| `canvas_tool.rs:3236-3247` | トゥイーン途中の書き戻しで常に `first_frame += elapsed`。I-3003 の原因。明示的な値の指定と区別していない |
| `timeline.rs:1302-1319`（`strip_transient_caches_with_prev`） | キーフレームごとの線形探索で O(K²)。I-3002 の原因 |
| `timeline.rs:1191-1199`（`resize_total_frames`） | 短縮時に範囲外のキーフレームを即時削除。I-3005 |
| `panels.rs:1271-1277, 1432-1439` | 描画関数の中で Undo 履歴（`tab.history`）を直接書き換えている |
| `canvas_tool.rs:4014-4021`（`clear_selection`） | 選択解除が Coalesce の終了を兼ねている（I-3006 の終了条件が散らばっている一例） |

## 4. ドキュメントの主張と裏付け

| 主張 | 出典 | 裏付け | 判定 |
|---|---|---|---|
| 文書を変える入口は `apply` だけ | ARCH 1章・4章 | なし。UI が `tab.history` を、Action 処理が文書を直接書き換える箇所がある（I-3006、I-3007） | ✗ |
| 文書を書き換える経路は4つに限る | ARCH 4章 | なし（I-3007） | ✗ |
| `apply` が連続操作をまとめる（`CoalesceKey`） | ARCH 4章 | `test_symbol_rename_consecutive_keystrokes_grouped_into_single_undo`、`test_stage01_undo_redo_coalesce_and_save_load_on_cel` でまとまることは確認。終了は UI 依存（I-3006） | △ |
| `apply` は再生を止める（FPS を除く） | ARCH 4章 | コードで確認（`doc_actions.rs:419-421`）。専用テストは見当たらない | △ |
| `with_active_timeline_mut` は必ずリビジョンを上げる | ARCH 4章 | `test_collapsed_symbol_instance_blocks_enter_and_timeline_mut_bumps_revision` | ✔ |
| `set_active_frame` 等はリビジョンを上げない | ARCH 4章 | `test_stage7_set_active_frame_and_per_layer_backdrop_cache` | ✔ |
| 土台キャッシュは文書から作り直した結果とビット単位で一致（各 Action のあと） | ARCH 4章 | `test_stage7_scene_unchanged_after_rebuild_overlay_and_base_cache_on_every_action`。ただし約22/70種の Action のみ（I-3026）。展開上限に達したセルでは文書と一致しない（I-3001） | △ |
| `Floating` は確定するまで文書を変えない | ARCH 1章・5章 | `test_stage7b_floating_holds_pieces_without_mutating_cels_until_commit` | ✔ |
| スライスを何もせず解除すると数値誤差なく元に戻る | ARCH 5章 | `test_stage01_move_deselect_merge_and_marquee_slice_restore_on_cel`、`test_marquee_select_partial_slice_and_shift_multi_select` | ✔ |
| 確定は Undo の1手にせず、未保存マークも変えない | ARCH 5章 | `test_save_while_selected_flushes_merge_drawing_and_preserves_selection`（`is_dirty` を確認）、`test_stage7b_...`（同） | ✔ |
| Undo/Redo 後、残っている図形だけ選択を保つ | ARCH 5章、DOC 3章 | `test_stage3_selection_preserved_across_undo_redo_frame_scrub_and_reexpansion` | ✔ |
| 触っていない図形はシンボルへの出入りや無関係な編集で劣化しない | ARCH 6章 | `test_stage6b_non_uniform_scale_rotated_symbol_edit_preserves_untouched_shape_bits` | ✔ |
| シンボルの中に入ったまま保存しても、外に出てから保存したものと同じ | ARCH 6章 | `test_stage6b_save_inside_two_level_nested_symbol_matches_save_after_exit_to_root` | ✔ |
| Undo/Redo 後に `EditPath` を検証し、辿れる階層まで戻る | ARCH 6章 | `test_stage6b_undo_redo_across_before_inside_and_after_symbol_restores_doc_and_edit_path`。フレーム単位の存在は見ていない（I-3025） | △ |
| 行列式ほぼ0・有限でない変換のインスタンスには入れない | ARCH 6章 | 行列式0のみテストあり。有限でない値はテストなし（コードには判定あり） | △ |
| Undo 最大100ステップ | ARCH 3章・9章、DOC 3章 | `test_compact_json_serialization_nan_rejection_and_undo_history_cap` | ✔ |
| 未変更のキーフレームやシンボルは O(1) でポインタ共有 | ARCH 9章 | 共有自体は `test_blank_frame_symbol_in_parent_group_moves_rotates_and_deletes_synchronously`（`Arc::ptr_eq` でライブラリとセル図形の共有を確認）。ただし1回の記録は O(ΣK²)（実測、I-3002） | ✗ |
| 履歴保存時に再計算可能なメッシュキャッシュを除去する | ARCH 9章 | `test_normalized_cel_instances_empty_symbol_revival_and_undo_cache_stripping` | ✔ |
| 通常ペーストは画面上 +16px ずつ階段状にずれる | DOC 3章 | `test_transform_rotate_zone_only_near_four_corners_and_paste_staircase_across_zoom` | ✔ |
| シンボルを削除しても絵が消えない（グループとして残る） | DOC 5章② | `test_symbol_rename_library_delete_detach_and_inplace_child_symbol_creation`、`test_symbol_delete_mid_tween_preserves_start_keyframe_pose`。開始コマが空白・アニメーションするシンボルは未検証で、実際に消える（I-3004） | ✗ |
| 入れ子展開はメモ化と上限で爆発しない | symbol.rs コメント | `test_nested_symbol_expansion_memoization_and_step_cap`（時間のみ）。上限到達時のデータ保持は未検証で、実際に失われる（I-3001、I-3027） | △ |
| `ResolvedCelSig` の照合は O(参照シンボル数) | symbol.rs:764 コメント | なし。実際は O(参照数×ライブラリ数)（I-3013） | ✗ |
| 循環参照の検出は線形時間 | clipboard_ops / symbol.rs | `test_dense_symbol_library_cycle_detection_runs_in_linear_time` | ✔ |

（判定：✔ 裏付けあり、△ 一部のみ、✗ 裏付けなし・主張が成り立たない）

## 5. 集計

- High：1件（I-3001）
- Medium：6件（I-3002〜I-3007）
- Low：21件（I-3008〜I-3028）
