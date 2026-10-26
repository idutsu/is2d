#![allow(dead_code)]

use crate::vector_engine::FillShape;

/// 1枚のセル画（ベクター図形群 `Vec<FillShape>`）
#[derive(Clone, Debug, Default)]
pub(crate) struct Cel {
    pub(crate) shapes: Vec<FillShape>,
}

impl Cel {
    pub(crate) fn new(shapes: Vec<FillShape>) -> Self {
        Self { shapes }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.shapes.is_empty()
    }
}

/// タイムライン上のキーフレーム（開始フレーム番号 0-indexed とセル画 `Cel`）
#[derive(Clone, Debug)]
pub(crate) struct Keyframe {
    pub(crate) start_frame: usize,
    pub(crate) cel: Cel,
}

impl Keyframe {
    pub(crate) fn new_blank(start_frame: usize) -> Self {
        Self {
            start_frame,
            cel: Cel::default(),
        }
    }
}

/// タイムラインの1レイヤー（名前、表示/非表示、ロック状態、開始フレーム順にソートされたキーフレーム配列）
#[derive(Clone, Debug)]
pub(crate) struct Layer {
    pub(crate) name: String,
    pub(crate) visible: bool,
    pub(crate) locked: bool,
    pub(crate) keyframes: Vec<Keyframe>,
}

impl Layer {
    pub(crate) fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            visible: true,
            locked: false,
            keyframes: vec![Keyframe::new_blank(0)],
        }
    }

    /// 指定フレーム番号 `frame` において有効な（保持されている）キーフレームのインデックスを返す
    pub(crate) fn keyframe_index_at(&self, frame: usize) -> Option<usize> {
        let idx = self.keyframes.partition_point(|kf| kf.start_frame <= frame);
        idx.checked_sub(1)
    }

    /// 指定フレーム番号 `frame` で表示される `Cel` への不変参照を返す
    pub(crate) fn cel_at(&self, frame: usize) -> Option<&Cel> {
        let idx = self.keyframe_index_at(frame)?;
        Some(&self.keyframes[idx].cel)
    }

    /// 指定フレーム番号 `frame` で編集対象となる `Cel` への可変参照を返す
    pub(crate) fn cel_at_mut(&mut self, frame: usize) -> Option<&mut Cel> {
        let idx = self.keyframe_index_at(frame)?;
        Some(&mut self.keyframes[idx].cel)
    }

    /// 指定フレーム `frame` にキーフレームを挿入する。
    /// - `clone_prev == true`（`F6` キーフレーム挿入）：直前のキーフレームの図形群を複製して新しいキーフレームを作る。
    /// - `clone_prev == false`（`F7` 空白キーフレーム挿入）：空の `Cel` を持つ新しいキーフレームを作る。
    pub(crate) fn insert_keyframe(&mut self, frame: usize, clone_prev: bool) -> usize {
        if let Some(existing_idx) = self
            .keyframes
            .iter()
            .position(|kf| kf.start_frame == frame)
        {
            if !clone_prev {
                self.keyframes[existing_idx].cel.shapes.clear();
            }
            return existing_idx;
        }

        let cloned_shapes = if clone_prev {
            self.cel_at(frame)
                .map(|c| c.shapes.clone())
                .unwrap_or_default()
        } else {
            Vec::new()
        };

        let insert_idx = self.keyframes.partition_point(|kf| kf.start_frame < frame);
        self.keyframes.insert(
            insert_idx,
            Keyframe {
                start_frame: frame,
                cel: Cel::new(cloned_shapes),
            },
        );
        insert_idx
    }
}

/// レイヤー配列・現在のフレーム・FPS・再生制御・オニオンスキン設定を管理するタイムライン構造体
#[derive(Clone, Debug)]
pub(crate) struct Timeline {
    pub(crate) layers: Vec<Layer>,
    pub(crate) current_frame: usize,
    pub(crate) active_layer: usize,
    pub(crate) total_frames: usize,
    pub(crate) fps: u32,
    pub(crate) is_playing: bool,
    pub(crate) onion_skin_enabled: bool,
    pub(crate) onion_prev_frames: usize,
    pub(crate) onion_next_frames: usize,
}

impl Default for Timeline {
    fn default() -> Self {
        Self {
            layers: vec![Layer::new("Layer 1")],
            current_frame: 0,
            active_layer: 0,
            total_frames: 24,
            fps: 24,
            is_playing: false,
            onion_skin_enabled: false,
            onion_prev_frames: 2,
            onion_next_frames: 2,
        }
    }
}

impl Timeline {
    /// 現在アクティブなレイヤー・フレームの `Cel` 内の図形スライスを返す
    pub(crate) fn active_cel_shapes(&self) -> &[FillShape] {
        self.layers
            .get(self.active_layer)
            .and_then(|layer| layer.cel_at(self.current_frame))
            .map(|cel| cel.shapes.as_slice())
            .unwrap_or(&[])
    }

    /// 現在アクティブなレイヤー・フレームの `Cel` 内の図形ベクタへの可変参照を返す
    pub(crate) fn active_cel_shapes_mut(&mut self) -> Option<&mut Vec<FillShape>> {
        let frame = self.current_frame;
        self.layers
            .get_mut(self.active_layer)
            .and_then(|layer| layer.cel_at_mut(frame))
            .map(|cel| &mut cel.shapes)
    }
}
