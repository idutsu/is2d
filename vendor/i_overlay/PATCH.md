# i_overlay 9.1.0 の写し（is2d 用に2か所足したもの）

- 元：crates.io の `i_overlay` 9.1.0（https://github.com/iShape-Rust/iOverlay 。ライセンスは MIT または Apache-2.0。`LICENSE-MIT`・`LICENSE-APACHE` は元のリポジトリのもの）
- 写したもの：`src`・`README.md`・`Cargo.toml`（テストとベンチの項目、dev-dependencies を外した）
- 足したもの：`src/core/edge_overlay.rs` の `EdgeOverlay::build_all_vectors`
  - 切ったあとのすべての区間を、左右の塗りと印（`OverlayEdgeData`）つきで返す。塗りの規則で区間をふるい落とさない
  - 中身は、i_overlay の中にもとからある `GraphBuilder::build_boolean_all` を呼ぶだけ
  - 使う所：is2d の `PlanarMap::overlay`（planar_map.rs）。ほぼ重なった2本の辺が格子で1本になった区間を落とさないため
- 足したもの（2つ目）：`src/core/edge_data.rs` の `OverlayEdgeData::keep_when_count_cancels`（ふだんは `false`）と、それを見る `src/segm/merge.rs` の `merge`（2か所）
  - 重なった区間をまとめたとき、数（count）が打ち消し合って 0 になっても、印が `true` を返せば区間を捨てない
  - 使う所：is2d の印（`ProvId`）が `true` を返す。i_overlay は、交点で切った区間の向きが「小さい点から大きい点」と逆になると数の符号を変えるので、交点を格子に丸めて向きが変わった2本の区間が重なると、数が打ち消し合って区間ごと消えていた。偶奇の規則では、数の偶奇は打ち消しても足しても同じなので、左右の塗りは変わらない
- 新しい版にするときは、この2か所を足し直す
