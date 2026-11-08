# i_overlay 9.1.0 の写し（is2d 用に1か所だけ足したもの）

- 元：crates.io の `i_overlay` 9.1.0（https://github.com/iShape-Rust/iOverlay 。ライセンスは MIT または Apache-2.0。`LICENSE-MIT`・`LICENSE-APACHE` は元のリポジトリのもの）
- 写したもの：`src`・`README.md`・`Cargo.toml`（テストとベンチの項目、dev-dependencies を外した）
- 足したもの：`src/core/edge_overlay.rs` の `EdgeOverlay::build_all_vectors`
  - 切ったあとのすべての区間を、左右の塗りと印（`OverlayEdgeData`）つきで返す。塗りの規則で区間をふるい落とさない
  - 中身は、i_overlay の中にもとからある `GraphBuilder::build_boolean_all` を呼ぶだけ
  - 使う所：is2d の `PlanarMap::overlay`（planar_map.rs）。ほぼ重なった2本の辺が格子で1本になった区間を落とさないため
- 新しい版にするときは、この1か所を足し直す
