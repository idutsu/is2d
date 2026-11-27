//! 保存の形（作品ファイル `.is2d` と復旧ファイル `.is2dj`）：ファイルの頭と置き換え、保存の型、流して読む読み手（Layer 2。
//! タブ・ダイアログ・知らせは `project_io`。設計審査 01e4283 の D-3：保存の形を `project_io` から分けた）

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use eframe::egui::Vec2;
use kurbo::{Affine, Point};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::chunked_list::{Chunk, ChunkedList, Keyed};
use crate::sound::SoundLibrary;
use crate::stored_map::{ReadTiles, SavedStroke, SavedTile, TileReader, WriteTiles};
use crate::symbol::Slot;
use crate::symbol::{
    ColorEffect, Document, SymbolDef, SymbolInstance, SymbolInstanceBinding, SymbolLoopMode,
    TweenRotate,
};
use crate::timeline::MAX_LAYERS_PER_TIMELINE;
use crate::timeline::{
    Cel, Keyframe, KeyframeList, Layer, SavedScopeMap, ScopeMap, Timeline, TimelineCellSelection,
};

/// 保存の形の版（作品ファイルと復旧ファイルの両方。版はこれ1つ：設計審査 01e4283 の D-3）。ファイルは、頭（種類の印と版）の
/// あとに記録の列（`write_record`：記録ごとに、中身の長さ・中身の CRC32・種類・番号の枠と、postcard で書いた中身）。作品
/// ファイルは全部を1回で書き、復旧ファイルは変わった所を書き足す（書き手 `RecordWriter`・読み手 `read_file` は1つずつ）。
/// 版の歴史：版 6 で保存の形をバイナリに（設計：claude/保存の形をバイナリにする設計_版6_7b5f6c4.md）、版 7 で地図を塊のまま
/// 書く（文書に1つの塊の表と、階層ごとの地図の塊の番号の並び）、版 8 で音、版 9 で辺に線、版 10 で線の番号（設計：
/// claude/線の番号を辺にしまう設計_81b715e.md）、版 11 でレイヤーの番号の数え札を作品に1つに（設計：claude/審査b7208d9の直しの設計.md の
/// 7.）、版 12 で作品ファイルも復旧ファイルと同じ記録の列にし、版を1つにした（前は作品ファイルは頭＋中身1つで、復旧ファイル
/// は記録の版を別に持った）。前の版は開かない（テスト用の変換で直す：`convert_work_files_to_current_format`）。保存の型の形を
/// 変えたら、版を上げ、前の版の変換を足し、指紋 `FORMAT_FINGERPRINT` を書き替える（テスト `the_format_fingerprint_matches_the_version`
/// が、形が変わったのに版が同じなら止める）。並びの位置で読むので、書かない項目（`skip_serializing_if`）は作らない
pub(crate) const FORMAT_VERSION: u32 = 12;

/// 今の版の保存の形の指紋：見本の作品（`testdata/format_sample.json`。保存の型を名前で書いた形で、全部の項目と種類を使う。
/// テスト `write_format_sample` で作る）を、今の書き手で書いた作品ファイルと復旧ファイルの CRC32。テスト
/// `the_format_fingerprint_matches_the_version` が確かめる：保存の型の形（項目・型・並び・種類の並び）を変えると止まるので、
/// 版を上げ、前の版の変換を足し、見本と指紋を作り直す（版の上げ忘れを止める：設計審査 01e4283 の D-3）
#[cfg(test)]
pub(crate) const FORMAT_FINGERPRINT: u32 = 0x49fa_9075;

/// `.is2d` ファイルの先頭の印
const PROJECT_FILE_MAGIC: [u8; 4] = *b"IS2D";

/// 復旧ファイル（`.is2dj`：`recovery_journal`）の先頭の印
const JOURNAL_FILE_MAGIC: [u8; 4] = *b"IS2J";

/// 保存するファイルの種類（頭の初めの種類の印で見分ける：作るのは `file_identity`、確かめるのは `read_identity` だけ）
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum SaveKind {
    /// 作品ファイル（`.is2d`）
    Project,
    /// 復旧ファイル（`.is2dj`）
    Journal,
}

/// 頭（種類の印 4 バイト・版 u32）の長さ
pub(crate) const IDENTITY_LEN: usize = 8;

/// 種類 `kind` のファイルの頭（種類の印と版）。作るのはここだけ（作品ファイルと復旧ファイルの両方）
pub(crate) fn file_identity(kind: SaveKind) -> [u8; IDENTITY_LEN] {
    let magic = match kind {
        SaveKind::Project => PROJECT_FILE_MAGIC,
        SaveKind::Journal => JOURNAL_FILE_MAGIC,
    };
    let mut head = [0u8; IDENTITY_LEN];
    head[..4].copy_from_slice(&magic);
    head[4..].copy_from_slice(&FORMAT_VERSION.to_le_bytes());
    head
}

/// ファイルの頭 `head`（初めから読めた分。短くてよい）の種類と版を確かめる（確かめるのはここだけ）。種類の印が無ければ、
/// 前の形式（JSON）か is2d でないと断り、版が今のアプリと違えば断る
pub(crate) fn read_identity(head: &[u8]) -> Result<SaveKind, String> {
    let not_is2d = || {
        if head.first() == Some(&b'{') {
            "前の形式（JSON）のファイルは開けません".to_owned()
        } else {
            "is2d のファイルではありません".to_owned()
        }
    };
    let kind = match head.get(..4) {
        Some(m) if m == PROJECT_FILE_MAGIC => SaveKind::Project,
        Some(m) if m == JOURNAL_FILE_MAGIC => SaveKind::Journal,
        _ => return Err(not_is2d()),
    };
    if head.len() < IDENTITY_LEN {
        return Err(not_is2d());
    }
    let version = u32::from_le_bytes(head[4..8].try_into().expect("4 バイト"));
    if version != FORMAT_VERSION {
        return Err(match kind {
            SaveKind::Project => format!("未対応のプロジェクトバージョンです (version: {version})"),
            SaveKind::Journal => format!("未対応の復旧ファイルの版です (version: {version})"),
        });
    }
    Ok(kind)
}

/// ファイル `path` の頭の初めを読んで、種類と版を確かめる（`read_identity`）
pub(crate) fn file_kind(path: &Path) -> Result<SaveKind, String> {
    use std::io::Read;
    let mut head = Vec::with_capacity(IDENTITY_LEN);
    std::fs::File::open(path)
        .and_then(|f| f.take(IDENTITY_LEN as u64).read_to_end(&mut head))
        .map_err(read_failed)?;
    read_identity(&head)
}

/// `target` に書くときの一時ファイル（同じフォルダの `.{名前}.{プロセス番号}.tmp`。名前の付け方はここだけ：作品の保存・
/// 復旧ファイル・動画の書き出し）
pub(crate) fn tmp_path_for(target: &Path) -> PathBuf {
    target.with_file_name(format!(".{}.{}.tmp", file_name_or(target, "file"), std::process::id()))
}

/// 書き終えた一時ファイル `tmp` を `target` に置き換える（置き換えるのはここだけ）：一時ファイルを確定し、名前を替え、
/// フォルダを確定する（電源が落ちても、置き換える前か後のどちらかが残る）。失敗したら一時ファイルを消す
pub(crate) fn replace_atomically(tmp: &Path, target: &Path) -> std::io::Result<()> {
    // 確定は書ける開き方で開いて行う（Windows は、読むだけの開き方ではファイルを確定できない）
    let res = std::fs::OpenOptions::new()
        .write(true)
        .open(tmp)
        .and_then(|f| f.sync_all())
        .and_then(|()| std::fs::rename(tmp, target));
    if res.is_err() {
        let _ = std::fs::remove_file(tmp);
        return res;
    }
    if let Some(parent) = target.parent() {
        if let Ok(dir_file) = std::fs::File::open(parent) {
            let _ = dir_file.sync_all();
        }
    }
    Ok(())
}

/// `target` を、一時ファイル（`tmp_path_for`）に `write` で書いてから置き換える（`replace_atomically`）。書くのに失敗
/// したら一時ファイルを消す
pub(crate) fn write_atomically(
    target: &Path,
    write: impl FnOnce(std::fs::File) -> std::io::Result<std::fs::File>,
) -> std::io::Result<()> {
    let tmp = tmp_path_for(target);
    if let Err(e) = std::fs::File::create(&tmp).and_then(write) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    replace_atomically(&tmp, target)
}

/// 読み込み時に許容するエンティティID（`layer_id` / `symbol_id` / `group_id` / `instance_gid`）の最大安全値
/// （`u64::MAX` 付近の細工IDによる `saturating_add` 衝突を防止するため、上限を超えるIDは読み込み時に安全な連番へ再採番する）
pub(crate) const MAX_SAFE_ENTITY_ID: u64 = u64::MAX / 2;

// ============================================================================
// 1. 保存用シリアライズデータ構造（DTO）
// ============================================================================

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub(crate) struct SavedSymbolInstanceBinding {
    pub(crate) instance_gid: u64,
    pub(crate) symbol_id: u64,
    pub(crate) xform: [f64; 6],
    #[serde(default)]
    pub(crate) pivot: [f64; 2],
    #[serde(default)]
    pub(crate) loop_mode: SymbolLoopMode,
    #[serde(default)]
    pub(crate) first_frame: usize,
    #[serde(default)]
    pub(crate) color_effect: ColorEffect,
}

impl From<SymbolInstanceBinding> for SavedSymbolInstanceBinding {
    fn from(b: SymbolInstanceBinding) -> Self {
        Self {
            instance_gid: b.instance_gid,
            symbol_id: b.symbol_id,
            xform: b.xform.as_coeffs(),
            pivot: [b.pivot.x, b.pivot.y],
            loop_mode: b.loop_mode,
            first_frame: b.first_frame,
            color_effect: b.color_effect,
        }
    }
}

impl From<SavedSymbolInstanceBinding> for SymbolInstanceBinding {
    fn from(s: SavedSymbolInstanceBinding) -> Self {
        // 書いてある数をそのまま読む。潰れた変換も読む（そのインスタンスは描かない：`instantiate_shapes`）。数が有限かは
        // 読み込みの初めに、書く入口と同じ判定で確かめる（`only_finite_numbers`）。色効果を描くとき（`ColorEffect::apply`）は
        // 色の範囲に収めるので、範囲の外の色効果も描ける
        let xform = Affine::new(s.xform);
        Self {
            instance_gid: s.instance_gid,
            symbol_id: s.symbol_id,
            xform,
            pivot: Point::new(s.pivot[0], s.pivot[1]),
            loop_mode: s.loop_mode,
            first_frame: s.first_frame,
            color_effect: s.color_effect,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct SavedSymbolInstance {
    pub(crate) binding: SavedSymbolInstanceBinding,
    #[serde(default)]
    pub(crate) layer_id: u64,
    #[serde(default)]
    pub(crate) parent_scope: Vec<u64>,
    /// 重なり位置：セル画の階層の地図の並びの何番目の手前か
    #[serde(default)]
    pub(crate) order: usize,
}

impl From<&SymbolInstance> for SavedSymbolInstance {
    fn from(inst: &SymbolInstance) -> Self {
        Self {
            binding: inst.binding.into(),
            layer_id: inst.layer_id,
            parent_scope: inst.parent_scope.clone(),
            order: inst.order,
        }
    }
}

impl From<SavedSymbolInstance> for SymbolInstance {
    fn from(s: SavedSymbolInstance) -> Self {
        Self {
            binding: s.binding.into(),
            layer_id: s.layer_id,
            parent_scope: s.parent_scope,
            order: s.order,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct SavedKeyframe {
    pub(crate) start_frame: usize,
    #[serde(default)]
    pub(crate) tween: bool,
    #[serde(default)]
    pub(crate) tween_rotate: TweenRotate,
    /// 階層ごとの地図（図形の並びで、その階層が最初に出てくる順）
    #[serde(default)]
    pub(crate) maps: Vec<SavedScopeMap>,
    #[serde(default)]
    pub(crate) instances: Vec<SavedSymbolInstance>,
    /// 付けた音の区間（版 8 から）
    #[serde(default)]
    pub(crate) sound: Option<SavedSoundClip>,
}

/// キーフレームの音の区間の保存の形（`SoundClip`：音の番号と、音の中の始まり・終わり。`SOUND_RATE` の単位）
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub(crate) struct SavedSoundClip {
    pub(crate) sound: u64,
    pub(crate) start: u64,
    pub(crate) end: u64,
}

/// 音のライブラリの1つの音の保存の形：番号・名前・読み込んだ WAV・MP3 のバイトのまま（書くときは文書の音のバイトを
/// 共有して書き、写さない）
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct SavedSound {
    pub(crate) id: u64,
    pub(crate) name: String,
    pub(crate) bytes: Arc<[u8]>,
}

impl From<&crate::sound::Sound> for SavedSound {
    fn from(s: &crate::sound::Sound) -> Self {
        Self {
            id: s.id,
            name: s.name.clone(),
            bytes: s.encoded.clone(),
        }
    }
}

impl SavedKeyframe {
    /// 始まりのコマが `start_frame` のキーフレーム `kf` の保存用の写し
    pub(crate) fn new(start_frame: usize, kf: &Keyframe) -> Self {
        Self {
            start_frame,
            tween: kf.tween,
            tween_rotate: kf.tween_rotate,
            maps: kf.cel.maps().iter().map(ScopeMap::saved_form).collect(),
            instances: kf
                .cel
                .instances()
                .iter()
                .map(SavedSymbolInstance::from)
                .collect(),
            sound: kf.sound.map(|c| SavedSoundClip {
                sound: c.sound,
                start: c.start,
                end: c.end,
            }),
        }
    }
}

impl Keyframe {
    /// 読んだキーフレーム `s`。地図は塊の表から作ったしまう形（`into_document`）で、正しいかは確かめずに、初めて使うときに
    /// 確かめる（`Cel::from_saved`）。しまう形が無いか、同じ階層の地図が2つあれば、壊れたファイルとして `Err`
    /// 同じ番号のインスタンスを外したら `trimmed` に記す（読み込みで中身を除いたかは、除いた所で記す）。
    /// 書いてある始まりのコマと組にして返す
    fn from_saved(s: SavedKeyframe, trimmed: &mut bool) -> Result<(usize, Self), String> {
        let broken = || "プロジェクトファイルの地図が壊れています".to_owned();
        if s.maps.iter().any(|m| m.stored.is_none()) {
            return Err(broken());
        }
        let mut seen_gids = HashSet::with_capacity(s.instances.len());
        let saved_instances = s.instances.len();
        let instances: Vec<SymbolInstance> = s
            .instances
            .into_iter()
            .map(SymbolInstance::from)
            // 同じキーフレームに同じ番号のインスタンスは1つだけ。使えない番号（`is_safe_id`）は、文書を作るときに
            // 新しい番号にし、無いシンボルを指すインスタンスはそこで外す
            .filter(|inst| seen_gids.insert(inst.binding.instance_gid))
            .collect();
        *trimmed |= instances.len() < saved_instances;
        // セル画の作りの決まり（書く入口と同じ1つ：`check_cel_form`）。同じ番号のインスタンスは上で外してある
        crate::timeline::check_cel_form(
            s.maps.iter().map(|m| m.group_path.as_slice()),
            instances.iter().map(|i| (i.binding.instance_gid, i.parent_scope.as_slice())),
        )
        .map_err(|_| broken())?;
        let cel = Cel::from_saved(s.maps, instances);
        // 音の付いたキーフレームは音だけ（セル画が空でなければ壊れたファイル）。音の区間が正しいかは、音のライブラリを
        // 読んでから確かめる（`into_document_noting`）
        if s.sound.is_some() && !cel.is_empty() {
            return Err(broken());
        }
        let sound = s.sound.map(|c| crate::sound::SoundClip {
            sound: c.sound,
            start: c.start,
            end: c.end,
        });
        Ok((
            s.start_frame,
            Self::with_cel(cel, s.tween, s.tween_rotate).with_sound(sound),
        ))
    }
}

fn default_layer_visible() -> bool {
    true
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct SavedLayer {
    pub(crate) id: u64,
    pub(crate) name: String,
    #[serde(default = "default_layer_visible")]
    pub(crate) visible: bool,
    #[serde(default)]
    pub(crate) locked: bool,
    pub(crate) frame_count: usize,
    #[serde(default)]
    pub(crate) keyframes: Vec<SavedKeyframe>,
}

impl From<&Layer> for SavedLayer {
    fn from(layer: &Layer) -> Self {
        Self {
            keyframes: layer
                .keyframes_with_start()
                .map(|(start, kf)| SavedKeyframe::new(start, kf))
                .collect(),
            ..Self::header(layer)
        }
    }
}

impl SavedLayer {
    /// レイヤー `layer` の、キーフレームを除いた保存の形（保存の形と、復旧の書き足すファイル `recovery_journal` の両方が
    /// これを使う）
    pub(crate) fn header(layer: &Layer) -> Self {
        Self {
            id: layer.id,
            name: layer.name.clone(),
            visible: layer.visible,
            locked: layer.locked,
            frame_count: layer.frame_count,
            keyframes: Vec::new(),
        }
    }
}

impl Layer {
    /// 読んだレイヤー `s`。レイヤーの長さの外のキーフレームと、同じコマの2つ目からのキーフレームは外し、外したら
    /// `trimmed` に記す（0 コマ目にキーフレームが無ければ空のキーフレームを足す。これは何も除かない）
    fn from_saved(s: SavedLayer, trimmed: &mut bool) -> Result<Self, String> {
        // コマ数に上限は設けない（編集で作れる長さはみな読み戻す。読み込みはキーフレームの数だけ回る：レビュー f01d518
        // の N-2004）
        let frame_count = s.frame_count.max(1);
        let saved_keyframes = s.keyframes.len();
        let mut keyframes: Vec<(usize, Arc<Keyframe>)> = Vec::with_capacity(s.keyframes.len());
        for saved in s.keyframes {
            let (start, kf) = Keyframe::from_saved(saved, trimmed)?;
            if start < frame_count {
                keyframes.push((start, Arc::new(kf)));
            }
        }
        keyframes.sort_by_key(|&(start, _)| start);
        keyframes.dedup_by_key(|&mut (start, _)| start);
        *trimmed |= keyframes.len() < saved_keyframes;
        if keyframes.first().is_none_or(|&(start, _)| start != 0) {
            keyframes.insert(0, (0, Arc::new(Keyframe::new_blank())));
        }
        Ok(Self {
            // 使えない番号（`is_safe_id`）は、タイムラインを作るときに新しい番号にする
            id: s.id,
            name: s.name,
            visible: s.visible,
            locked: s.locked,
            frame_count,
            keyframes: KeyframeList::from_keyed(keyframes),
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct SavedTimeline {
    pub(crate) fps: u32,
    pub(crate) total_frames: usize,
    pub(crate) current_frame: usize,
    pub(crate) active_layer: usize,
    #[serde(default)]
    pub(crate) next_layer_number: usize,
    pub(crate) layers: Vec<SavedLayer>,
}

impl From<&Timeline> for SavedTimeline {
    fn from(tl: &Timeline) -> Self {
        Self {
            layers: tl.layers.iter().map(SavedLayer::from).collect(),
            ..Self::header(tl)
        }
    }
}

impl SavedTimeline {
    /// タイムライン `tl` の、レイヤーを除いた保存の形（fps は 24。文書の fps は作品の一番上で入れる：`saved_fps`。保存の
    /// 形と、復旧の書き足すファイル `recovery_journal` の両方がこれを使う）
    pub(crate) fn header(tl: &Timeline) -> Self {
        Self {
            fps: 24,
            total_frames: tl.total_frames,
            current_frame: tl.current_frame,
            active_layer: tl.active_layer,
            next_layer_number: tl.next_layer_number,
            layers: Vec::new(),
        }
    }
}

impl Timeline {
    /// 読んだタイムライン `s`。レイヤーが上限（`MAX_LAYERS_PER_TIMELINE`）より多ければ後ろを外し、外したら `trimmed` に
    /// 記す（レイヤー・キーフレーム・インスタンスで外したものも、そこで記す）
    fn from_saved(s: SavedTimeline, trimmed: &mut bool) -> Result<Self, String> {
        *trimmed |= s.layers.len() > MAX_LAYERS_PER_TIMELINE;
        let mut layers: Vec<Layer> = s
            .layers
            .into_iter()
            .take(MAX_LAYERS_PER_TIMELINE)
            .map(|l| Layer::from_saved(l, trimmed))
            .collect::<Result<_, _>>()?;
        if layers.is_empty() {
            layers.push(Layer::new_with_id(1, "Layer 1"));
        }

        // 使えない・重なるレイヤーの番号は新しい番号にし（`FreshIds`）、各キーフレームのインスタンスの layer_id もそろえる
        let mut seen_layer_ids = HashSet::with_capacity(layers.len());
        let mut layer_ids = FreshIds::after(layers.iter().map(|l| l.id));
        for layer in &mut layers {
            if !is_safe_id(layer.id) || !seen_layer_ids.insert(layer.id) {
                layer.id = layer_ids.take();
            }
            let lid = layer.id;
            for i in 0..layer.keyframes.len() {
                let kf = layer.keyframe_mut(i);
                for inst in kf.cel.instances_mut() {
                    inst.layer_id = lid;
                }
            }
        }

        let total_frames = layers
            .iter()
            .map(|l| l.frame_count)
            .max()
            .unwrap_or(1)
            .max(1);
        let active_layer = s.active_layer.min(layers.len().saturating_sub(1));
        let current_frame = s.current_frame.min(total_frames.saturating_sub(1));
        let mut tl = Self {
            layers,
            current_frame,
            active_layer,
            total_frames,
            next_layer_number: s.next_layer_number.max(2),
            cell_selection: Some(TimelineCellSelection::single(active_layer, current_frame)),
        };
        tl.sync_total_frames();
        Ok(tl)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct SavedSymbolDef {
    pub(crate) id: u64,
    pub(crate) name: String,
    pub(crate) timeline: SavedTimeline,
}

impl From<&SymbolDef> for SavedSymbolDef {
    fn from(sym: &SymbolDef) -> Self {
        Self {
            id: sym.id,
            name: sym.name.clone(),
            timeline: (&sym.timeline).into(),
        }
    }
}

impl SymbolDef {
    /// 読んだシンボル `s`（タイムラインで外したものは `trimmed` に記す）
    fn from_saved(s: SavedSymbolDef, trimmed: &mut bool) -> Result<Self, String> {
        Ok(SymbolDef::with_timeline(
            s.id,
            s.name,
            Timeline::from_saved(s.timeline, trimmed)?,
        ))
    }
}

/// `.is2d` ファイルの中身のルート構造体（版の番号はファイルの頭にだけ持つ：`project_file_head`）。postcard は項目を
/// 並びの位置で読むので、書かない項目（`skip_serializing_if`）は作らない。塊の表は、書くときは文書の塊から流す形
/// （`WriteTiles`）、アプリが読むときは読みながら塊にした形（`ReadTiles`）。テストは、中身を見たり細工したりするために
/// 塊の表を保存の形のまま読み書きする（読むのはどれも同じ読み込みの道 `read_project` を通る）
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct ProjectDocument<T = WriteTiles> {
    pub(crate) stage_size: [f32; 2],
    pub(crate) timeline: SavedTimeline,
    #[serde(default)]
    pub(crate) library: Vec<SavedSymbolDef>,
    #[serde(default)]
    pub(crate) next_symbol_id: u64,
    #[serde(default)]
    pub(crate) next_group_id: u64,
    #[serde(default)]
    pub(crate) recovery_source: Option<PathBuf>,
    /// 地図の塊の表（キーフレームの地図は、この表の塊の番号の並び）
    #[serde(default)]
    pub(crate) map_tiles: T,
    /// 音のライブラリ（版 8 から）
    #[serde(default)]
    pub(crate) sounds: Vec<SavedSound>,
    #[serde(default)]
    pub(crate) next_sound_id: u64,
    /// 次に足すレイヤーの番号（版 11 から：作品の数え札。前はタイムラインごとに持った）
    #[serde(default)]
    pub(crate) next_layer_id: u64,
}

/// 番号 `id`（レイヤー・シンボル・グループ・インスタンス）が、ファイルから読んでそのまま使える番号か（0 でなく、安全な
/// 上限 `MAX_SAFE_ENTITY_ID` 以下）。読み込みで番号を確かめる決まりはこれ1つ
fn is_safe_id(id: u64) -> bool {
    id > 0 && id <= MAX_SAFE_ENTITY_ID
}

/// 読み込みで新しく振る番号（レイヤー・シンボル・グループ・音。番号を新しくする決まりはこれ1つ）：ファイルにある使える
/// 番号（`is_safe_id`）のいちばん大きいものの次から順に振るので、ファイルにある使える番号とも、振った番号どうしとも
/// 重ならない
struct FreshIds {
    next: u64,
}

impl FreshIds {
    /// ファイルにある番号 `ids` のあとから振る
    fn after(ids: impl Iterator<Item = u64>) -> Self {
        let max = ids.filter(|&id| is_safe_id(id)).max().unwrap_or(0);
        Self { next: max + 1 }
    }

    /// 新しい番号を1つ取る
    fn take(&mut self) -> u64 {
        let id = self.next;
        self.next += 1;
        id
    }

    /// 文書に入れる「次に振る番号」：ファイルに書いてある次の番号 `saved`（使えない番号なら無いものとする）を、ファイル
    /// にある番号と振った番号のどれよりも後にしたもの
    fn next_to_save(&self, saved: u64) -> u64 {
        if is_safe_id(saved) {
            saved.max(self.next)
        } else {
            self.next
        }
    }
}


/// 文書 `doc` の保存する fps（範囲に収める。保存の形と復旧の書き足すファイルの両方がこれを使う）
pub(crate) fn saved_fps(doc: &Document) -> u32 {
    doc.fps.clamp(1, crate::symbol::MAX_FPS)
}

impl SavedScopeMap {
    /// 地図のしまう形（`stored`）を、塊の表 `writer` の塊の番号の並びにする（共有している塊は1回だけ表に足す。保存の形
    /// 記録の書き手 `RecordWriter` が使う）
    pub(crate) fn write_tiles(&mut self, writer: &mut WriteTiles) {
        if let Some(stored) = self.stored.take() {
            self.tiles = writer.tiles_of(&stored);
        }
    }
}


impl<T> ProjectDocument<T> {
    /// 書いてある地図（ルートとシンボルのタイムラインの、キーフレームの順）みなに `f`
    pub(crate) fn for_each_map(&mut self, mut f: impl FnMut(&mut SavedScopeMap)) {
        let timelines = std::iter::once(&mut self.timeline)
            .chain(self.library.iter_mut().map(|s| &mut s.timeline));
        for tl in timelines {
            for kf in tl.layers.iter_mut().flat_map(|l| l.keyframes.iter_mut()) {
                kf.maps.iter_mut().for_each(&mut f);
            }
        }
    }
}

impl ProjectDocument<ReadTiles> {
    /// 書いてある小数（ステージの大きさ、地図の頂点と制御点、インスタンスの変換・中心点・色効果）がみな有限か。文書には
    /// 有限な数しか入らない（ステージの大きさは書く入口と同じ `stage_size_of`、地図は格子の内：`fits_grid`、インスタンスは
    /// 書く入口と同じ `SymbolInstanceBinding::is_finite`）ので、そうでなければ壊れたファイル（読むときに断る：
    /// `into_document_noting`。前の版のアプリが書いた有限でない数のあるファイルも、ここで断る）
    fn only_finite_numbers(&self) -> bool {
        let timeline_ok = |tl: &SavedTimeline| {
            tl.layers.iter().flat_map(|l| &l.keyframes).all(|kf| {
                // インスタンスは書く入口と同じ判定
                kf.instances
                    .iter()
                    .all(|inst| SymbolInstanceBinding::from(inst.binding).is_finite())
            })
        };
        crate::symbol::stage_size_of(Vec2::from(self.stage_size)).is_some()
            && self.map_tiles.only_finite_numbers()
            && std::iter::once(&self.timeline)
                .chain(self.library.iter().map(|s| &s.timeline))
                .all(timeline_ok)
    }

    /// 読んだ中身から文書を作る。壊れた・使えない所を除いたら `trimmed` に記す（除いた所でその場で記す：レイヤー・
    /// キーフレーム・インスタンスの `from_saved` と、無いシンボルを指すインスタンス。番号の振り直しや、空のキーフレーム・
    /// レイヤーを足すことは何も除かないので記さない）
    pub(crate) fn into_document_noting(mut self, trimmed: &mut bool) -> Result<Document, String> {
        if !self.only_finite_numbers() {
            return Err("ファイルが壊れています（有限でない数があります）".to_owned());
        }
        // 地図：塊の表から、塊を共有したしまう形を作る（番号が表の外なら壊れたファイル。塊に記した図形の印は新しい番号に
        // 替える：`TileReader::new`）
        let broken = || "プロジェクトファイルの地図が壊れています".to_owned();
        let reader = TileReader::new(std::mem::take(&mut self.map_tiles)).ok_or_else(broken)?;
        let mut all_read = true;
        self.for_each_map(|m| {
            m.stored = reader.map(&m.tiles);
            all_read &= m.stored.is_some();
        });
        if !all_read {
            return Err(broken());
        }
        // 音のライブラリ：どの音も、使える番号（`is_safe_id`）で番号が重ならないこと。そうでなければ壊れたファイル（黙って
        // 音を外さない）。波形にできない音は「読めない音」として開いて知らせる（`Sound::loaded`）
        let mut sounds: Vec<Arc<crate::sound::Sound>> = Vec::with_capacity(self.sounds.len());
        let mut sound_ids = HashSet::with_capacity(self.sounds.len());
        for s in std::mem::take(&mut self.sounds) {
            if !is_safe_id(s.id) || !sound_ids.insert(s.id) {
                return Err("プロジェクトファイルの音が壊れています".to_owned());
            }
            sounds.push(Arc::new(crate::sound::Sound::loaded(s.id, s.name, s.bytes)));
        }
        let fps = self.timeline.fps.clamp(1, crate::symbol::MAX_FPS);
        let mut timeline = Timeline::from_saved(self.timeline, trimmed)?;

        // 使えない・重なるシンボルの番号は新しい番号にして（`FreshIds`）ライブラリを作る
        let mut seen_sym_ids = HashSet::with_capacity(self.library.len());
        let mut sym_ids = FreshIds::after(self.library.iter().map(|s| s.id));
        let mut sym_id_remap: HashMap<u64, u64> = HashMap::new();
        let mut library: Vec<SymbolDef> = Vec::with_capacity(self.library.len());
        for saved_sym in self.library {
            let mut sym = SymbolDef::from_saved(saved_sym, trimmed)?;
            let orig_id = sym.id;
            if !is_safe_id(sym.id) || !seen_sym_ids.insert(sym.id) {
                sym.id = sym_ids.take();
                seen_sym_ids.insert(sym.id);
                if orig_id > 0 && !seen_sym_ids.contains(&orig_id) {
                    sym_id_remap.entry(orig_id).or_insert(sym.id);
                }
            }
            library.push(sym);
        }

        // 使えないグループの番号（group_id / instance_gid）を振り直す番号（`FreshIds`。同じ番号には同じ新しい番号）
        let used_gids = std::iter::once(&timeline)
            .chain(library.iter().map(|s| &s.timeline))
            .flat_map(|tl| &tl.layers)
            .flat_map(|layer| &layer.keyframes)
            .flat_map(|kf| {
                let maps = kf
                    .cel
                    .maps()
                    .iter()
                    .flat_map(|m| m.group_path.iter().copied());
                let insts = kf.cel.instances().iter().flat_map(|inst| {
                    inst.parent_scope
                        .iter()
                        .copied()
                        .chain([inst.binding.instance_gid])
                });
                maps.chain(insts)
            });
        let mut gids = FreshIds::after(used_gids);
        let mut gid_map: HashMap<u64, u64> = HashMap::new();

        // 存在しない symbol_id（孤児インスタンス）をルートタイムラインおよび各シンボルタイムラインから除去し、
        // 再採番された symbol_id / group_id / instance_gid の反映と tween フラグの正規化を行う
        for tl in std::iter::once(&mut timeline).chain(library.iter_mut().map(|s| &mut s.timeline))
        {
            for layer in &mut tl.layers {
                for i in 0..layer.keyframes.len() {
                    let kf = layer.keyframe_mut(i);
                    let needs_remap = kf
                        .cel
                        .maps()
                        .iter()
                        .any(|m| m.group_path.iter().any(|&gid| !is_safe_id(gid)))
                        || kf.cel.instances().iter().any(|inst| {
                            sym_id_remap.contains_key(&inst.binding.symbol_id)
                                || !is_safe_id(inst.binding.instance_gid)
                                || inst.parent_scope.iter().any(|&gid| !is_safe_id(gid))
                        });
                    if needs_remap {
                        kf.cel.remap_ids(
                            |gid| {
                                if is_safe_id(gid) {
                                    gid
                                } else {
                                    *gid_map.entry(gid).or_insert_with(|| gids.take())
                                }
                            },
                            |sym| sym_id_remap.get(&sym).copied().unwrap_or(sym),
                            None,
                        );
                    }
                    if kf
                        .cel
                        .instances()
                        .iter()
                        .any(|inst| !seen_sym_ids.contains(&inst.binding.symbol_id))
                    {
                        kf.cel
                            .instances_mut()
                            .retain(|inst| seen_sym_ids.contains(&inst.binding.symbol_id));
                        *trimmed = true;
                    }
                }
            }
        }

        // 数が有限なことは、読み込みの初めに確かめてある（`only_finite_numbers`。ステージの大きさも同じ `stage_size_of`）
        let stage_size = crate::symbol::stage_size_of(Vec2::from(self.stage_size))
            .expect("読み込みの初めに有限か確かめてある");
        // キーフレームの音の区間は、どれも音のライブラリの音を指し、始まりが終わりより前であること（`is_well_formed`）。
        // 音の長さを越える区間は残し、その音を知らせる（`exceeds`）
        let sounds = crate::sound::SoundLibrary::from(sounds);
        let mut short_sounds: Vec<u64> = Vec::new();
        for c in std::iter::once(&timeline).chain(library.iter().map(|s| &s.timeline)).flat_map(|tl| tl.sound_clips()) {
            if !c.is_well_formed(&sounds) {
                return Err("プロジェクトファイルの音が壊れています".to_owned());
            }
            if sounds.get(c.sound).is_some_and(|s| c.exceeds(s)) {
                short_sounds.push(c.sound);
            }
        }
        short_sounds.sort_unstable();
        short_sounds.dedup();
        let fresh_sound_ids = FreshIds::after(sounds.iter().map(|s| s.id));
        // レイヤーの番号は、どのタイムラインのレイヤーの番号よりも後から配る
        let fresh_layer_ids = FreshIds::after(
            std::iter::once(&timeline).chain(library.iter().map(|s| &s.timeline)).flat_map(|tl| tl.layers.iter().map(|l| l.id)),
        );
        let doc = Document::loaded(
            timeline,
            library.into_iter().map(Arc::new).collect(),
            sounds,
            stage_size,
            fps,
            [
                sym_ids.next_to_save(self.next_symbol_id),
                gids.next_to_save(self.next_group_id),
                fresh_sound_ids.next_to_save(self.next_sound_id),
                fresh_layer_ids.next_to_save(self.next_layer_id),
            ]
            .map(crate::symbol::Counter::starting_at),
        );
        doc.watch_broken_maps();
        for s in doc.sounds.iter().filter(|s| !s.readable()) {
            doc.load_notices.push(crate::timeline::LoadNotice::UnreadableSound(s.name.clone()));
        }
        for s in short_sounds.iter().filter_map(|&id| doc.sounds.get(id)) {
            doc.load_notices.push(crate::timeline::LoadNotice::ShortSound(s.name.clone()));
        }
        Ok(doc)
    }
}


/// 読んだバイトの長さと CRC32 を数えながら `inner` から読む読み手（中身を手元に持たずに、頭の数と照らす：
/// `read_project`）。postcard は読み手の失敗を自分の失敗（中身が途中で終わった）に置き換えるので、読み手の失敗は返さずに
/// ここに取っておき（失敗した後は終わりとして何も読まない）、読み終えてから見る
pub(crate) struct BodyReader<R> {
    inner: R,
    len: u64,
    crc: crc32fast::Hasher,
    failed: Option<std::io::Error>,
}

impl<R> BodyReader<R> {
    /// `inner` から読んだ長さと CRC32 を数えながら読む読み手
    pub(crate) fn new(inner: R) -> Self {
        Self { inner, len: 0, crc: crc32fast::Hasher::new(), failed: None }
    }

    /// 読んだ長さと CRC32（読めなかったら、そのわけ）
    pub(crate) fn finish(self) -> std::io::Result<(u64, u32)> {
        match self.failed {
            Some(e) => Err(e),
            None => Ok((self.len, self.crc.finalize())),
        }
    }
}

impl<R: std::io::Read> std::io::Read for BodyReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        while self.failed.is_none() {
            match self.inner.read(buf) {
                Ok(n) => {
                    self.len += n as u64;
                    self.crc.update(&buf[..n]);
                    return Ok(n);
                }
                // 割り込まれただけなら読み直す（`Read` の決まり）
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => self.failed = Some(e),
            }
        }
        Ok(0)
    }
}

/// 中身を流しながら postcard に渡す所（postcard の `Flavor`）。数バイトずつ渡すときは、`input` の手元の一続きをそのまま
/// 貸し（写さない）、貸した所は次に読むときに読み終えたことにする（`lent`）。手元の一続きに収まらない長さ（長い文字列・
/// 一続きの切れ目にかかる数）は `scratch` に、読めたバイトの分だけ広げて置く（長さに上限を置かず、細工した長さでも
/// ファイルの大きさを越えて取らない）。保存用の型は借りる値を持たないので、借りたまま返す読み方（`try_take_n`）は無い
struct BodyFlavor<R> {
    input: std::io::BufReader<R>,
    lent: usize,
    scratch: Vec<u8>,
}

impl<R: std::io::Read> BodyFlavor<R> {
    /// 手元の、まだ渡していない一続き（無ければ次を読む。中身の終わりなら空）
    fn buffered(&mut self) -> postcard::Result<&[u8]> {
        use std::io::BufRead;
        self.input.consume(std::mem::take(&mut self.lent));
        self.input
            .fill_buf()
            .map_err(|_| postcard::Error::DeserializeUnexpectedEnd)
    }
}

impl<'de, R: std::io::Read + 'de> postcard::de_flavors::Flavor<'de> for BodyFlavor<R> {
    type Remainder = std::io::BufReader<R>;
    type Source = R;

    fn pop(&mut self) -> postcard::Result<u8> {
        let byte = *self
            .buffered()?
            .first()
            .ok_or(postcard::Error::DeserializeUnexpectedEnd)?;
        self.lent = 1;
        Ok(byte)
    }

    fn try_take_n(&mut self, _ct: usize) -> postcard::Result<&'de [u8]> {
        Err(postcard::Error::WontImplement)
    }

    fn try_take_n_temp<'a>(&'a mut self, ct: usize) -> postcard::Result<&'a [u8]>
    where
        'de: 'a,
    {
        use std::io::Read;
        if self.buffered()?.len() >= ct {
            self.lent = ct;
            return Ok(&self.input.buffer()[..ct]);
        }
        self.scratch.clear();
        let got = (&mut self.input)
            .take(ct as u64)
            .read_to_end(&mut self.scratch)
            .map_err(|_| postcard::Error::DeserializeUnexpectedEnd)?;
        if got < ct {
            return Err(postcard::Error::DeserializeUnexpectedEnd);
        }
        Ok(&self.scratch)
    }

    fn finalize(mut self) -> postcard::Result<std::io::BufReader<R>> {
        use std::io::BufRead;
        self.input.consume(self.lent);
        Ok(self.input)
    }
}

/// 流して読んだもの：読んだ値（postcard の答え）・読んだ長さ・その CRC32・読み残した長さ
pub(crate) struct Streamed<T> {
    pub(crate) value: postcard::Result<T>,
    pub(crate) len: u64,
    pub(crate) crc: u32,
    pub(crate) left: u64,
}

/// 読み手 `input` の中身を、型 `T` として postcard で流しながら読む（中身を1つの並びに持たず、取るのは読めたバイトの分
/// だけ：`BodyFlavor`）。読んだ長さと CRC32（読み残しも含めて読み切った分）も返す。作品ファイルの中身と、復旧ファイルの
/// 大きな記録を読む、1つの読み手（設計審査 b7208d9 の 6.。前は復旧ファイルだけファイル全体をメモリへ読んだ）
pub(crate) fn read_streamed<T: DeserializeOwned>(input: impl std::io::Read) -> Result<Streamed<T>, String> {
    let mut de = postcard::Deserializer::from_flavor(BodyFlavor {
        input: std::io::BufReader::with_capacity(READ_CHUNK_LEN, BodyReader::new(input)),
        lent: 0,
        scratch: Vec::new(),
    });
    let value = T::deserialize(&mut de);
    let mut rest = de.finalize().expect("流す所の終わりは失敗しない");
    let left = std::io::copy(&mut rest, &mut std::io::sink()).map_err(read_failed)?;
    let (len, crc) = rest.into_inner().finish().map_err(read_failed)?;
    Ok(Streamed { value, len, crc, left })
}

/// 作品のファイルを1回に読む大きさ（`read_project`）。読む1回ごとにシステムコールの手間がかかるので、小さいと開くのが
/// 遅くなる（`BufReader` の標準の 8KB では、作品 A（277MB）を読む回数が約 3 万 4 千回になり、読み込みが 465ms。256KB で
/// 368ms）。1MB・4MB でも 365〜370ms で変わらないので、速さが頭打ちになるいちばん小さい大きさにする（計測：Apple M5）
pub(crate) const READ_CHUNK_LEN: usize = 256 * 1024;

/// ファイルを読めなかったときの知らせ
pub(crate) fn read_failed(e: std::io::Error) -> String {
    format!("ファイルの読み込みに失敗しました: {e}")
}

/// パスのファイル名を文字列で返す（取り出せなければ `fallback`）
pub(crate) fn file_name_or<'a>(path: &'a Path, fallback: &'a str) -> &'a str {
    path.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(fallback)
}

// ============================================================================
// 記録の列（作品ファイルと復旧ファイルの中身：書き手 `RecordWriter`・読み手 `read_file` の1つずつ）
// ============================================================================

/// 記録1つ。目録は入れ子で、どの段も、書き換えていない所は前に書いた記録を番号で指す（番号は書いた順の通し番号）
#[derive(Serialize, Deserialize)]
enum Record {
    /// 塊の表の続き（色・塊・その塊の線。番号は前の記録の続き）
    Tiles {
        colors: Vec<[u8; 4]>,
        tiles: Vec<SavedTile>,
        strokes: Vec<SavedStroke>,
    },
    /// 音（音のライブラリの1つ）：番号・名前と、どのデータか（データは `SoundBytes` で1回だけ書く）
    Sound(SoundEntry),
    /// 音のライブラリ（番号 `id`）：音の番号の並び
    Sounds { id: u64, sounds: Vec<u64> },
    /// キーフレームの塊（番号 `id`）：塊の基準からの始まりのコマ `rels` と、キーフレーム（始まりのコマは使わない）
    Chunk {
        id: u64,
        rels: Vec<i64>,
        keyframes: Vec<SavedKeyframe>,
    },
    /// レイヤーのキーフレームの並び（番号 `id`）：キーフレームの塊の番号と、その塊の基準
    Keyframes { id: u64, chunks: Vec<(u64, i64)> },
    /// ライブラリの塊（番号 `id`）：シンボル（番号の順）
    Symbols { id: u64, symbols: Vec<JournalSymbol> },
    /// ライブラリの見せる順の塊（番号 `id`）：シンボルの番号
    Order { id: u64, ids: Vec<u64> },
    /// 目録（作品の一番上）。最後の目録が今の作品
    Manifest(Manifest),
    /// 保存した（最後の記録がこれなら、復元するものは無い：`is_saved`）
    Saved,
    /// 音のデータ（番号 `id`。WAV・MP3 のバイトのまま）。同じデータは1回だけ書く（名前を変えても書き直さない）
    SoundBytes { id: u64, bytes: Arc<[u8]> },
}

/// 音の記録の形：音の番号・名前・データの記録の番号
#[derive(Serialize, Deserialize)]
struct SoundEntry {
    id: u64,
    name: String,
    bytes: u64,
}

#[derive(Serialize, Deserialize)]
struct Manifest {
    stage_size: [f32; 2],
    fps: u32,
    next_symbol_id: u64,
    next_group_id: u64,
    next_sound_id: u64,
    next_layer_id: u64,
    recovery_source: Option<PathBuf>,
    timeline: JournalTimeline,
    /// ライブラリの塊の番号の並びと、見せる順の塊の番号の並び
    library: Vec<u64>,
    order: Vec<u64>,
    /// 音のライブラリの番号
    sounds: u64,
}

#[derive(Serialize, Deserialize)]
struct JournalSymbol {
    id: u64,
    name: String,
    timeline: JournalTimeline,
}

/// タイムラインの目録の形：レイヤーを除いた保存の形（`SavedTimeline::header`）と、レイヤー
#[derive(Serialize, Deserialize)]
struct JournalTimeline {
    header: SavedTimeline,
    layers: Vec<JournalLayer>,
}

/// レイヤーの目録の形：キーフレームを除いた保存の形（`SavedLayer::header`）と、キーフレームの並びの番号
#[derive(Serialize, Deserialize)]
struct JournalLayer {
    header: SavedLayer,
    keyframes: u64,
}

/// 書いたもの（番地 → 番号）。番地の持ち主の写しを一緒に持つので、持っている間は、番地がほかのものに使い回されない
struct Written<T> {
    ids: HashMap<usize, (u64, T)>,
}

impl<T> Default for Written<T> {
    fn default() -> Self {
        Self { ids: HashMap::new() }
    }
}

/// 記録の種類（記録の枠に書く1バイト。記録から決めるのは `Record::kind` の1つ。読み手はこの1バイトで読み分けるので、
/// 列挙 `Record` の並びに頼らない：設計審査 01e4283 の D-3。前は読み手が並びの番号を手で写していた）
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum RecordKind {
    Tiles,
    Sound,
    Sounds,
    Chunk,
    Keyframes,
    Symbols,
    Order,
    Manifest,
    Saved,
    SoundBytes,
}

impl RecordKind {
    const ALL: [Self; 10] = [
        Self::Tiles,
        Self::Sound,
        Self::Sounds,
        Self::Chunk,
        Self::Keyframes,
        Self::Symbols,
        Self::Order,
        Self::Manifest,
        Self::Saved,
        Self::SoundBytes,
    ];

    /// 枠に書く1バイト（`ALL` の何番目か）
    fn byte(self) -> u8 {
        Self::ALL.iter().position(|&k| k == self).expect("種類の表にある") as u8
    }

    /// 枠の1バイトの種類（表に無ければ `None`）
    fn of_byte(b: u8) -> Option<Self> {
        Self::ALL.get(usize::from(b)).copied()
    }

    /// 大きな記録か（読み手の1回目は場所だけ覚え、2回目に最後の目録の作品が指すものだけを読む）
    fn is_big(self) -> bool {
        matches!(self, Self::Tiles | Self::Chunk | Self::SoundBytes)
    }
}

impl Record {
    /// 記録の種類（枠に書く）
    fn kind(&self) -> RecordKind {
        match self {
            Record::Tiles { .. } => RecordKind::Tiles,
            Record::Sound(_) => RecordKind::Sound,
            Record::Sounds { .. } => RecordKind::Sounds,
            Record::Chunk { .. } => RecordKind::Chunk,
            Record::Keyframes { .. } => RecordKind::Keyframes,
            Record::Symbols { .. } => RecordKind::Symbols,
            Record::Order { .. } => RecordKind::Order,
            Record::Manifest(_) => RecordKind::Manifest,
            Record::Saved => RecordKind::Saved,
            Record::SoundBytes { .. } => RecordKind::SoundBytes,
        }
    }

    /// 記録の番号（枠に書く。番号で指される記録のもの。番号の無い記録は 0）
    fn id(&self) -> u64 {
        match self {
            Record::Sound(s) => s.id,
            Record::Sounds { id, .. }
            | Record::Chunk { id, .. }
            | Record::Keyframes { id, .. }
            | Record::Symbols { id, .. }
            | Record::Order { id, .. }
            | Record::SoundBytes { id, .. } => *id,
            Record::Tiles { .. } | Record::Manifest(_) | Record::Saved => 0,
        }
    }
}

/// 作品を記録の列で書く書き手（作品ファイルも復旧ファイルも、これ1つで書く：設計審査 01e4283 の D-3）。書いたもの（地図の
/// 塊・音・キーフレームの塊・並び・ライブラリの塊）を覚えていて、次に書くときは、変わった所だけを書き足す（作品ファイル
/// は新しい書き手で全部を書く。復旧ファイルは同じ書き手で書き足す：`recovery_journal::Journal`）
pub(crate) struct RecordWriter {
    /// 書いた地図の塊の表（番号はファイルの中で続く）と、書いた塊の数・色の数
    tiles: WriteTiles,
    done: (usize, usize),
    /// 書いた音のデータ（番地は `Sound::bytes_address`）・音・音のライブラリ・キーフレームの塊・キーフレームの並び・
    /// ライブラリの塊
    sound_bytes: Written<Arc<[u8]>>,
    sounds: Written<Arc<crate::sound::Sound>>,
    sound_lists: Written<crate::sound::SoundLibrary>,
    chunks: Written<crate::chunked_list::Chunk<Keyframe>>,
    keyframe_lists: Written<crate::chunked_list::ChunkedList<Keyframe>>,
    symbol_chunks: Written<crate::chunked_list::Chunk<SymbolDef>>,
    order_chunks: Written<crate::chunked_list::Chunk<crate::symbol::Slot>>,
    /// 次に振る番号
    next_id: u64,
}

impl Default for RecordWriter {
    fn default() -> Self {
        Self {
            tiles: WriteTiles::default(),
            done: (0, 0),
            sound_bytes: Written::default(),
            sounds: Written::default(),
            sound_lists: Written::default(),
            chunks: Written::default(),
            keyframe_lists: Written::default(),
            symbol_chunks: Written::default(),
            order_chunks: Written::default(),
            next_id: 1,
        }
    }
}

impl RecordWriter {
    /// 文書 `doc`（元のファイル `source`）の、前に書いてから変わった所の記録と目録を `out` に書く（初めてなら全部）。書いた
    /// バイト数。手間は、変わった所（新しい塊・書き換えた並び）と、作品の一番上の段（タイムラインのレイヤー・ライブラリの塊の
    /// 数）で決まる
    pub(crate) fn write(&mut self, out: &mut impl std::io::Write, doc: &Document, source: Option<&Path>) -> std::io::Result<u64> {
        let mut written = 0;
        let sounds = self.sound_list(out, &doc.sounds, &mut written)?;
        let timeline = self.timeline(out, &doc.root, &mut written)?;
        let mut library = Vec::new();
        for c in doc.library().def_chunks() {
            library.push(self.symbol_chunk(out, c, &mut written)?);
        }
        let mut order = Vec::new();
        for c in doc.library().order_chunks() {
            order.push(self.order_chunk(out, c, &mut written)?);
        }
        let manifest = Manifest {
            stage_size: [doc.stage_size.x, doc.stage_size.y],
            fps: saved_fps(doc),
            next_symbol_id: doc.next_symbol_id.next(),
            next_group_id: doc.next_group_id.next(),
            next_sound_id: doc.next_sound_id.next(),
            next_layer_id: doc.next_layer_id.next(),
            recovery_source: source.map(Path::to_path_buf),
            timeline,
            library,
            order,
            sounds,
        };
        written += write_record(out, &Record::Manifest(manifest))?;
        Ok(written)
    }

    /// 「保存した」の印の記録を `out` に書く（復旧ファイル：最後の記録がこれなら、復元するものは無い）
    pub(crate) fn write_saved_mark(out: &mut impl std::io::Write) -> std::io::Result<u64> {
        write_record(out, &Record::Saved)
    }

    /// 新しく振る番号
    fn new_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id - 1
    }

    /// 音のライブラリ `lib` の番号（まだ書いていなければ、新しい音と並びを書く）
    fn sound_list(&mut self, out: &mut impl std::io::Write, lib: &SoundLibrary, written: &mut u64) -> std::io::Result<u64> {
        let key = lib.address();
        if let Some((id, _)) = self.sound_lists.ids.get(&key) {
            return Ok(*id);
        }
        for s in lib.iter() {
            let k = Arc::as_ptr(s) as usize;
            if self.sounds.ids.contains_key(&k) {
                continue;
            }
            let bytes = match self.sound_bytes.ids.get(&s.bytes_address()) {
                Some((id, _)) => *id,
                None => {
                    let id = self.new_id();
                    *written += write_record(out, &Record::SoundBytes { id, bytes: s.encoded.clone() })?;
                    self.sound_bytes.ids.insert(s.bytes_address(), (id, s.encoded.clone()));
                    id
                }
            };
            *written += write_record(out, &Record::Sound(SoundEntry { id: s.id, name: s.name.clone(), bytes }))?;
            self.sounds.ids.insert(k, (s.id, s.clone()));
        }
        let id = self.new_id();
        *written += write_record(out, &Record::Sounds { id, sounds: lib.iter().map(|s| s.id).collect() })?;
        self.sound_lists.ids.insert(key, (id, lib.clone()));
        Ok(id)
    }

    /// ライブラリの塊 `c` の番号（まだ書いていなければ、シンボルのタイムラインの変わった所と、塊を書く）
    fn symbol_chunk(&mut self, out: &mut impl std::io::Write, c: &Chunk<SymbolDef>, written: &mut u64) -> std::io::Result<u64> {
        let key = Arc::as_ptr(c) as usize;
        if let Some((id, _)) = self.symbol_chunks.ids.get(&key) {
            return Ok(*id);
        }
        let mut symbols = Vec::with_capacity(c.len());
        for sym in c.iter() {
            let timeline = self.timeline(out, &sym.timeline, written)?;
            symbols.push(JournalSymbol { id: sym.id, name: sym.name.clone(), timeline });
        }
        let id = self.new_id();
        *written += write_record(out, &Record::Symbols { id, symbols })?;
        self.symbol_chunks.ids.insert(key, (id, c.clone()));
        Ok(id)
    }

    /// ライブラリの見せる順の塊 `c` の番号（まだ書いていなければ書く）
    fn order_chunk(&mut self, out: &mut impl std::io::Write, c: &Chunk<Slot>, written: &mut u64) -> std::io::Result<u64> {
        let key = Arc::as_ptr(c) as usize;
        if let Some((id, _)) = self.order_chunks.ids.get(&key) {
            return Ok(*id);
        }
        let id = self.new_id();
        *written += write_record(out, &Record::Order { id, ids: c.iter().map(|s| s.0).collect() })?;
        self.order_chunks.ids.insert(key, (id, c.clone()));
        Ok(id)
    }

    /// タイムライン `tl` の目録の形（レイヤーのキーフレームの並びのうち、まだ書いていないものを書く）
    fn timeline(&mut self, out: &mut impl std::io::Write, tl: &Timeline, written: &mut u64) -> std::io::Result<JournalTimeline> {
        let mut layers = Vec::with_capacity(tl.layers.len());
        for l in &tl.layers {
            let keyframes = self.keyframe_list(out, &l.keyframes, written)?;
            layers.push(JournalLayer { header: SavedLayer::header(l), keyframes });
        }
        Ok(JournalTimeline { header: SavedTimeline::header(tl), layers })
    }

    /// キーフレームの並び `list` の番号（まだ書いていなければ、新しい塊と並びを書く）
    fn keyframe_list(&mut self, out: &mut impl std::io::Write, list: &ChunkedList<Keyframe>, written: &mut u64) -> std::io::Result<u64> {
        let key = list.list_address();
        if let Some((id, _)) = self.keyframe_lists.ids.get(&key) {
            return Ok(*id);
        }
        let mut chunks = Vec::new();
        for (c, base) in list.chunks_with_bases() {
            chunks.push((self.chunk(out, c, written)?, base));
        }
        let id = self.new_id();
        *written += write_record(out, &Record::Keyframes { id, chunks })?;
        self.keyframe_lists.ids.insert(key, (id, list.clone()));
        Ok(id)
    }

    /// キーフレームの塊 `c` の番号（まだ書いていなければ、その塊と、塊が使う新しい地図の塊を書く。地図の塊は塊ごとに
    /// 書くので、全体を書くときも、一時に持つのは1つの塊が使う分だけ）
    fn chunk(&mut self, out: &mut impl std::io::Write, c: &Chunk<Keyframe>, written: &mut u64) -> std::io::Result<u64> {
        let key = Arc::as_ptr(c) as usize;
        if let Some((id, _)) = self.chunks.ids.get(&key) {
            return Ok(*id);
        }
        let rels: Vec<i64> = c.iter().map(|kf| kf.rel()).collect();
        let keyframes: Vec<SavedKeyframe> = c
            .iter()
            .map(|kf| {
                let mut s = SavedKeyframe::new(0, kf);
                for m in &mut s.maps {
                    m.write_tiles(&mut self.tiles);
                }
                s
            })
            .collect();
        let (colors, tiles, strokes) = self.tiles.appended(self.done.0, self.done.1);
        self.done = self.tiles.counts();
        if !tiles.is_empty() || !colors.is_empty() {
            *written += write_record(out, &Record::Tiles { colors, tiles, strokes })?;
        }
        let id = self.new_id();
        *written += write_record(out, &Record::Chunk { id, rels, keyframes })?;
        self.chunks.ids.insert(key, (id, c.clone()));
        Ok(id)
    }
}

/// 「保存した」の印の記録のバイト（復旧ファイルの終わりがこれか：`recovery_journal::is_saved`）
pub(crate) fn saved_mark_bytes() -> Vec<u8> {
    let mut out = Vec::new();
    write_record(&mut out, &Record::Saved).expect("並びに書くのは失敗しない");
    out
}

/// 記録の枠の大きさ：中身の長さ（u32）・中身の CRC32（u32）・種類（u8）・番号（u64）。数はリトルエンディアン
const FRAME_LEN: u64 = 17;

/// 記録 `rec` を枠（中身の長さ・中身の CRC32・種類・番号）と中身（postcard）で書く。書いたバイト数
fn write_record(out: &mut impl std::io::Write, rec: &Record) -> std::io::Result<u64> {
    let body = postcard::to_allocvec(rec).map_err(std::io::Error::other)?;
    let len = u32::try_from(body.len()).map_err(std::io::Error::other)?;
    out.write_all(&len.to_le_bytes())?;
    out.write_all(&crc32fast::hash(&body).to_le_bytes())?;
    out.write_all(&[rec.kind().byte()])?;
    out.write_all(&rec.id().to_le_bytes())?;
    out.write_all(&body)?;
    Ok(FRAME_LEN + u64::from(len))
}

/// 作品 `doc` を作品ファイル `target` に書く（記録の列で全部を書く：復旧ファイルと同じ書き手 `RecordWriter`。一時ファイルに
/// 書いてから置き換える：`write_atomically`）
pub(crate) fn write_project_file(target: &Path, doc: &Document) -> Result<(), String> {
    use std::io::Write;
    write_atomically(target, |file| {
        let mut out = std::io::BufWriter::new(file);
        out.write_all(&file_identity(SaveKind::Project))?;
        RecordWriter::default().write(&mut out, doc, None)?;
        out.into_inner().map_err(|e| e.into_error())
    })
    .map_err(|e| format!("ファイルの保存に失敗しました: {e}"))
}

/// テスト用：保存の型 `pd`（塊の表は `colors`・`tiles`・`strokes`）を、中身を確かめずに作品ファイルの記録の列にする（テストで
/// 壊れた中身のファイルを作る。本番の書き手 `RecordWriter` は作品から書くので、壊れた中身は書けない）。記録の型・目録の形・
/// 枠は本番のもの（`Record`・`Manifest`・`write_record`）をそのまま使う。キーフレームは、レイヤーごとに1つの塊（基準 0）
#[cfg(test)]
pub(crate) fn saved_project_bytes<T>(
    pd: &ProjectDocument<T>,
    colors: Vec<[u8; 4]>,
    tiles: Vec<SavedTile>,
    strokes: Vec<SavedStroke>,
) -> Vec<u8> {
    let mut out = file_identity(SaveKind::Project).to_vec();
    let put = |out: &mut Vec<u8>, rec: Record| {
        write_record(out, &rec).expect("並びに書くのは失敗しない");
    };
    let mut next = 1u64;
    let mut new_id = || {
        next += 1;
        next - 1
    };
    put(&mut out, Record::Tiles { colors, tiles, strokes });
    let mut sound_ids = Vec::new();
    for s in &pd.sounds {
        let data = new_id();
        put(&mut out, Record::SoundBytes { id: data, bytes: s.bytes.clone() });
        put(&mut out, Record::Sound(SoundEntry { id: s.id, name: s.name.clone(), bytes: data }));
        sound_ids.push(s.id);
    }
    let sounds = new_id();
    put(&mut out, Record::Sounds { id: sounds, sounds: sound_ids });
    let mut timeline = |out: &mut Vec<u8>, tl: &SavedTimeline| -> JournalTimeline {
        let mut layers = Vec::new();
        for l in &tl.layers {
            let chunk = new_id();
            let rels = l.keyframes.iter().map(|k| k.start_frame as i64).collect();
            put(out, Record::Chunk { id: chunk, rels, keyframes: l.keyframes.clone() });
            let list = new_id();
            put(out, Record::Keyframes { id: list, chunks: vec![(chunk, 0)] });
            layers.push(JournalLayer { header: SavedLayer { keyframes: Vec::new(), ..l.clone() }, keyframes: list });
        }
        JournalTimeline { header: SavedTimeline { layers: Vec::new(), ..tl.clone() }, layers }
    };
    let root = timeline(&mut out, &pd.timeline);
    let symbols: Vec<JournalSymbol> = pd
        .library
        .iter()
        .map(|s| JournalSymbol { id: s.id, name: s.name.clone(), timeline: timeline(&mut out, &s.timeline) })
        .collect();
    let (library, order) = if symbols.is_empty() {
        (Vec::new(), Vec::new())
    } else {
        let ids: Vec<u64> = symbols.iter().map(|s| s.id).collect();
        let chunk = new_id();
        put(&mut out, Record::Symbols { id: chunk, symbols });
        let order = new_id();
        put(&mut out, Record::Order { id: order, ids });
        (vec![chunk], vec![order])
    };
    put(
        &mut out,
        Record::Manifest(Manifest {
            stage_size: pd.stage_size,
            fps: pd.timeline.fps,
            next_symbol_id: pd.next_symbol_id,
            next_group_id: pd.next_group_id,
            next_sound_id: pd.next_sound_id,
            next_layer_id: pd.next_layer_id,
            recovery_source: pd.recovery_source.clone(),
            timeline: root,
            library,
            order,
            sounds,
        }),
    );
    out
}

/// 大きな記録（塊の表・キーフレームの塊・音のデータ）の場所：中身の始まり・長さ・CRC32・種類・番号
struct BigRecord {
    at: u64,
    len: u64,
    crc: u32,
    kind: RecordKind,
    id: u64,
}

/// 作品ファイル・復旧ファイル `path` を読み（読み手はこれ1つ：設計審査 01e4283 の D-3）、ファイルの種類と、最後の完全な目録の
/// 作品の保存の形にする。欠けた記録（書いている途中で止まった）から後は読まない。目録が1つも無い・目録が無い塊を指すなら
/// `Err`。ファイル全体をメモリに持たない：1回目は記録を順に流して
/// CRC32 を確かめ、小さな記録は読み、大きな記録（塊の表・キーフレームの塊・音のデータ）は場所だけ覚える。2回目に、塊の
/// 表と、最後の目録の作品が指すキーフレームの塊・音のデータだけを、作品ファイルと同じ読み手（`read_streamed`）で読む
/// （置き換わった古い記録は持たない：設計審査 b7208d9 の 6.。前はファイル全体を読み、古い記録まで全部持った）
pub(crate) fn read_file(path: &Path) -> Result<(SaveKind, ProjectDocument<ReadTiles>), String> {
    use std::io::{Read, Seek, SeekFrom};
    let failed = read_failed;
    let file = std::fs::File::open(path).map_err(failed)?;
    let size = file.metadata().map_err(failed)?.len();
    let mut file = std::io::BufReader::with_capacity(READ_CHUNK_LEN, file);
    let id_len = IDENTITY_LEN;
    let mut head = Vec::with_capacity(id_len);
    (&mut file).take(id_len as u64).read_to_end(&mut head).map_err(failed)?;
    let kind = read_identity(&head)?;
    let broken = || match kind {
        SaveKind::Project => "ファイルが壊れています".to_owned(),
        SaveKind::Journal => "復旧ファイルが壊れています".to_owned(),
    };
    let mut sounds: HashMap<u64, SoundEntry> = HashMap::new();
    let mut sound_lists: HashMap<u64, Vec<u64>> = HashMap::new();
    let mut keyframe_lists: HashMap<u64, Vec<(u64, i64)>> = HashMap::new();
    let mut symbol_chunks: HashMap<u64, Vec<JournalSymbol>> = HashMap::new();
    let mut order_chunks: HashMap<u64, Vec<u64>> = HashMap::new();
    let mut big: Vec<BigRecord> = Vec::new();
    let mut manifest = None;
    // 1回目：記録を順に流す（記録の長さは、ファイルの残りより長ければ欠けた記録として止まる：書いてある長さで取らない）
    let mut at = id_len as u64;
    while size.saturating_sub(at) >= FRAME_LEN {
        let mut frame = [0u8; FRAME_LEN as usize];
        if file.read_exact(&mut frame).is_err() {
            break;
        }
        let len = u64::from(u32::from_le_bytes(frame[..4].try_into().expect("4 バイト")));
        let crc = u32::from_le_bytes(frame[4..8].try_into().expect("4 バイト"));
        let Some(rec_kind) = RecordKind::of_byte(frame[8]) else {
            break;
        };
        let rec_id = u64::from_le_bytes(frame[9..17].try_into().expect("8 バイト"));
        if len == 0 || len > size - at - FRAME_LEN {
            break;
        }
        let mut body = BodyReader::new((&mut file).take(len));
        let mut small = Vec::new();
        let read = if rec_kind.is_big() {
            std::io::copy(&mut body, &mut std::io::sink()).map(drop)
        } else {
            body.read_to_end(&mut small).map(drop)
        };
        let (Ok(()), Ok((got, sum))) = (read, body.finish()) else {
            break;
        };
        if got != len || sum != crc {
            break;
        }
        if rec_kind.is_big() {
            big.push(BigRecord { at: at + FRAME_LEN, len, crc, kind: rec_kind, id: rec_id });
        } else {
            let Ok(rec) = postcard::from_bytes::<Record>(&small) else {
                break;
            };
            // 枠の種類・番号と、中身の記録が同じか（違えば、そこから後は読まない）
            if rec.kind() != rec_kind || rec.id() != rec_id {
                break;
            }
            match rec {
                Record::Sound(s) => {
                    sounds.insert(s.id, s);
                }
                Record::Sounds { id, sounds: list } => {
                    sound_lists.insert(id, list);
                }
                Record::Keyframes { id, chunks: list } => {
                    keyframe_lists.insert(id, list);
                }
                Record::Symbols { id, symbols } => {
                    symbol_chunks.insert(id, symbols);
                }
                Record::Order { id, ids } => {
                    order_chunks.insert(id, ids);
                }
                Record::Manifest(m) => manifest = Some((m, at)),
                Record::Saved | Record::Tiles { .. } | Record::Chunk { .. } | Record::SoundBytes { .. } => {}
            }
        }
        at += FRAME_LEN + len;
    }
    let (m, manifest_at) = manifest.ok_or_else(broken)?;
    // 最後の目録の作品が指すキーフレームの塊と音のデータ
    let timelines = std::iter::once(&m.timeline).chain(
        m.library.iter().filter_map(|id| symbol_chunks.get(id)).flatten().map(|s| &s.timeline),
    );
    let wanted_chunks: std::collections::HashSet<u64> = timelines
        .flat_map(|t| t.layers.iter())
        .filter_map(|l| keyframe_lists.get(&l.keyframes))
        .flatten()
        .map(|&(id, _)| id)
        .collect();
    let wanted_bytes: std::collections::HashSet<u64> = sound_lists
        .get(&m.sounds)
        .into_iter()
        .flatten()
        .filter_map(|id| sounds.get(id))
        .map(|s| s.bytes)
        .collect();
    // 2回目：塊の表（番号が続くので、目録までのものを全部）と、指されている大きな記録だけを読む
    let mut tiles = ReadTiles::default();
    let mut chunks: HashMap<u64, (Vec<i64>, Vec<SavedKeyframe>)> = HashMap::new();
    let mut sound_bytes: HashMap<u64, Arc<[u8]>> = HashMap::new();
    for r in big.iter().filter(|r| r.at < manifest_at) {
        let wanted = match r.kind {
            RecordKind::Chunk => wanted_chunks.contains(&r.id),
            RecordKind::SoundBytes => wanted_bytes.contains(&r.id),
            _ => true,
        };
        if !wanted {
            continue;
        }
        file.seek(SeekFrom::Start(r.at)).map_err(failed)?;
        let streamed = read_streamed::<Record>((&mut file).take(r.len))?;
        // 1回目に確かめた中身と同じか（2回のあいだにファイルが変わっていても、違う中身を読まない）
        if streamed.len != r.len || streamed.crc != r.crc || streamed.left != 0 {
            return Err(broken());
        }
        let rec = streamed.value.map_err(|_| broken())?;
        if rec.kind() != r.kind || rec.id() != r.id {
            return Err(broken());
        }
        match rec {
            Record::Tiles { colors, tiles: t, strokes } => tiles.add(colors, t, &strokes),
            Record::Chunk { id, rels, keyframes } => {
                chunks.insert(id, (rels, keyframes));
            }
            Record::SoundBytes { id, bytes } => {
                sound_bytes.insert(id, bytes);
            }
            _ => return Err(broken()),
        }
    }
    let timeline = |jt: &JournalTimeline| -> Option<SavedTimeline> {
        let mut layers = Vec::with_capacity(jt.layers.len());
        for l in &jt.layers {
            let mut keyframes = Vec::new();
            for (id, base) in keyframe_lists.get(&l.keyframes)? {
                let (rels, kfs) = chunks.get(id)?;
                for (rel, kf) in rels.iter().zip(kfs) {
                    let mut kf = kf.clone();
                    kf.start_frame = usize::try_from(base + rel).ok()?;
                    keyframes.push(kf);
                }
            }
            layers.push(SavedLayer { keyframes, ..l.header.clone() });
        }
        Some(SavedTimeline { layers, ..jt.header.clone() })
    };
    let root = SavedTimeline { fps: m.fps, ..timeline(&m.timeline).ok_or_else(broken)? };
    // シンボルを番号で引けるようにしてから、見せる順に並べる
    let mut by_id: HashMap<u64, &JournalSymbol> = HashMap::new();
    for id in &m.library {
        for s in symbol_chunks.get(id).ok_or_else(broken)? {
            by_id.insert(s.id, s);
        }
    }
    let mut library = Vec::with_capacity(by_id.len());
    for id in &m.order {
        for sym_id in order_chunks.get(id).ok_or_else(broken)? {
            let s = by_id.get(sym_id).ok_or_else(broken)?;
            let timeline = timeline(&s.timeline).ok_or_else(broken)?;
            library.push(SavedSymbolDef { id: s.id, name: s.name.clone(), timeline });
        }
    }
    let sounds = sound_lists
        .get(&m.sounds)
        .ok_or_else(broken)?
        .iter()
        .map(|id| {
            let s = sounds.get(id)?;
            Some(SavedSound { id: s.id, name: s.name.clone(), bytes: sound_bytes.get(&s.bytes)?.clone() })
        })
        .collect::<Option<Vec<_>>>()
        .ok_or_else(broken)?;
    Ok((kind, ProjectDocument {
        stage_size: m.stage_size,
        timeline: root,
        library,
        next_symbol_id: m.next_symbol_id,
        next_group_id: m.next_group_id,
        recovery_source: m.recovery_source,
        map_tiles: tiles,
        sounds,
        next_sound_id: m.next_sound_id,
        next_layer_id: m.next_layer_id,
    }))
}
