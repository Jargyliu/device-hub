//! MCHOSE 磁気軸キーボード (M HUB の "GLW" SDK 系) — ACE68 GT (3837:3007)。
//!
//! 出典 (docs/PROTOCOL_NOTES.md §10):
//! - 公式 M HUB の配布 SDK `cizhou/CZ_SHARED_DATA/main.*.js` (GLWDeviceSDK・channel) —
//!   コマンド番号・オフセット・レコード形式・機種定義 (VID/PID・設定 IF・軸・精度)。
//! - mansur54321/mchose-hub-rust (MIT) — 同じ SDK の Ace 75 8K 実装 (実機で読み取り・
//!   照明書き込みを確認済み)。SDK に名前の無い値 (ポーリング符号・keyMode・SOCD 優先・
//!   マクロの実行モード・テレメトリ単位) をここで照合した。コードは書き写していない。
//!
//! **ACE68 GT の実機では未確認**。
//!
//! ワイヤ形式 (設定 IF = usage page 0x0001 / usage 0x0000、64B の番号なし output/input):
//! - 要求 `[0x55, cmd, 0, sum, len, offLo, offHi, 0, data…]`、
//!   `sum = (len + offLo + offHi + data…) & 0xFF`。読みは data 無し・len = 読む長さ。
//! - 応答 `[0xAA, cmd, ?, sum, len, offLo, offHi, ?, data…]` (同じ和で検証)。
//! - 1 パケット 56B まで。大きな領域は 56B ずつ (書きはレコード境界に揃える)。
//! - 自律通知: 0xA0 = 押下深さテレメトリ (キャリブレーション中 / debug 有効時)、
//!   0xA1 = プロファイル切替、0xA2 = リセット、0xA3 = 機能変更。
//!
//! 主なコマンド (読み / 書き):
//! 3 情報 · 4/14 プロファイル基本 · 5/6 機能 64B×profile · 7 既定キー表 · 8/9 キー表
//! (512 × (4·profile + layer)、3B×128) · 10/11 キー色 (512×profile、3B×128) ·
//! 12/13 マクロ領域 · 160/161 トリガー (1024×profile、8B×128) · 162/163 DKS (768, 24B×32) ·
//! 164/165 MT 系 (256, 6B×32) · 166/167 TGL (128, 3B×32) · 168/169/177 キャリブレーション
//! 開始/終了/再開 · 238 工場リセット。

use crate::glw_tables::{ACE68_GT_LAYOUT, GLW_ACTIONS};
use devhub_core::backend::*;
use devhub_core::model::*;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::thread::sleep;
use std::time::{Duration, Instant};

const PACKET: usize = 64;
const CHUNK: usize = 56;
const KEYS: usize = 128;
const LAYERS: usize = 4;
const SLOTS: usize = 32;
const MACROS: usize = 16;
const MIN_DELAY: u16 = 5;

mod cmd {
    pub const INFO: u8 = 3;
    pub const BASE: u8 = 4;
    pub const FUNC_GET: u8 = 5;
    pub const FUNC_SET: u8 = 6;
    pub const DEFAULT_KEYS: u8 = 7;
    pub const KEYS_GET: u8 = 8;
    pub const KEYS_SET: u8 = 9;
    pub const COLOR_GET: u8 = 10;
    pub const COLOR_SET: u8 = 11;
    pub const MACRO_GET: u8 = 12;
    pub const MACRO_SET: u8 = 13;
    pub const BASE_SET: u8 = 14;
    pub const TRIG_GET: u8 = 160;
    pub const TRIG_SET: u8 = 161;
    pub const DKS_GET: u8 = 162;
    pub const DKS_SET: u8 = 163;
    pub const MT_GET: u8 = 164;
    pub const MT_SET: u8 = 165;
    pub const TGL_GET: u8 = 166;
    pub const TGL_SET: u8 = 167;
    pub const CAL_START: u8 = 168;
    pub const CAL_END: u8 = 169;
    pub const CAL_RESTART: u8 = 177;
    pub const FACTORY: u8 = 238;
}

/// キー表のエントリ種別 (type)。
mod kt {
    pub const KEYBOARD: u8 = 16;
    pub const MACRO: u8 = 112;
    pub const DKS: u8 = 144;
    pub const TGL: u8 = 145;
    pub const MT: u8 = 146;
    pub const RS: u8 = 147;
    pub const SOCD: u8 = 148;
    pub const LT: u8 = 149;
}

// ==================================================================== 機種表

/// GLW 系 1 機種。
#[derive(Clone, Copy, Debug)]
pub struct GlwModel {
    pub model: &'static str,
    pub layout: &'static [(u8, f32, f32, f32, f32, &'static str)],
    /// 公式 switchOptions の表示名 (値 = 並び順)。
    pub switches: &'static [&'static str],
    pub travel: f32,
    /// 公式 layout の lighting.effect の並び (UI 表示順)。
    pub effects: &'static [u8],
    /// サイドライト (ライトバー) のエフェクト (値, 名前, 色, 速度)。空なら無し。
    pub strip: &'static [(u8, &'static str, bool, bool)],
}

pub const ACE68_GT: GlwModel = GlwModel {
    model: "ace68-gt",
    layout: ACE68_GT_LAYOUT,
    // 公式: ztDefault / ztPinkTsGt / ztTsGt / ztBm。
    switches: &["Default", "TS GT Pink", "TS GT", "BM"],
    travel: 3.4,
    effects: &[
        0, 6, 7, 8, 9, 4, 1, 5, 3, 2, 11, 12, 10, 18, 19, 20, 21, 22, 14, 13, 15, 16, 17,
    ],
    // 公式 layout: side-light-flowing-light-mode-0, neon-1, breathing-2, constant-on-3, off-4。
    strip: &[
        (0, "Flow", true, true),
        (1, "Neon", false, true),
        (2, "Breath", true, true),
        (3, "Constant On", true, false),
        (4, "Off", false, false),
    ],
};

const MODELS: &[&GlwModel] = &[&ACE68_GT];

/// registry の model id → 機種表 (不明なら ACE68 GT)。
pub fn model_for(id: &str) -> &'static GlwModel {
    MODELS.iter().find(|m| m.model == id).copied().unwrap_or(&ACE68_GT)
}

/// メイン照明エフェクト (値 → 名前, 色, 速度, 方向)。公式の英語名。
fn effect_meta(id: u8) -> (&'static str, bool, bool, bool) {
    match id {
        0 => ("Custom (per-key)", true, false, false),
        1 => ("Rainbow Cycle", false, true, false),
        2 => ("Linear Gradient", true, false, false),
        3 => ("Constant On", true, false, false),
        4 => ("Breath", true, true, false),
        5 => ("Disco", false, true, false),
        6 => ("Horizontal Wave", true, true, true),
        7 => ("Vertical Wave", true, true, true),
        8 => ("Center Spread", true, true, true),
        9 => ("Star Twinkle", true, true, false),
        10 => ("Center Spin", true, true, true),
        11 => ("Bottom Up", true, true, false),
        12 => ("Reciprocal Rebound", true, true, false),
        13 => ("Key Ripple", true, true, false),
        14 => ("Solid Ripple", true, true, false),
        15 => ("Key Trail", true, true, false),
        16 => ("Key Firework", true, true, false),
        17 => ("Key Beam", true, true, false),
        18 => ("Diagonal Flow", true, true, false),
        19 => ("Laser Rain", true, true, false),
        20 => ("Dot Twinkle", true, true, false),
        21 => ("Fireworks", true, true, false),
        22 => ("Triangle Bounce", true, true, false),
        _ => ("Unknown", false, false, false),
    }
}

// ==================================================================== パケット

/// 要求パケット (report ID を除く 64B)。
pub fn request(command: u8, offset: u16, len: usize, data: &[u8]) -> [u8; PACKET] {
    let mut p = [0u8; PACKET];
    let n = data.len().min(CHUNK);
    p[0] = 0x55;
    p[1] = command;
    p[4] = len.min(CHUNK) as u8;
    p[5] = offset as u8;
    p[6] = (offset >> 8) as u8;
    p[8..8 + n].copy_from_slice(&data[..n]);
    p[3] = p[4..8 + n].iter().fold(0u8, |a, &b| a.wrapping_add(b));
    p
}

/// 応答の検証。`Some(payload)` = このコマンド・オフセットへの正しい応答。
pub fn parse_response(raw: &[u8], command: u8, offset: u16) -> Option<Vec<u8>> {
    if raw.len() < 8 || raw[0] != 0xAA || raw[1] != command {
        return None;
    }
    if u16::from_le_bytes([raw[5], raw[6]]) != offset {
        return None;
    }
    let len = (raw[4] as usize).min(CHUNK);
    if raw.len() < 8 + len {
        return None;
    }
    let sum = raw[4..8 + len].iter().fold(0u8, |a, &b| a.wrapping_add(b));
    (sum == raw[3]).then(|| raw[8..8 + len].to_vec())
}

/// 0xA0 テレメトリ → (キー, 深さ mm)。
pub fn parse_telemetry(raw: &[u8]) -> Option<([u8; 3], f32)> {
    (raw.len() >= 11 && raw[0] == 0xA0).then(|| {
        (
            [raw[1], raw[2], raw[3]],
            u16::from_be_bytes([raw[6], raw[7]]) as f32 / 100.,
        )
    })
}

// ==================================================================== レコード

/// トリガー 8B レコード。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Trigger {
    pub switch_type: u8,
    pub key_mode: u8,
    pub priority: u8,
    pub actuation: u16,
    pub press_precision: u8,
    pub release_precision: u8,
    pub rt_press: u16,
    pub rt_release: u16,
    pub press_deadzone: u8,
    pub release_deadzone: u8,
    /// 未定義ビットの保存用 (レコード先頭バイトの上位ニブル)。
    pub head: u8,
}

impl Trigger {
    pub fn decode(b: &[u8]) -> Self {
        Self {
            head: b[0] & 0xF0,
            switch_type: b[0] & 15,
            key_mode: b[1] & 15,
            priority: b[1] >> 4,
            actuation: (b[2] as u16 | ((b[3] as u16 & 1) << 8)) + 1,
            press_precision: (b[3] >> 3) & 3,
            release_precision: (b[3] >> 1) & 3,
            rt_press: (b[4] as u16 | ((b[5] as u16 & 1) << 8)) + 1,
            rt_release: (b[6] as u16 | ((b[7] as u16 & 1) << 8)) + 1,
            press_deadzone: (b[5] >> 1) & 127,
            release_deadzone: (b[7] >> 1) & 127,
        }
    }

    pub fn encode(&self) -> [u8; 8] {
        let a = self.actuation.clamp(1, 512) - 1;
        let p = self.rt_press.clamp(1, 512) - 1;
        let r = self.rt_release.clamp(1, 512) - 1;
        [
            // 公式は 160|switchType を書く (上位ニブル 0xA)。読んだ値があればそれを保つ。
            (if self.head == 0 { 0xA0 } else { self.head }) | (self.switch_type & 15),
            (self.priority << 4) | (self.key_mode & 15),
            a as u8,
            ((a >> 8) as u8 & 1) | ((self.press_precision & 3) << 3) | ((self.release_precision & 3) << 1),
            p as u8,
            ((p >> 8) as u8 & 1) | ((self.press_deadzone.min(127)) << 1),
            r as u8,
            ((r >> 8) as u8 & 1) | ((self.release_deadzone.min(127)) << 1),
        ]
    }
}

fn precision_base(p: u8) -> f32 {
    match p {
        1 => 200.,
        2 => 1000.,
        _ => 100.,
    }
}

/// mm → (raw, precision)。0.01 で表せない値は細かい精度を使う (0.5mm 以下のみ)。
fn rt_raw(mm: f32) -> (u16, u8) {
    let r100 = (mm * 100.).round();
    if ((mm * 100.) - r100).abs() < 0.01 || mm > 0.5 {
        return ((r100 as u16).clamp(2, 512), 0);
    }
    let r1000 = (mm * 1000.).round() as u16;
    (r1000.clamp(2, 500), 2)
}

/// DKS 24B レコード。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DksRec {
    pub points: [u8; 4],
    pub actions: [[u8; 3]; 4],
    /// アクションごと・区切りごとのマスク (0..7, 最後だけ 0..1)。
    pub masks: [[u8; 4]; 4],
    pub reserved: [u16; 4],
}

impl DksRec {
    pub fn decode(b: &[u8]) -> Self {
        let mut r = Self {
            points: [b[0], b[1], b[2], b[3]],
            actions: [[0; 3]; 4],
            masks: [[0; 4]; 4],
            reserved: [0; 4],
        };
        for i in 0..4 {
            let s = 4 + i * 5;
            r.actions[i] = [b[s], b[s + 1], b[s + 2]];
            let w = u16::from_le_bytes([b[s + 3], b[s + 4]]);
            r.masks[i] = [
                (w & 7) as u8,
                ((w >> 3) & 7) as u8,
                ((w >> 6) & 7) as u8,
                ((w >> 9) & 1) as u8,
            ];
            r.reserved[i] = w & !0x3FF;
        }
        r
    }

    pub fn encode(&self) -> [u8; 24] {
        let mut b = [0u8; 24];
        b[..4].copy_from_slice(&self.points);
        for i in 0..4 {
            let s = 4 + i * 5;
            b[s..s + 3].copy_from_slice(&self.actions[i]);
            let m = self.masks[i];
            let w = self.reserved[i]
                | (m[0] as u16 & 7)
                | ((m[1] as u16 & 7) << 3)
                | ((m[2] as u16 & 7) << 6)
                | ((m[3] as u16 & 1) << 9);
            b[s + 3..s + 5].copy_from_slice(&w.to_le_bytes());
        }
        b
    }
}

/// DKS のマスク ⇄ フェーズ。公式の「温度計」符号 (1 区間 = 0b001、2 = 0b011、3 = 0b111)。
/// Tap = その区切りで押して離す (1 区間)、Hold = 次の区切りまで押し続ける (2 区間)。
fn dks_phase(mask: u8) -> DksPhase {
    match mask {
        0 => DksPhase::Off,
        1 => DksPhase::Tap,
        _ => DksPhase::Hold,
    }
}

fn dks_mask(p: DksPhase, last: bool) -> u8 {
    match (p, last) {
        (DksPhase::Off, _) => 0,
        (_, true) => 1,
        (DksPhase::Tap, false) => 1,
        (DksPhase::Hold, false) => 3,
    }
}

// ==================================================================== キーコード

/// (type, code1, code2) → catalog コード。
fn code_of(k: [u8; 3]) -> u32 {
    (k[0] as u32) << 16 | (k[1] as u32) << 8 | k[2] as u32
}

fn key_of(code: u32) -> [u8; 3] {
    [(code >> 16) as u8, (code >> 8) as u8, code as u8]
}

const DISABLED: [u8; 3] = [kt::KEYBOARD, 0, 0];

fn catalog() -> Vec<KeyGroup> {
    let mut groups: Vec<KeyGroup> = vec![];
    for &(t, c1, c2, group, name) in GLW_ACTIONS {
        let choice = KeyChoice {
            code: code_of([t, c1, c2]),
            label: name.into(),
            short: None,
        };
        match groups.iter_mut().find(|g| g.name == group) {
            Some(g) => g.keys.push(choice),
            None => groups.push(KeyGroup {
                name: group.into(),
                keys: vec![choice],
            }),
        }
    }
    groups
}

fn is_catalog(k: [u8; 3]) -> bool {
    GLW_ACTIONS.iter().any(|&(t, a, b, _, _)| [t, a, b] == k)
}

/// キー表エントリ → KeyAction (高度キーは別扱いで None を返さない呼び出し側が判定)。
fn to_action(k: [u8; 3]) -> KeyAction {
    match k {
        DISABLED => KeyAction::None,
        [kt::MACRO, slot, _] => KeyAction::Macro(slot),
        [kt::KEYBOARD, mods, key] if mods != 0 && key != 0 && !is_catalog(k) => KeyAction::Combo {
            mods,
            code: code_of([kt::KEYBOARD, 0, key]),
        },
        _ => KeyAction::Code(code_of(k)),
    }
}

/// HID modifier の 1 ビット ⇄ usage 0xE0..0xE7。
fn mouse_bits(f: MouseFn) -> u8 {
    match f {
        MouseFn::Left => 1,
        MouseFn::Right => 2,
        MouseFn::Middle => 4,
        MouseFn::Back => 8,
        MouseFn::Forward => 16,
        _ => 1,
    }
}

fn mouse_fn(bits: u8) -> MouseFn {
    match bits {
        2 => MouseFn::Right,
        4 => MouseFn::Middle,
        8 => MouseFn::Back,
        16 => MouseFn::Forward,
        _ => MouseFn::Left,
    }
}

// ==================================================================== マクロ

/// マクロ領域 (公式 `te()`): `[0..32]` = 16 本の開始オフセット u16LE (≥64)、
/// `[32..34]` = 129,126、`[34..50]` = 実行モード、`[64..]` = 4B ワードの列。
/// ワード = `[遅延LE(2), 種別|押下0x40|終端0x80, コード]`、種別 1 = 修飾ビット、
/// 2 = キー (コード 0 なら待ちだけ)、3 = マウスボタン。遅延 < 5 は「待ち無し」。
pub fn decode_macro(bank: &[u8], slot: usize) -> Option<MacroDef> {
    if bank.len() < 64 || bank[..32].iter().all(|&b| b == 0) || bank[..32].iter().all(|&b| b == 255) {
        return None;
    }
    let off = u16::from_le_bytes([bank[slot * 2], bank[slot * 2 + 1]]) as usize;
    if off < 64 || off >= bank.len() {
        return None;
    }
    let wire_mode = if bank[32..34] == [129, 126] { bank[34 + slot] } else { 0 };
    let mut events: Vec<MacroEvent> = vec![];
    for w in bank[off..].chunks_exact(4) {
        let delay = u16::from_le_bytes([w[0], w[1]]);
        let delay = if delay >= MIN_DELAY { delay as u32 } else { 0 };
        let edge = if w[2] & 0x40 != 0 { MacroEdge::Down } else { MacroEdge::Up };
        let input = match (w[2] & 63, w[3]) {
            (1, b) if b.is_power_of_two() => Some(MacroInput::Key(0xE0 + b.trailing_zeros() as u16)),
            (2, 0) => None,
            (2, c) => Some(MacroInput::Key(c as u16)),
            (3, b) => Some(MacroInput::Mouse(mouse_fn(b))),
            _ => None,
        };
        match input {
            Some(input) => events.push(MacroEvent {
                input,
                edge,
                delay_ms: delay,
            }),
            None => {
                if let Some(last) = events.last_mut() {
                    last.delay_ms += delay;
                }
            }
        }
        if w[2] & 0x80 != 0 {
            break;
        }
    }
    if events.is_empty() {
        return None;
    }
    let (mode, repeat) = match wire_mode {
        0 => (0, 1),
        255 => (255, 1),
        n => (1, n as u16),
    };
    Some(MacroDef {
        name: String::new(),
        events,
        mode,
        repeat,
    })
}

fn encode_events(ev: &[MacroEvent]) -> Vec<u8> {
    if ev.is_empty() {
        return vec![0, 0, 0x80, 0];
    }
    let mut out: Vec<[u8; 4]> = vec![];
    for e in ev {
        let (kind, code) = match e.input {
            MacroInput::Key(u) if (0xE0..=0xE7).contains(&u) => (1u8, 1u8 << (u - 0xE0)),
            MacroInput::Key(u) => (2, u as u8),
            MacroInput::Mouse(f) => (3, mouse_bits(f)),
        };
        let down = if e.edge == MacroEdge::Down { 0x40 } else { 0 };
        // 65535ms を超える待ちは「待ちだけのワード」で足す。
        let mut d = e.delay_ms;
        let first = if d >= MIN_DELAY as u32 { d.min(65535) as u16 } else { MIN_DELAY - 1 };
        d = d.saturating_sub(first as u32);
        let f = first.to_le_bytes();
        out.push([f[0], f[1], kind | down, code]);
        while d >= MIN_DELAY as u32 {
            let x = d.min(65535) as u16;
            d -= x as u32;
            let b = x.to_le_bytes();
            out.push([b[0], b[1], 2, 0]);
        }
    }
    out.last_mut().unwrap()[2] |= 0x80;
    out.concat()
}

fn wire_mode(def: &MacroDef) -> u8 {
    match def.mode {
        0 => 0,
        255 => 255,
        _ => def.repeat.clamp(1, 254) as u8,
    }
}

/// 16 本を詰め直したマクロ領域 (ヘッダ外のバイトは元の領域を保つ)。
pub fn encode_bank(orig: &[u8], defs: &[Option<MacroDef>]) -> HubResult<Vec<u8>> {
    let mut b = orig.to_vec();
    b[32] = 129;
    b[33] = 126;
    let mut content: Vec<u8> = vec![0, 0, 0x80, 0];
    let mut streams: Vec<(Vec<u8>, usize)> = vec![(content.clone(), 64)];
    for (i, d) in defs.iter().enumerate().take(MACROS) {
        let bytes = d.as_ref().map(|d| encode_events(&d.events)).unwrap_or(vec![0, 0, 0x80, 0]);
        let off = match streams.iter().find(|(s, _)| *s == bytes) {
            Some((_, o)) => *o,
            None => {
                let o = 64 + content.len();
                content.extend_from_slice(&bytes);
                streams.push((bytes, o));
                o
            }
        };
        b[i * 2..i * 2 + 2].copy_from_slice(&(off as u16).to_le_bytes());
        b[34 + i] = d.as_ref().map(wire_mode).unwrap_or(0);
    }
    if 64 + content.len() > b.len() {
        return Err(HubError::Invalid("マクロ領域に収まりません".into()));
    }
    b[64..64 + content.len()].copy_from_slice(&content);
    Ok(b)
}

// ==================================================================== ドライバ

struct Live {
    last_call: Instant,
    debug_on: bool,
    profile: u8,
}

pub struct GlwDriver {
    dev: Arc<Mutex<hidapi::HidDevice>>,
    info: Mutex<DeviceInfo>,
    model: &'static GlwModel,
    /// マトリクス番号ごとの押下深さ (mm)。
    travel: Arc<Mutex<Vec<f32>>>,
    /// 既定キー表 layer 0 (テレメトリのキー → マトリクス番号の対応用)。
    defaults: Mutex<Option<Vec<[u8; 3]>>>,
    live: Arc<Mutex<Live>>,
}

pub fn open(dev: hidapi::HidDevice, info: DeviceInfo) -> HubResult<GlwDriver> {
    dev.set_blocking_mode(false).ok();
    Ok(GlwDriver {
        dev: Arc::new(Mutex::new(dev)),
        model: model_for(&info.model),
        info: Mutex::new(info),
        travel: Arc::new(Mutex::new(vec![0.; KEYS])),
        defaults: Mutex::new(None),
        live: Arc::new(Mutex::new(Live {
            last_call: Instant::now(),
            debug_on: false,
            profile: 0,
        })),
    })
}

/// 1 往復 (最大 4 回)。テレメトリは `travel` に取り込みつつ読み飛ばす。
fn xfer(
    dev: &hidapi::HidDevice,
    travel: &Mutex<Vec<f32>>,
    defaults: Option<&[[u8; 3]]>,
    command: u8,
    offset: u16,
    len: usize,
    data: &[u8],
) -> HubResult<Vec<u8>> {
    let p = request(command, offset, len, data);
    let mut w = [0u8; PACKET + 1];
    w[1..].copy_from_slice(&p);
    for attempt in 0..4 {
        if attempt > 0 {
            sleep(Duration::from_millis(20));
        }
        dev.write(&w)
            .map_err(|e| HubError::Io(format!("mchose write: {e}")))?;
        let deadline = Instant::now() + Duration::from_millis(800);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            let mut buf = [0u8; PACKET + 1];
            match dev.read_timeout(&mut buf, left.as_millis().min(100) as i32) {
                Ok(n) if n > 0 => {
                    let raw = &buf[..n];
                    if let Some(r) = parse_response(raw, command, offset) {
                        return Ok(r);
                    }
                    absorb_telemetry(raw, travel, defaults);
                }
                Ok(_) => {}
                Err(e) => return Err(HubError::Io(format!("mchose read: {e}"))),
            }
        }
    }
    Err(HubError::Protocol(format!(
        "mchose: no response to cmd {command} @{offset}"
    )))
}

fn absorb_telemetry(raw: &[u8], travel: &Mutex<Vec<f32>>, defaults: Option<&[[u8; 3]]>) {
    let (Some((key, mm)), Some(defs)) = (parse_telemetry(raw), defaults) else {
        return;
    };
    if let Some(i) = defs.iter().position(|d| *d == key) {
        if let Some(t) = travel.lock().unwrap_or_else(|p| p.into_inner()).get_mut(i) {
            *t = mm;
        }
    }
}

impl GlwDriver {
    fn io<T>(&self, f: impl FnOnce(&hidapi::HidDevice, Option<&[[u8; 3]]>) -> T) -> T {
        let defs = self.defaults.lock().unwrap_or_else(|p| p.into_inner()).clone();
        let dev = self.dev.lock().unwrap_or_else(|p| p.into_inner());
        f(&dev, defs.as_deref())
    }

    fn read_range(&self, command: u8, offset: usize, len: usize) -> HubResult<Vec<u8>> {
        let mut out = Vec::with_capacity(len);
        while out.len() < len {
            let n = (len - out.len()).min(CHUNK);
            let off = offset + out.len();
            let r = self.io(|d, defs| xfer(d, &self.travel, defs, command, off as u16, n, &[]))?;
            if r.len() < n {
                return Err(HubError::Protocol(format!(
                    "mchose: short read cmd {command} @{off} ({} of {n})",
                    r.len()
                )));
            }
            out.extend_from_slice(&r[..n]);
        }
        Ok(out)
    }

    /// `rec` バイトのレコード境界に揃えて書く。`old` があれば変わったレコードだけ送る。
    fn write_range(&self, command: u8, offset: usize, data: &[u8], rec: usize, old: Option<&[u8]>) -> HubResult<()> {
        let per = if rec >= CHUNK { CHUNK } else { (CHUNK / rec) * rec };
        let mut i = 0;
        while i < data.len() {
            let end = (i + per).min(data.len());
            let changed = old.is_none_or(|o| o.get(i..end) != Some(&data[i..end]));
            if changed {
                let off = offset + i;
                self.io(|d, defs| xfer(d, &self.travel, defs, command, off as u16, end - i, &data[i..end]))?;
            }
            i = end;
        }
        Ok(())
    }

    fn info_raw(&self) -> HubResult<Vec<u8>> {
        self.io(|d, defs| xfer(d, &self.travel, defs, cmd::INFO, 0, CHUNK, &[]))
    }

    /// (有効プロファイル, 有効数, 並び, 並び替え対応)。
    fn base(&self) -> HubResult<(u8, u8, Vec<u8>, Vec<u8>)> {
        let raw = self.read_range(cmd::BASE, 0, CHUNK)?;
        let mut sorted = raw[2..6].to_vec();
        sorted.sort_unstable();
        let ordered = sorted == [0, 1, 2, 3];
        let (active, count, order) = if ordered && raw[0] < 4 && raw[1] <= 4 && raw[1] > 0 {
            (raw[2 + raw[0] as usize], raw[1], raw[2..6].to_vec())
        } else if raw[0] < 4 {
            (raw[0], 2, vec![0, 1])
        } else {
            (0, 1, vec![0])
        };
        Ok((active, count, order, raw))
    }

    fn profile(&self) -> HubResult<u8> {
        Ok(self.base()?.0)
    }

    fn func(&self, p: u8) -> HubResult<Vec<u8>> {
        self.read_range(cmd::FUNC_GET, 64 * p as usize, 64)
    }

    fn write_func(&self, p: u8, old: &[u8], new: &[u8]) -> HubResult<()> {
        self.write_range(cmd::FUNC_SET, 64 * p as usize, new, 64, None)?;
        let back = self.func(p)?;
        // 書き込んだ範囲 (電池など本体が更新するバイトは除く) を照合。
        let watched = [1usize, 4, 5, 6, 7, 8, 9, 10, 11, 12, 14, 15, 16, 20, 24, 25, 26, 27, 29, 30, 31, 35, 36];
        if watched.iter().any(|&i| back[i] != new[i] && new[i] != old[i]) {
            return Err(HubError::Protocol("mchose: 設定が反映されませんでした".into()));
        }
        Ok(())
    }

    fn keymap(&self, p: u8, layer: usize) -> HubResult<Vec<[u8; 3]>> {
        let raw = self.read_range(cmd::KEYS_GET, 512 * (4 * p as usize + layer), KEYS * 3)?;
        Ok(raw.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect())
    }

    fn default_keymap(&self, p: u8, layer: usize) -> HubResult<Vec<[u8; 3]>> {
        let raw = self.read_range(cmd::DEFAULT_KEYS, 512 * (4 * p as usize + layer), KEYS * 3)?;
        Ok(raw.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect())
    }

    fn write_keymap(&self, p: u8, layer: usize, old: &[[u8; 3]], new: &[[u8; 3]]) -> HubResult<()> {
        let o: Vec<u8> = old.concat();
        let n: Vec<u8> = new.concat();
        self.write_range(cmd::KEYS_SET, 512 * (4 * p as usize + layer), &n, 3, Some(&o))
    }

    fn triggers(&self, p: u8) -> HubResult<Vec<Trigger>> {
        let raw = self.read_range(cmd::TRIG_GET, 1024 * p as usize, KEYS * 8)?;
        Ok(raw.chunks_exact(8).map(Trigger::decode).collect())
    }

    fn write_triggers(&self, p: u8, old: &[Trigger], new: &[Trigger]) -> HubResult<()> {
        let o: Vec<u8> = old.iter().flat_map(|t| t.encode()).collect();
        let n: Vec<u8> = new.iter().flat_map(|t| t.encode()).collect();
        self.write_range(cmd::TRIG_SET, 1024 * p as usize, &n, 8, Some(&o))
    }

    fn colors(&self, p: u8) -> HubResult<Vec<u8>> {
        self.read_range(cmd::COLOR_GET, 512 * p as usize, KEYS * 3)
    }

    fn ensure_defaults(&self) {
        if self.defaults.lock().unwrap_or_else(|p| p.into_inner()).is_some() {
            return;
        }
        if let Ok(d) = self.default_keymap(0, 0) {
            *self.defaults.lock().unwrap_or_else(|p| p.into_inner()) = Some(d);
        }
    }

    /// マクロ領域の (オフセット, 大きさ)。並び替え対応 (4 プロファイル) なら共有 8KB。
    fn macro_area(&self, p: u8) -> HubResult<(usize, usize)> {
        let (_, _, _, raw) = self.base()?;
        let mut s = raw[2..6].to_vec();
        s.sort_unstable();
        Ok(if s == [0, 1, 2, 3] {
            (0, 8192)
        } else {
            (2048 * p as usize, 2048)
        })
    }

    /// layout index → マトリクス番号。
    fn matrix(&self, layout: usize) -> HubResult<usize> {
        self.model
            .layout
            .get(layout)
            .map(|k| k.0 as usize)
            .ok_or_else(|| HubError::Invalid("key index".into()))
    }

    fn layout_of(&self, matrix: usize) -> Option<usize> {
        self.model.layout.iter().position(|k| k.0 as usize == matrix)
    }

    /// 全レイヤーで参照されている高度キースロット (DKS, TGL, MT 系)。
    fn used_slots(&self, p: u8) -> HubResult<([bool; SLOTS], [bool; SLOTS], [bool; SLOTS])> {
        let mut dks = [false; SLOTS];
        let mut tgl = [false; SLOTS];
        let mut mt = [false; SLOTS];
        for l in 0..LAYERS {
            for k in self.keymap(p, l)? {
                let s = k[1] as usize;
                if s >= SLOTS {
                    continue;
                }
                match k[0] {
                    kt::DKS => dks[s] = true,
                    kt::TGL => tgl[s] = true,
                    kt::MT | kt::RS | kt::SOCD | kt::LT => mt[s] = true,
                    _ => {}
                }
            }
        }
        Ok((dks, tgl, mt))
    }

    fn perf_of(&self, t: &Trigger) -> KeyPerf {
        KeyPerf {
            mode: if t.key_mode == 0 {
                TriggerMode::Normal
            } else {
                TriggerMode::Rapid
            },
            actuation: t.actuation as f32 / 100.,
            release: None,
            rt_first: t.actuation as f32 / 100.,
            rt_press: t.rt_press as f32 / precision_base(t.press_precision),
            rt_release: t.rt_release as f32 / precision_base(t.release_precision),
            top_deadzone: t.press_deadzone as f32 / 100.,
            bottom_deadzone: t.release_deadzone as f32 / 100.,
            continuous: t.key_mode == 2,
            switch_type: t.switch_type,
        }
    }

    fn apply_perf(&self, t: &mut Trigger, k: &KeyPerf) {
        let max = (self.model.travel * 100.) as u16;
        t.key_mode = match (k.mode, k.continuous) {
            (TriggerMode::Normal, _) => 0,
            (TriggerMode::Rapid, false) => 1,
            (TriggerMode::Rapid, true) => 2,
        };
        t.actuation = ((k.actuation * 100.).round() as u16).clamp(10, max);
        let (p, pp) = rt_raw(k.rt_press);
        let (r, rp) = rt_raw(k.rt_release);
        t.rt_press = p;
        t.press_precision = pp;
        t.rt_release = r;
        t.release_precision = rp;
        t.press_deadzone = ((k.top_deadzone * 100.).round() as u8).min(50);
        t.release_deadzone = ((k.bottom_deadzone * 100.).round() as u8).min(50);
        t.switch_type = k.switch_type.min(self.model.switches.len().saturating_sub(1) as u8);
    }

    fn lighting_state(&self, f: &[u8], area: u8, colors: &[u8]) -> LightingState {
        let rgb = |i: usize| Rgb::new(f[i], f[i + 1], f[i + 2]);
        if area == 0 {
            let mut per_key = BTreeMap::new();
            for (li, k) in self.model.layout.iter().enumerate() {
                let m = k.0 as usize * 3;
                if let Some(c) = colors.get(m..m + 3) {
                    if c != [0, 0, 0] {
                        per_key.insert(li, Rgb::new(c[0], c[1], c[2]));
                    }
                }
            }
            LightingState {
                on: f[9] > 0,
                effect: f[8],
                color: rgb(14),
                brightness: f[9],
                speed: 4u8.saturating_sub(f[10]),
                direction: f[11],
                per_key,
                zone_on: vec![],
                palette: vec![],
                palette_sel: 0,
                color_correction: None,
            }
        } else {
            LightingState {
                on: f[24] != 4 && f[25] > 0,
                effect: f[24],
                color: rgb(29),
                brightness: f[25],
                speed: 4u8.saturating_sub(f[26]),
                direction: 0,
                per_key: BTreeMap::new(),
                zone_on: vec![],
                palette: vec![],
                palette_sel: 0,
                color_correction: None,
            }
        }
    }

    /// layout 上の高度キー (ペアは小さい方だけ)。
    fn advanced_of(
        &self,
        layer: u8,
        map: &[[u8; 3]],
        dks: &[u8],
        mt: &[u8],
        tgl: &[u8],
        trig: &[Trigger],
    ) -> Vec<AdvancedKey> {
        let mut out = vec![];
        for (li, k) in self.model.layout.iter().enumerate() {
            let m = k.0 as usize;
            let e = map[m];
            let s = e[1] as usize;
            if s >= SLOTS {
                continue;
            }
            let mt_rec = |slot: usize| -> ([u8; 3], [u8; 3]) {
                let b = &mt[slot * 6..slot * 6 + 6];
                ([b[0], b[1], b[2]], [b[3], b[4], b[5]])
            };
            let binding = match e[0] {
                kt::DKS => {
                    let r = DksRec::decode(&dks[s * 24..s * 24 + 24]);
                    let rows = (0..4)
                        .filter(|&i| r.actions[i] != [0, 0, 0])
                        .map(|i| DksRow {
                            code: code_of(r.actions[i]),
                            phases: [
                                dks_phase(r.masks[i][0]),
                                dks_phase(r.masks[i][1]),
                                dks_phase(r.masks[i][2]),
                                dks_phase(r.masks[i][3]),
                            ],
                        })
                        .collect();
                    Some(Advanced::Dks {
                        rows,
                        bottom: r.points[1] as f32 / 10.,
                    })
                }
                kt::TGL => {
                    let b = &tgl[s * 3..s * 3 + 3];
                    Some(Advanced::Tgl {
                        code: code_of([b[0], b[1], b[2]]),
                        delay_ms: 0,
                    })
                }
                kt::MT => {
                    let (tap, hold) = mt_rec(s);
                    Some(Advanced::Mt {
                        tap: code_of(tap),
                        hold: code_of(hold),
                        hold_ms: e[2] as u16 * 10,
                    })
                }
                kt::RS | kt::SOCD => {
                    let partner = e[2] as usize;
                    if partner < m {
                        continue; // 相手側で 1 回だけ出す
                    }
                    let Some(other) = self.layout_of(partner) else {
                        continue;
                    };
                    let (a, b) = mt_rec(s);
                    if e[0] == kt::RS {
                        Some(Advanced::Rs {
                            other,
                            code_a: code_of(a),
                            code_b: code_of(b),
                            delay_ms: 0,
                        })
                    } else {
                        Some(Advanced::Socd {
                            other,
                            code_a: code_of(a),
                            code_b: code_of(b),
                            mode: match trig[m].priority {
                                1 => SocdMode::PriorityA,
                                2 => SocdMode::PriorityB,
                                3 => SocdMode::Neutral,
                                _ => SocdMode::Last,
                            },
                            delay_ms: 0,
                        })
                    }
                }
                _ => None,
            };
            if let Some(binding) = binding {
                out.push(AdvancedKey {
                    key: li,
                    layer,
                    binding,
                });
            }
        }
        out
    }

    /// 高度キーの設定/解除。
    fn set_advanced(&self, key: usize, layer: u8, binding: &Option<Advanced>) -> HubResult<()> {
        let p = self.profile()?;
        let l = (layer as usize).min(LAYERS - 1);
        let m = self.matrix(key)?;
        let old_map = self.keymap(p, l)?;
        let defaults = self.default_keymap(p, l)?;
        let mut map = old_map.clone();
        let old_trig = self.triggers(p)?;
        let mut trig = old_trig.clone();

        // 既存の割り当てを外す (ペアなら相手も既定キーへ)。
        let clear = |map: &mut Vec<[u8; 3]>, trig: &mut Vec<Trigger>, i: usize| {
            let e = map[i];
            if matches!(e[0], kt::RS | kt::SOCD | kt::LT) {
                let partner = e[2] as usize;
                if partner < KEYS && map[partner][0] == e[0] {
                    map[partner] = defaults[partner];
                    trig[partner].priority = 0;
                }
            }
            if (kt::DKS..=kt::LT).contains(&e[0]) {
                map[i] = defaults[i];
                trig[i].priority = 0;
            }
        };
        clear(&mut map, &mut trig, m);

        if let Some(b) = binding {
            let (used_dks, used_tgl, used_mt) = self.used_slots(p)?;
            let free = |used: &[bool; SLOTS], skip: Option<usize>| {
                (0..SLOTS).find(|&s| !used[s] && Some(s) != skip)
            };
            let none = || HubError::Invalid("高度キーの空きスロットがありません".into());
            match b {
                Advanced::Dks { rows, bottom } => {
                    let s = free(&used_dks, None).ok_or_else(none)?;
                    let old = self.read_range(cmd::DKS_GET, 768 * p as usize, 768)?;
                    let mut rec = DksRec::decode(&old[s * 24..s * 24 + 24]);
                    let b10 = ((bottom * 10.).round() as u8).clamp(1, 34);
                    rec.points = [10.min(b10), b10, b10, 10.min(b10)];
                    rec.actions = [[0; 3]; 4];
                    rec.masks = [[0; 4]; 4];
                    for (i, r) in rows.iter().take(4).enumerate() {
                        rec.actions[i] = key_of(r.code);
                        rec.masks[i] = [
                            dks_mask(r.phases[0], false),
                            dks_mask(r.phases[1], false),
                            dks_mask(r.phases[2], false),
                            dks_mask(r.phases[3], true),
                        ];
                    }
                    let mut new = old.clone();
                    new[s * 24..s * 24 + 24].copy_from_slice(&rec.encode());
                    self.write_range(cmd::DKS_SET, 768 * p as usize, &new, 24, Some(&old))?;
                    map[m] = [kt::DKS, s as u8, 0];
                }
                Advanced::Tgl { code, .. } => {
                    let s = free(&used_tgl, None).ok_or_else(none)?;
                    let old = self.read_range(cmd::TGL_GET, 128 * p as usize, 128)?;
                    let mut new = old.clone();
                    new[s * 3..s * 3 + 3].copy_from_slice(&key_of(*code));
                    self.write_range(cmd::TGL_SET, 128 * p as usize, &new, 3, Some(&old))?;
                    map[m] = [kt::TGL, s as u8, 0];
                }
                Advanced::Mt { tap, hold, hold_ms } => {
                    let s = free(&used_mt, None).ok_or_else(none)?;
                    let old = self.read_range(cmd::MT_GET, 256 * p as usize, 256)?;
                    let mut new = old.clone();
                    new[s * 6..s * 6 + 3].copy_from_slice(&key_of(*tap));
                    new[s * 6 + 3..s * 6 + 6].copy_from_slice(&key_of(*hold));
                    self.write_range(cmd::MT_SET, 256 * p as usize, &new, 6, Some(&old))?;
                    map[m] = [kt::MT, s as u8, (*hold_ms / 10).min(255) as u8];
                }
                Advanced::Rs { other, code_a, code_b, .. }
                | Advanced::Socd { other, code_a, code_b, .. } => {
                    let o = self.matrix(*other)?;
                    if o == m {
                        return Err(HubError::Invalid("同じキー同士は組めません".into()));
                    }
                    clear(&mut map, &mut trig, o);
                    let s1 = free(&used_mt, None).ok_or_else(none)?;
                    let s2 = free(&used_mt, Some(s1)).ok_or_else(none)?;
                    let old = self.read_range(cmd::MT_GET, 256 * p as usize, 256)?;
                    let mut new = old.clone();
                    let (ka, kb) = (key_of(*code_a), key_of(*code_b));
                    // 公式: 各キーの MT スロット = [自分が送るキー, 相手のキー]。
                    new[s1 * 6..s1 * 6 + 3].copy_from_slice(&ka);
                    new[s1 * 6 + 3..s1 * 6 + 6].copy_from_slice(&kb);
                    new[s2 * 6..s2 * 6 + 3].copy_from_slice(&kb);
                    new[s2 * 6 + 3..s2 * 6 + 6].copy_from_slice(&ka);
                    self.write_range(cmd::MT_SET, 256 * p as usize, &new, 6, Some(&old))?;
                    let ty = if matches!(b, Advanced::Rs { .. }) {
                        kt::RS
                    } else {
                        kt::SOCD
                    };
                    map[m] = [ty, s1 as u8, o as u8];
                    map[o] = [ty, s2 as u8, m as u8];
                    if let Advanced::Socd { mode, .. } = b {
                        let (pa, pb) = match mode {
                            SocdMode::PriorityA => (1, 2),
                            SocdMode::PriorityB => (2, 1),
                            SocdMode::Neutral => (3, 3),
                            _ => (0, 0),
                        };
                        trig[m].priority = pa;
                        trig[o].priority = pb;
                    }
                }
                Advanced::Mpt { .. } | Advanced::End { .. } => {
                    return Err(HubError::Unsupported("この高度キー"));
                }
            }
        }
        self.write_keymap(p, l, &old_map, &map)?;
        if trig != old_trig {
            self.write_triggers(p, &old_trig, &trig)?;
        }
        Ok(())
    }

    /// live_travel 用の debug テレメトリを ON/OFF (機能領域 byte7 bit3)。
    fn set_debug(dev: &Mutex<hidapi::HidDevice>, travel: &Mutex<Vec<f32>>, p: u8, on: bool) -> HubResult<()> {
        let d = dev.lock().unwrap_or_else(|p| p.into_inner());
        let mut f = vec![];
        for off in [0usize, 56] {
            let n = if off == 0 { 56 } else { 8 };
            f.extend(xfer(&d, travel, None, cmd::FUNC_GET, (64 * p as usize + off) as u16, n, &[])?);
        }
        if f.len() < 64 || ((f[7] >> 3) & 1 == 1) == on {
            return Ok(());
        }
        f[7] = (f[7] & !0x08) | ((on as u8) << 3);
        xfer(&d, travel, None, cmd::FUNC_SET, (64 * p as usize) as u16, 56, &f[..56])?;
        Ok(())
    }
}

fn opts(v: &[(i64, &str)]) -> Vec<ChoiceOpt> {
    v.iter().map(|(a, b)| ChoiceOpt::new(*a, *b)).collect()
}

pub fn build_caps(m: &GlwModel) -> Capabilities {
    use section::*;
    let settings = vec![
        SettingSpec::choice(
            "polling",
            PERFORMANCE,
            "Polling rate",
            opts(&[(1000, "1K"), (2000, "2K"), (4000, "4K"), (8000, "8K")]),
        )
        .help("How often the keyboard reports to the PC (Hz)"),
        SettingSpec::toggle("tachyon", PERFORMANCE, "Tachyon mode")
            .help("High-precision Rapid Trigger: unlocks 0.005 / 0.001 mm steps"),
        SettingSpec::slider("rt_smart", PERFORMANCE, "RT Smart", 0., 15., 1., "")
            .help("Adaptive Rapid Trigger sensitivity (0 = off)"),
        SettingSpec::toggle("debounce_opt", KEYBOARD, "Anti-chatter optimization")
            .help("Filters key chatter on the magnetic switches"),
        SettingSpec::toggle("bottom_rt", KEYBOARD, "Bottom RT stability")
            .help("Reduces false triggers from desk vibration or resting pressure near bottom-out"),
        SettingSpec::toggle("travel_check", KEYBOARD, "Adaptive travel")
            .help("Automatically compensates for switch travel drift"),
        SettingSpec::toggle("mac_mode", KEYBOARD, "Mac mode")
            .help("Uses the Mac layers instead of the Windows layers"),
        SettingSpec::toggle("lock_win", KEYBOARD, "Lock Win key"),
        SettingSpec::toggle("lock_alt_tab", KEYBOARD, "Lock Alt+Tab"),
        SettingSpec::toggle("lock_alt_f4", KEYBOARD, "Lock Alt+F4"),
        SettingSpec::toggle("light_rainbow", LIGHTING, "Key lighting: rainbow colors")
            .help("Use multicolor instead of the chosen color (effects that support it)"),
        SettingSpec::toggle("strip_rainbow", LIGHTING, "Light strip: rainbow colors"),
        SettingSpec::choice(
            "light_sleep",
            POWER,
            "Lighting sleep",
            opts(&[
                (0, "Never"),
                (1, "1 min"),
                (3, "3 min"),
                (5, "5 min"),
                (10, "10 min"),
                (15, "15 min"),
                (20, "20 min"),
                (30, "30 min"),
            ]),
        )
        .help("Turns the lighting off after this long without input"),
    ];
    let mut lighting = vec![LightArea {
        id: 0,
        name: "Keys".into(),
        effects: m
            .effects
            .iter()
            .map(|&id| {
                let (name, color, speed, dir) = effect_meta(id);
                let mut e = LightEffect::new(id, name);
                if id == 0 {
                    e = e.per_key();
                }
                if color {
                    e = e.color();
                }
                if !speed {
                    e = e.still();
                }
                if dir {
                    e = e.dir();
                }
                e
            })
            .collect(),
        brightness_max: 100,
        speed_max: 4,
        directions: vec!["Forward".into(), "Reverse".into()],
        switchable: false,
        zones: vec![],
        per_key: true,
        direct_drive: false,
        live_readback: false,
    }];
    if !m.strip.is_empty() {
        lighting.push(LightArea {
            id: 1,
            name: "Light strip".into(),
            effects: m
                .strip
                .iter()
                .map(|&(id, name, color, speed)| {
                    let mut e = LightEffect::new(id, name);
                    if color {
                        e = e.color();
                    }
                    if !speed {
                        e = e.still();
                    }
                    e
                })
                .collect(),
            brightness_max: 100,
            speed_max: 4,
            directions: vec![],
            switchable: false,
            zones: vec![],
            per_key: false,
            direct_drive: false,
            live_readback: false,
        });
    }
    Capabilities {
        settings,
        actions: vec![
            ActionSpec::new("calibrate", PERFORMANCE, "Start magnetic calibration")
                .help("Press every key slowly to the bottom once, then end calibration"),
            ActionSpec::new("calibrate_finish", PERFORMANCE, "End calibration")
                .help("Saves the measured travel to the keyboard"),
            ActionSpec::new("calibrate_restart", PERFORMANCE, "Restart calibration"),
            ActionSpec::new("factory_reset", SYSTEM, "Factory reset")
                .help("Restores every profile, key, trigger, lighting and macro setting")
                .danger("All settings on this keyboard will be erased"),
        ],
        profiles: Some(ProfileCaps {
            count: 4,
            renamable: false,
        }),
        mouse: None,
        keyboard: Some(KeyboardCaps {
            layout: m
                .layout
                .iter()
                .map(|&(id, x, y, w, h, label)| {
                    // 公式座標は 1 キー = 1.0952 (キー 1.0 + 隙間 0.0952)。アプリの 1u ピッチへ。
                    const PITCH: f32 = 1.0952;
                    const GAP: f32 = PITCH - 1.0;
                    LayoutKey {
                        id: id as u16,
                        x: x / PITCH,
                        y: y / PITCH,
                        w: (w + GAP) / PITCH,
                        h: (h + GAP) / PITCH,
                        label: label.into(),
                    }
                })
                .collect(),
            layers: vec!["Win".into(), "Win Fn".into(), "Mac".into(), "Mac Fn".into()],
            catalog: catalog(),
            combos: true,
            perf: Some(PerfCaps {
                travel: m.travel,
                min: 0.02,
                step: 0.01,
                rapid_trigger: true,
                separate_release: false,
                rt_first: false,
                deadzones: true,
                continuous_rt: true,
                switch_types: m.switches.iter().map(|s| s.to_string()).collect(),
                switch_ids: vec![],
                per_key: true,
                live_travel: true,
            }),
            advanced: vec![
                AdvancedKind::Dks,
                AdvancedKind::Mt,
                AdvancedKind::Tgl,
                AdvancedKind::Socd,
                AdvancedKind::Rs,
            ],
            macros: Some(MacroCaps {
                slots: MACROS as u8,
                max_events: 1500,
                max_delay_ms: 65535,
                modes: opts(&[
                    (1, "Run a set number of times"),
                    (0, "Repeat while held"),
                    (255, "Repeat until pressed again"),
                ])
                .into_iter()
                .collect(),
                repeat_modes: vec![1],
                names: false,
                mouse_events: true,
            }),
        }),
        lighting,
        battery: false,
        peq: None,
        firmware_feed: None,
    }
}

impl DeviceDriver for GlwDriver {
    fn info(&self) -> DeviceInfo {
        let mut i = self.info.lock().unwrap_or_else(|p| p.into_inner()).clone();
        if let Ok(r) = self.info_raw() {
            if r.len() >= 2 {
                i.firmware = Some(format!("{:x}", u16::from_le_bytes([r[0], r[1]])));
            }
        }
        i.supported = true;
        i
    }

    fn caps(&self) -> Capabilities {
        build_caps(self.model)
    }

    fn read(&self) -> HubResult<DeviceState> {
        self.ensure_defaults();
        let (p, count, _, _) = self.base()?;
        let f = self.func(p)?;
        let mut settings = BTreeMap::new();
        let hz = match f[4] & 15 {
            1 => 8000,
            2 => 4000,
            3 => 2000,
            _ => 1000,
        };
        settings.insert("polling".into(), SettingValue::Choice(hz));
        settings.insert("tachyon".into(), SettingValue::Bool(f[7] & 1 != 0));
        settings.insert("rt_smart".into(), SettingValue::Num(f[20] as f32));
        settings.insert("debounce_opt".into(), SettingValue::Bool(f[7] >> 5 != 0));
        settings.insert("bottom_rt".into(), SettingValue::Bool(f[7] & 2 != 0));
        settings.insert("travel_check".into(), SettingValue::Bool(f[7] & 4 != 0));
        settings.insert("mac_mode".into(), SettingValue::Bool((f[1] & 15) % 4 >= 2));
        settings.insert("lock_win".into(), SettingValue::Bool(f[6] & 1 != 0));
        settings.insert("lock_alt_tab".into(), SettingValue::Bool(f[6] & 2 != 0));
        settings.insert("lock_alt_f4".into(), SettingValue::Bool(f[6] & 4 != 0));
        settings.insert("light_rainbow".into(), SettingValue::Bool(f[12] != 0));
        settings.insert("strip_rainbow".into(), SettingValue::Bool(f[27] != 0));
        let sleep = if f[36] == 1 { 0 } else { (f[35] as i64 / 2).max(1) };
        settings.insert("light_sleep".into(), SettingValue::Choice(sleep));

        let maps: Vec<Vec<[u8; 3]>> = (0..LAYERS).map(|l| self.keymap(p, l)).collect::<HubResult<_>>()?;
        let trig = self.triggers(p)?;
        let dks = self.read_range(cmd::DKS_GET, 768 * p as usize, 768)?;
        let mt = self.read_range(cmd::MT_GET, 256 * p as usize, 256)?;
        let tgl = self.read_range(cmd::TGL_GET, 128 * p as usize, 128)?;
        let colors = self.colors(p)?;
        let (moff, msize) = self.macro_area(p)?;
        let bank = self.read_range(cmd::MACRO_GET, moff, msize)?;

        let keymap = maps
            .iter()
            .map(|map| {
                self.model
                    .layout
                    .iter()
                    .map(|k| {
                        let e = map[k.0 as usize];
                        if (kt::DKS..=kt::LT).contains(&e[0]) {
                            // 高度キーは advanced 側で出す。表示は割り当て無し扱い。
                            KeyAction::None
                        } else {
                            to_action(e)
                        }
                    })
                    .collect()
            })
            .collect();
        let perf = self
            .model
            .layout
            .iter()
            .map(|k| self.perf_of(&trig[k.0 as usize]))
            .collect();
        let advanced = (0..LAYERS)
            .flat_map(|l| self.advanced_of(l as u8, &maps[l], &dks, &mt, &tgl, &trig))
            .collect();
        let macros = (0..MACROS).map(|s| decode_macro(&bank, s)).collect();
        let mut lighting = BTreeMap::new();
        lighting.insert(0u8, self.lighting_state(&f, 0, &colors));
        if !self.model.strip.is_empty() {
            lighting.insert(1u8, self.lighting_state(&f, 1, &colors));
        }
        let _ = count;
        Ok(DeviceState {
            profile: ProfileState {
                active: p,
                names: vec![],
            },
            settings,
            mouse: None,
            keyboard: Some(KeyboardState {
                keymap,
                perf,
                advanced,
                macros,
            }),
            lighting,
            peq: None,
        })
    }

    fn apply(&self, change: &Change) -> HubResult<()> {
        if let Change::Profile(i) = change {
            let (_, count, order, raw) = self.base()?;
            let pos = order
                .iter()
                .position(|&o| o == *i)
                .filter(|&x| x < count as usize)
                .ok_or_else(|| HubError::Invalid("このプロファイルは無効です".into()))?;
            let mut new = raw.clone();
            new[0] = pos as u8;
            return self.write_range(cmd::BASE_SET, 0, &new, CHUNK, None);
        }
        let p = self.profile()?;
        match change {
            Change::Setting { id, value } => {
                let old = self.func(p)?;
                let mut f = old.clone();
                let b = value.as_bool() as u8;
                match id.as_str() {
                    "polling" => {
                        let code = match value.as_choice() {
                            8000 => 1,
                            4000 => 2,
                            2000 => 3,
                            1000 => 4,
                            _ => return Err(HubError::Invalid("polling rate".into())),
                        };
                        f[4] = (f[4] & 0xF0) | code;
                    }
                    "tachyon" => f[7] = (f[7] & !1) | b,
                    "rt_smart" => f[20] = value.as_num().clamp(0., 15.) as u8,
                    "debounce_opt" => f[7] = (f[7] & 0x1F) | if b == 1 { 7 << 5 } else { 0 },
                    "bottom_rt" => f[7] = (f[7] & !2) | (b << 1),
                    "travel_check" => f[7] = (f[7] & !4) | (b << 2),
                    "mac_mode" => {
                        // 公式 setMacMode: macMode = profile×4 + (Mac なら 2)、Mac では Win ロック解除。
                        let v = p * 4 + if b == 1 { 2 } else { 0 };
                        f[1] = (f[1] & 0xF0) | (v & 15);
                        if b == 1 {
                            f[6] &= !1;
                        }
                    }
                    "lock_win" => f[6] = (f[6] & !1) | b,
                    "lock_alt_tab" => f[6] = (f[6] & !2) | (b << 1),
                    "lock_alt_f4" => f[6] = (f[6] & !4) | (b << 2),
                    "light_rainbow" => f[12] = b,
                    "strip_rainbow" => f[27] = b,
                    "light_sleep" => {
                        let min = value.as_choice();
                        if min <= 0 {
                            f[36] = 1;
                        } else {
                            f[36] = 0;
                            f[35] = (min.clamp(1, 30) * 2) as u8;
                        }
                    }
                    _ => return Err(HubError::Unsupported("setting")),
                }
                self.write_func(p, &old, &f)
            }
            Change::Lighting { area, state } => {
                let old = self.func(p)?;
                let mut f = old.clone();
                let c = state.color;
                if *area == 0 {
                    f[8] = state.effect;
                    f[9] = state.brightness.min(100);
                    f[10] = 4u8.saturating_sub(state.speed.min(4));
                    f[11] = state.direction;
                    f[14..17].copy_from_slice(&[c.r, c.g, c.b]);
                    // キーごとの色 (layout 外のスロットは保つ)。
                    let oldc = self.colors(p)?;
                    let mut newc = oldc.clone();
                    for (li, k) in self.model.layout.iter().enumerate() {
                        let m = k.0 as usize * 3;
                        let v = state.per_key.get(&li).copied().unwrap_or_default();
                        newc[m..m + 3].copy_from_slice(&[v.r, v.g, v.b]);
                    }
                    if newc != oldc {
                        self.write_range(cmd::COLOR_SET, 512 * p as usize, &newc, 3, Some(&oldc))?;
                    }
                } else {
                    f[24] = state.effect;
                    f[25] = state.brightness.min(100);
                    f[26] = 4u8.saturating_sub(state.speed.min(4));
                    f[29..32].copy_from_slice(&[c.r, c.g, c.b]);
                }
                if f != old {
                    self.write_func(p, &old, &f)?;
                }
                Ok(())
            }
            Change::KeyMap { layer, key, action } => {
                let l = (*layer as usize).min(LAYERS - 1);
                let m = self.matrix(*key)?;
                let old = self.keymap(p, l)?;
                let mut map = old.clone();
                let entry = match action {
                    KeyAction::None => DISABLED,
                    KeyAction::Code(c) => key_of(*c),
                    KeyAction::Combo { mods, code } => {
                        let k = key_of(*code);
                        [kt::KEYBOARD, *mods, k[2]]
                    }
                    KeyAction::Macro(s) => {
                        let (moff, msize) = self.macro_area(p)?;
                        let bank = self.read_range(cmd::MACRO_GET, moff, 64)?;
                        let mode = if bank.len() >= 50 && bank[32..34] == [129, 126] {
                            bank[34 + (*s as usize).min(MACROS - 1)]
                        } else {
                            1
                        };
                        let _ = msize;
                        [kt::MACRO, (*s).min(MACROS as u8 - 1), mode]
                    }
                    KeyAction::Transparent => return Err(HubError::Unsupported("transparent key")),
                };
                // 高度キーを上書きするなら先に外す (ペアの相手を既定に戻す)。
                let old = if (kt::DKS..=kt::LT).contains(&map[m][0]) {
                    self.set_advanced(*key, l as u8, &None)?;
                    let fresh = self.keymap(p, l)?;
                    map = fresh.clone();
                    fresh
                } else {
                    old
                };
                map[m] = entry;
                self.write_keymap(p, l, &old, &map)
            }
            Change::KeyMapReset { layer } => {
                let l = (*layer as usize).min(LAYERS - 1);
                let old = self.keymap(p, l)?;
                let def = self.default_keymap(p, l)?;
                self.write_keymap(p, l, &old, &def)
            }
            Change::KeyPerf { keys, perf } => {
                let old = self.triggers(p)?;
                let mut new = old.clone();
                for &k in keys {
                    let m = self.matrix(k)?;
                    self.apply_perf(&mut new[m], perf);
                }
                self.write_triggers(p, &old, &new)
            }
            Change::Advanced { key, layer, binding } => self.set_advanced(*key, *layer, binding),
            Change::Macro { slot, def } => {
                if *slot >= MACROS {
                    return Err(HubError::Invalid("macro slot".into()));
                }
                let (moff, msize) = self.macro_area(p)?;
                let old = self.read_range(cmd::MACRO_GET, moff, msize)?;
                let mut defs: Vec<Option<MacroDef>> = (0..MACROS).map(|s| decode_macro(&old, s)).collect();
                defs[*slot] = def.clone();
                let new = encode_bank(&old, &defs)?;
                self.write_range(cmd::MACRO_SET, moff, &new, 1, Some(&old))?;
                // 割り当て済みキーの実行モード (code2) も合わせる。
                if let Some(d) = def {
                    let mode = wire_mode(d);
                    for l in 0..LAYERS {
                        let oldm = self.keymap(p, l)?;
                        let mut m = oldm.clone();
                        for e in m.iter_mut() {
                            if e[0] == kt::MACRO && e[1] as usize == *slot {
                                e[2] = mode;
                            }
                        }
                        if m != oldm {
                            self.write_keymap(p, l, &oldm, &m)?;
                        }
                    }
                }
                Ok(())
            }
            _ => Err(HubError::Unsupported("change")),
        }
    }

    fn run_action(&self, id: &str, _file: Option<&std::path::Path>) -> HubResult<ActionOutcome> {
        let simple = |c: u8, data: u8| {
            self.io(|d, defs| xfer(d, &self.travel, defs, c, 0, 1, &[data]))
                .map(|_| ())
        };
        match id {
            "calibrate" => {
                self.ensure_defaults();
                simple(cmd::CAL_START, 0)?;
                Ok(ActionOutcome {
                    message: Some("Calibration started — press every key to the bottom once".into()),
                    reload: false,
                })
            }
            "calibrate_finish" => {
                simple(cmd::CAL_END, 0)?;
                Ok(ActionOutcome {
                    message: Some("Calibration saved".into()),
                    reload: true,
                })
            }
            "calibrate_restart" => {
                self.ensure_defaults();
                simple(cmd::CAL_RESTART, 0)?;
                Ok(ActionOutcome {
                    message: Some("Calibration restarted".into()),
                    reload: false,
                })
            }
            "factory_reset" => {
                simple(cmd::FACTORY, 255)?;
                sleep(Duration::from_millis(1500));
                Ok(ActionOutcome {
                    message: Some("Factory settings restored".into()),
                    reload: true,
                })
            }
            _ => Err(HubError::Unsupported("action")),
        }
    }

    fn live_travel(&self) -> HubResult<Vec<f32>> {
        self.ensure_defaults();
        // debug テレメトリを有効化 (呼ばれなくなって 2 秒で自動で戻す)。
        let start_watchdog = {
            let mut l = self.live.lock().unwrap_or_else(|p| p.into_inner());
            l.last_call = Instant::now();
            if l.debug_on {
                false
            } else {
                l.profile = self.profile()?;
                Self::set_debug(&self.dev, &self.travel, l.profile, true)?;
                l.debug_on = true;
                true
            }
        };
        if start_watchdog {
            let (dev, travel, live) = (self.dev.clone(), self.travel.clone(), self.live.clone());
            std::thread::spawn(move || loop {
                sleep(Duration::from_millis(500));
                let mut l = live.lock().unwrap_or_else(|p| p.into_inner());
                if l.last_call.elapsed() > Duration::from_secs(2) {
                    let _ = Self::set_debug(&dev, &travel, l.profile, false);
                    l.debug_on = false;
                    return;
                }
            });
        }
        // 届いているテレメトリを取り込む。
        let defs = self.defaults.lock().unwrap_or_else(|p| p.into_inner()).clone();
        {
            let dev = self.dev.lock().unwrap_or_else(|p| p.into_inner());
            let mut buf = [0u8; PACKET + 1];
            for _ in 0..256 {
                match dev.read_timeout(&mut buf, 0) {
                    Ok(n) if n > 0 => absorb_telemetry(&buf[..n], &self.travel, defs.as_deref()),
                    _ => break,
                }
            }
        }
        let t = self.travel.lock().unwrap_or_else(|p| p.into_inner());
        Ok(self.model.layout.iter().map(|k| t[k.0 as usize]).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_matches_sdk_layout() {
        // getInfo: _simpleGetCommand(3, 0, 56) → [0x55, 3, 0, sum(56,0,0,0), 56, 0, 0, 0]。
        let p = request(3, 0, 56, &[]);
        assert_eq!(&p[..8], &[0x55, 3, 0, 56, 56, 0, 0, 0]);
        // 書き込み: sum は len, offLo, offHi, 0, data の和。
        let p = request(161, 0x0408, 2, &[0x10, 0x20]);
        assert_eq!(&p[..10], &[0x55, 161, 0, (2 + 8 + 4 + 0x10 + 0x20) as u8, 2, 8, 4, 0, 0x10, 0x20]);
    }

    #[test]
    fn response_is_checked() {
        let mut r = [0u8; 64];
        r[0] = 0xAA;
        r[1] = 8;
        r[4] = 3;
        r[5] = 0;
        r[6] = 2;
        r[8..11].copy_from_slice(&[16, 0, 4]);
        r[3] = (3 + 2 + 16 + 4) as u8;
        assert_eq!(parse_response(&r, 8, 512), Some(vec![16, 0, 4]));
        assert_eq!(parse_response(&r, 8, 0), None);
        r[3] ^= 1;
        assert_eq!(parse_response(&r, 8, 512), None);
    }

    #[test]
    fn trigger_round_trips_and_matches_preset() {
        // 公式プリセットの既定: actuation 150, rtPress/Release 30, deadzone 15。
        let t = Trigger {
            switch_type: 0,
            key_mode: 1,
            priority: 0,
            actuation: 150,
            press_precision: 0,
            release_precision: 0,
            rt_press: 30,
            rt_release: 30,
            press_deadzone: 15,
            release_deadzone: 15,
            head: 0,
        };
        let b = t.encode();
        assert_eq!(b[0], 0xA0);
        assert_eq!(b[2], 149);
        assert_eq!(b[5], 15 << 1);
        let back = Trigger::decode(&b);
        assert_eq!(Trigger { head: 0xA0, ..t }, back);
        // 9 ビット境界 (3.40mm = 340)。
        let t2 = Trigger { actuation: 340, ..t };
        assert_eq!(Trigger::decode(&t2.encode()).actuation, 340);
    }

    #[test]
    fn dks_round_trips() {
        let mut b = [0u8; 24];
        b[..4].copy_from_slice(&[10, 30, 30, 10]);
        b[4..7].copy_from_slice(&[16, 0, 4]);
        let w: u16 = 1 | (3 << 3) | (1 << 9);
        b[7..9].copy_from_slice(&w.to_le_bytes());
        let r = DksRec::decode(&b);
        assert_eq!(r.masks[0], [1, 3, 0, 1]);
        assert_eq!(r.encode(), b);
    }

    #[test]
    fn macro_bank_round_trips() {
        let def = MacroDef {
            name: String::new(),
            events: vec![
                MacroEvent { input: MacroInput::Key(0xE1), edge: MacroEdge::Down, delay_ms: 10 },
                MacroEvent { input: MacroInput::Key(4), edge: MacroEdge::Down, delay_ms: 0 },
                MacroEvent { input: MacroInput::Key(4), edge: MacroEdge::Up, delay_ms: 30 },
                MacroEvent { input: MacroInput::Mouse(MouseFn::Right), edge: MacroEdge::Up, delay_ms: 0 },
            ],
            mode: 1,
            repeat: 3,
        };
        let mut defs: Vec<Option<MacroDef>> = vec![None; MACROS];
        defs[2] = Some(def.clone());
        let bank = encode_bank(&vec![0u8; 8192], &defs).unwrap();
        assert_eq!(&bank[32..34], &[129, 126]);
        assert_eq!(bank[34 + 2], 3);
        assert_eq!(decode_macro(&bank, 2), Some(def));
        assert_eq!(decode_macro(&bank, 0), None);
        // 空のマクロは共通の終端ストリーム (offset 64) を指す。
        assert_eq!(u16::from_le_bytes([bank[0], bank[1]]), 64);
    }

    #[test]
    fn telemetry_is_big_endian_hundredths() {
        let mut r = [0u8; 11];
        r[0] = 0xA0;
        r[1..4].copy_from_slice(&[16, 0, 4]);
        r[6..8].copy_from_slice(&250u16.to_be_bytes());
        assert_eq!(parse_telemetry(&r), Some(([16, 0, 4], 2.5)));
    }

    #[test]
    fn layout_and_catalog_are_sane() {
        let c = build_caps(&ACE68_GT);
        let k = c.keyboard.unwrap();
        assert_eq!(k.layout.len(), 69);
        let mut ids: Vec<u16> = k.layout.iter().map(|l| l.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), 69, "matrix indices are unique");
        assert!(ids.iter().all(|&i| (i as usize) < KEYS));
        assert!(k.catalog.iter().map(|g| g.keys.len()).sum::<usize>() > 150);
        assert_eq!(to_action(DISABLED), KeyAction::None);
        assert_eq!(to_action([16, 0, 4]), KeyAction::Code(0x100004));
        // カタログに無い修飾+キーは Combo。
        assert_eq!(
            to_action([16, 0x22, 4]),
            KeyAction::Combo { mods: 0x22, code: 0x100004 }
        );
    }
}
