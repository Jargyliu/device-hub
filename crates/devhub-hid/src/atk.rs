//! CompX 系 16B report-8 + EEPROM プロトコル — ATK HUB / Scyrox / Pulsar (X2 CrazyLight)。
//!
//! 公式 ATK HUB (hub-v3) の解析 + F1 V3 ULTIMATE+ (373b:12a7) 実機で検証。
//! Scyrox V6 (3554:f5f6/f5f7) と Pulsar X2 CrazyLight (3710:3414/5406) は
//! 同じ CompX 系 ODM のファームで、ワイヤ形式も EEPROM マップも共通
//! (docs/PROTOCOL_NOTES.md §6)。機種差は [`CompxModel`] に集約する。
//! ワイヤ形式は mouse.fit 系と同じ系列:
//!
//! - 設定コレクション usage_page 0xFF02 / usage 2、output/input report id 8。
//! - 要求  `[cmd, 0, addrHi, addrLo, len, data…]` (16B、17 バイト目に cksum)
//! - 応答  `[rid=8, cmd echo, status, addrHi, addrLo, len, data…, cksum]` (17B)
//! - cksum = `0x55 - (全バイト和 & 0xff)` (17B 合計 ≡ 0x55)
//! - EEPROM: 読み cmd8、書き cmd7 (10B チャンク)。スカラは (v, 0x55-v) ペア。
//!
//! EEPROM マップ (実機 + 公式 JS `y.Address`):
//! ReportRate ブロック @0 (10B: rate,maxDpi,currentDpi,bhop,btnOpMode + 各crc)、
//! LOD @10、DPI値 @12 (4B×8、3950/旧形式)、DPI色 @44 (4B×8)、キー割当 @96 (4B×16)、
//! Debounce @169、MotionSync @171、CloseLedTime @173、直線補正 @175、リップル @177、
//! Sensor系 @181(競技/ターボ)/183(スリープ)/185、RfTxTime @187、WheelDebounce @227、
//! Shortcut @256 (32B×16)、Macro @768 (384B×16)、PAW3955 DPI @6912 (6B×8)、
//! DynamicSensitivity @6960。
//!
//! その他 cmd: 3=online, 4=battery, 5/6=pair, 9=factory, 14/15=configID,
//! 16=CID/MID (F1 V3 = "1,62"), 18=FW, 22/23=長距離, 29=dongle FW。
//! cmd16/29 と @179/@227/bhop は ATK HUB でしか確認していないので
//! 他メーカー機には送らない (未確認のコマンド番号を投げない — 9 は工場リセット)。

use devhub_core::backend::*;
use devhub_core::model::*;
use std::collections::BTreeMap;
use std::sync::Mutex;
use std::thread::sleep;
use std::time::{Duration, Instant};

const RID: u8 = 8;
const CHUNK: u16 = 10;

#[allow(dead_code)]
mod ee {
    pub const REPORT_RATE: u16 = 0;
    pub const MAX_DPI_STAGE: u16 = 2;
    pub const CURRENT_DPI: u16 = 4;
    pub const LOD: u16 = 10;
    /// 旧形式 DPI レコード (4B: xLo, yLo, flags, crc) — PAW3950 / Pulsar。
    pub const DPI_LEGACY: u16 = 12;
    pub const DPI_COLOR: u16 = 44;
    pub const KEY_FUNCTION: u16 = 96;
    pub const DEBOUNCE: u16 = 169;
    pub const MOTION_SYNC: u16 = 171;
    /// スリープ時間 — 公式 closeLedTime。wire 値は 10 秒単位 (6 = 60s = 1分)。
    pub const CLOSE_LED: u16 = 173;
    pub const ANGLE_SNAP: u16 = 175;
    pub const RIPPLE: u16 = 177;
    /// 移動中 LED 消灯 (moveCloseLights)。
    pub const MOVE_LED: u16 = 179;
    /// 競技モード (Scyrox "Highest performance") / ターボモード (Pulsar)。
    pub const PERF_MODE: u16 = 181;
    /// スリープ時間の実体 (Scyrox / Pulsar。@173 にも同値を書く)。10 秒単位。
    pub const SLEEP_TIME: u16 = 183;
    /// ボタン別ショートカット領域 — 32B × slot (slot index = 物理ボタン)。
    pub const SHORTCUT: u16 = 256;
    pub const WHEEL_DEBOUNCE: u16 = 227;
    pub const DPI_3955: u16 = 6912;
}

// ==================================================================== 機種表

/// DPI レコードの置き場所と形式。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DpiLayout {
    /// PAW3955: @6912 の 6B レコード、1 DPI 刻み (ATK F1 V3 ULTIMATE+)。
    Sensor3955,
    /// @12 の 4B レコード、50 DPI 刻み (PAW3950 の Scyrox V6 など)。
    Legacy50,
    /// @12 の 4B レコード、Pulsar 拡張 (10 刻み + 50/100 刻みの高 DPI 帯)。
    PulsarFine,
}

/// LOD バイトの意味。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LodKind {
    /// 1–11 → 0.7–1.7mm (PAW3955 "Ultimate")。
    Level3955,
    /// (表示 mm×10, wire コード)。
    Table(&'static [(i64, u8)]),
}

/// CompX 系 1 機種の差分。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CompxModel {
    pub model: &'static str,
    pub dpi: DpiLayout,
    pub dpi_min: u32,
    pub dpi_max: u32,
    pub dpi_step: u32,
    pub max_stages: usize,
    pub stage_colors: bool,
    pub xy_split: bool,
    pub rates_wired: &'static [i64],
    pub rates_wireless: &'static [i64],
    pub lod: LodKind,
    pub buttons: &'static [&'static str],
    pub debounce_max: f32,
    /// スリープを @183 から読み、@173 と @183 の両方に書く (Scyrox / Pulsar)。
    pub sleep_mirror: bool,
    /// スリープの選択肢 (分)。
    pub sleep_opts: &'static [i64],
    /// ATK HUB 固有 (cmd16 CID/MID, cmd29, @179, @227, bhop)。
    pub atk_hub: bool,
    /// @181 競技/ターボモードのラベル (None = 出さない)。
    pub perf_mode: Option<&'static str>,
    pub factory_reset: bool,
    /// オンボードプロファイル数 (0 = 宣言しない)。
    pub profiles: u8,
    /// FW バージョンが BCD (0x02 0x18 → "2.18")。
    pub fw_bcd: bool,
    /// ドングル初回接続時に Pulsar 公式ドライバの初期化列が要る。
    pub pulsar_init: bool,
}

const RATES_ALL: &[i64] = &[125, 250, 500, 1000, 2000, 4000, 8000];
const RATES_1K: &[i64] = &[125, 250, 500, 1000];
/// PAW3950 の 3 段 LOD (公式: 0.7→3, 1→1, 2→2。mousefit8 HTX Ultra 3950 と同じ)。
const LOD_3950: &[(i64, u8)] = &[(7, 3), (10, 1), (20, 2)];
const BUTTONS_5: &[&str] = &[
    "Left",
    "Right",
    "ホイールクリック",
    "サイド (戻る)",
    "サイド (進む)",
];

pub const ATK_F1V3U: CompxModel = CompxModel {
    model: "atk-f1v3u",
    dpi: DpiLayout::Sensor3955,
    dpi_min: 50,
    dpi_max: 42000,
    dpi_step: 1,
    max_stages: 8,
    stage_colors: true,
    xy_split: true,
    rates_wired: RATES_ALL,
    rates_wireless: RATES_ALL,
    lod: LodKind::Level3955,
    buttons: BUTTONS_5,
    debounce_max: 32.,
    sleep_mirror: false,
    sleep_opts: &[1, 2, 3, 5, 10, 30, 60],
    atk_hub: true,
    perf_mode: None,
    factory_reset: true,
    profiles: 4,
    fw_bcd: false,
    pulsar_init: false,
};

/// Scyrox V6 (PAW3950)。有線 3554:f5f6 / 8K ドングル 3554:f5f7
/// (f5f7 は V8 と共用 — V8 も 3950・@12 なので同じ表で扱える)。
pub const SCYROX_V6: CompxModel = CompxModel {
    model: "scyrox-v6",
    dpi: DpiLayout::Legacy50,
    dpi_min: 50,
    dpi_max: 30000,
    dpi_step: 50,
    max_stages: 5,
    stage_colors: true,
    xy_split: false,
    rates_wired: RATES_ALL,
    rates_wireless: RATES_ALL,
    lod: LodKind::Table(LOD_3950),
    buttons: BUTTONS_5,
    debounce_max: 20.,
    sleep_mirror: true,
    sleep_opts: &[1, 2, 3, 5, 10, 15, 20, 30],
    atk_hub: false,
    perf_mode: Some("競技モード"),
    factory_reset: true,
    profiles: 0,
    fw_bcd: true,
    pulsar_init: false,
};

/// Pulsar X2 CrazyLight。有線 3710:3414 (最大 1K) / 8K ドングル 3710:5406。
pub const PULSAR_X2CL: CompxModel = CompxModel {
    model: "x2-crazylight",
    dpi: DpiLayout::PulsarFine,
    dpi_min: 10,
    dpi_max: 32000,
    dpi_step: 10,
    max_stages: 6,
    stage_colors: true,
    xy_split: false,
    rates_wired: RATES_1K,
    rates_wireless: RATES_ALL,
    lod: LodKind::Table(LOD_3950),
    buttons: &[
        "Left",
        "Right",
        "ホイールクリック",
        "サイド (戻る)",
        "サイド (進む)",
        "DPI ボタン",
    ],
    debounce_max: 20.,
    sleep_mirror: true,
    sleep_opts: &[1, 2, 3, 5, 10, 15, 20, 30],
    atk_hub: false,
    perf_mode: Some("ターボモード"),
    // 公式 (Fusion / bbb.pulsar.gg) が cmd9 を送るのを確認できていない。
    factory_reset: false,
    profiles: 4,
    fw_bcd: true,
    pulsar_init: true,
};

/// registry の model id → 機種表。
pub fn model_for(id: &str) -> &'static CompxModel {
    match id {
        "scyrox-v6" => &SCYROX_V6,
        "x2-crazylight" => &PULSAR_X2CL,
        _ => &ATK_F1V3U,
    }
}

/// cmd16 応答 (data[0]=CID, data[1]=MID) → (表示名, model id)。
/// ドングル経由でも中継されるため、受信機名のままにしない。
fn mid_name(cid: u8, mid: u8) -> Option<(&'static str, &'static str)> {
    match (cid, mid) {
        (1, 62) => Some(("ATK F1 V3 ULTIMATE+", "atk-f1v3u")),
        _ => None,
    }
}

/// Pulsar 公式ドライバの初期化列 (litecrazy が実機の USB キャプチャから再生しているもの)。
/// 挿したばかりのドングルはこれを受けるまで電池などに応答しないことがある。
/// cmd1 (EncryptionData) ×2 種、cmd3 (online)、cmd2 (PCDriverStatus)、cmd4 (battery)。
const PULSAR_INIT: [[u8; 17]; 6] = [
    [8, 1, 0, 0, 8, 0x8e, 0x0c, 0x4d, 0x4c, 0, 0, 0, 0, 0, 0, 0, 0x11],
    [8, 3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x4a],
    [8, 1, 0, 0, 8, 0x95, 0x05, 0xdd, 0x4b, 0, 0, 0, 0, 0, 0, 0, 0x82],
    [8, 3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x4a],
    [8, 2, 0, 0, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x49],
    [8, 4, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x49],
];

// ==================================================================== DPI 形式

fn crc(b: &[u8]) -> u8 {
    85u8.wrapping_sub(b[..b.len() - 1].iter().map(|&x| x as u32).sum::<u32>() as u8)
}

/// 旧形式 4B レコード `[xLo, yLo, flags, crc]` の 1 軸。
/// `nib` = flags の下位 (x) / 上位 (y) ニブル: bit2-3 = 値の bit8-9。
/// Pulsar 拡張では bit1 = 50 刻み帯 (+10000)、bit0 = 2 倍帯。
fn legacy_axis(low: u8, nib: u8, layout: DpiLayout) -> u32 {
    let raw = low as u32 | (((nib as u32 >> 2) & 3) << 8);
    match layout {
        DpiLayout::PulsarFine => {
            let mut dpi = (raw + 1) * 10;
            if nib & 2 != 0 {
                dpi = dpi * 5 + 10000;
            }
            if nib & 1 != 0 {
                dpi *= 2;
            }
            dpi
        }
        _ => (raw + 1) * 50,
    }
}

/// 1 軸 → (low, nibble)。刻みに合わない値は丸める。
fn legacy_axis_enc(dpi: u32, layout: DpiLayout) -> (u8, u8) {
    let (raw, band) = match layout {
        DpiLayout::PulsarFine => {
            // bbb.pulsar.gg の encoder と同じ帯の切り方。
            if dpi >= 30100 {
                let d = (dpi / 100) * 100;
                ((d / 2).saturating_sub(10050) / 50, 3)
            } else if dpi >= 10050 {
                let d = ((dpi + 25) / 50) * 50;
                ((d - 10050) / 50, 2)
            } else {
                let d = ((dpi + 5) / 10).max(1) * 10;
                (d / 10 - 1, 0)
            }
        }
        _ => {
            let d = ((dpi + 25) / 50).max(1) * 50;
            (d / 50 - 1, 0)
        }
    };
    let raw = raw.min(0x3ff);
    ((raw & 0xff) as u8, (((raw >> 8) as u8) << 2) | band)
}

pub fn parse_legacy(b: &[u8], layout: DpiLayout) -> (u32, u32) {
    (
        legacy_axis(b[0], b[2] & 0x0f, layout),
        legacy_axis(b[1], b[2] >> 4, layout),
    )
}

pub fn encode_legacy(x: u32, y: u32, layout: DpiLayout) -> Vec<u8> {
    let (xl, xn) = legacy_axis_enc(x, layout);
    let (yl, yn) = legacy_axis_enc(y, layout);
    let mut b = vec![xl, yl, (yn << 4) | xn, 0];
    b[3] = crc(&b);
    b
}

/// 3955 DPI レコード (6B): [xLo,xHi,yLo,yHi,flags,crc]、raw+1 = dpi。
pub fn parse_3955(b: &[u8]) -> (u32, u32) {
    let f = b[4] as u32;
    let mut x = b[0] as u32 | ((b[1] as u32) << 8) | (((f >> 2) & 3) << 16);
    let mut y = b[2] as u32 | ((b[3] as u32) << 8) | ((f >> 6) << 16);
    x += 1;
    y += 1;
    if f & 1 == 1 {
        x *= 2;
    }
    if f & 2 == 2 {
        y *= 2;
    }
    (x, y)
}

pub fn encode_3955(x: u32, y: u32) -> Vec<u8> {
    let l = x.saturating_sub(1);
    let s = y.saturating_sub(1);
    let mut b = vec![
        (l & 0xff) as u8,
        ((l >> 8) & 0xff) as u8,
        (s & 0xff) as u8,
        ((s >> 8) & 0xff) as u8,
        ((((l >> 16) & 3) << 2) | (((s >> 16) & 3) << 6)) as u8,
        0,
    ];
    b[5] = crc(&b);
    b
}

// ==================================================================== 構造

pub struct AtkDriver {
    dev: Mutex<hidapi::HidDevice>,
    info: Mutex<DeviceInfo>,
    caps: Capabilities,
    model: &'static CompxModel,
    events: Option<std::sync::Arc<dyn Fn(HubEvent) + Send + Sync>>,
    /// CID/MID で機種確定済みか (ドングル経由で休眠中は未確定のまま)。
    detected: Mutex<bool>,
}

pub fn open(
    dev: hidapi::HidDevice,
    info: DeviceInfo,
    events: Option<std::sync::Arc<dyn Fn(HubEvent) + Send + Sync>>,
) -> HubResult<AtkDriver> {
    dev.set_blocking_mode(false).ok();
    let model = model_for(&info.model);
    let d = AtkDriver {
        dev: Mutex::new(dev),
        caps: build_caps(model, info.transport),
        info: Mutex::new(info),
        model,
        events,
        detected: Mutex::new(!model.atk_hub),
    };
    if model.pulsar_init && d.is_wireless() && d.cmd_raw(4, 0, &[]).is_err() {
        d.pulsar_handshake();
    }
    // CID/MID で機種確定 (応答があれば名を差し替える)。
    d.maybe_detect();
    Ok(d)
}

impl AtkDriver {
    fn is_wireless(&self) -> bool {
        self.info.lock().unwrap_or_else(|p| p.into_inner()).transport == Transport::Wireless24G
    }

    /// この接続で選べるポーリングレート。
    fn caps_rates(&self) -> &'static [i64] {
        if self.is_wireless() {
            self.model.rates_wireless
        } else {
            self.model.rates_wired
        }
    }

    /// Pulsar ドングルの初期化列を送る (応答は読み捨て)。
    fn pulsar_handshake(&self) {
        let dev = self.dev.lock().unwrap_or_else(|p| p.into_inner());
        for f in &PULSAR_INIT {
            if dev.write(f).is_err() {
                return;
            }
            sleep(Duration::from_millis(10));
            let mut buf = [0u8; 64];
            while matches!(dev.read_timeout(&mut buf, 5), Ok(n) if n > 0) {}
        }
    }

    /// `info` の変更を UI へ通知。
    fn emit_info(&self, i: DeviceInfo) {
        if let Some(ev) = &self.events {
            ev(HubEvent::Info(i.id.clone(), Box::new(i)));
        }
    }

    /// CID/MID 未確定なら cmd16 を試し、取れれば機種名を差し替える。
    /// ドングル経由でマウス休眠中は失敗するので、覚醒後の呼び出しで確定する。
    fn maybe_detect(&self) {
        if *self.detected.lock().unwrap_or_else(|p| p.into_inner()) {
            return;
        }
        let Ok(r) = self.cmd_raw(16, 0, &[]) else {
            return;
        };
        let (cid, mid) = (
            r.get(6).copied().unwrap_or(0),
            r.get(7).copied().unwrap_or(0),
        );
        if cid == 0 && mid == 0 {
            return;
        }
        *self.detected.lock().unwrap_or_else(|p| p.into_inner()) = true;
        let mut i = self.info.lock().unwrap_or_else(|p| p.into_inner());
        let mut dirty = false;
        if let Some((name, model)) = mid_name(cid, mid) {
            if i.name != name {
                i.name = name.into();
                i.model = model.into();
                dirty = true;
            }
        }
        if dirty {
            let info = i.clone();
            drop(i);
            self.emit_info(info);
        }
    }

    /// 休眠フラグの更新 (ドングル経由のみ意味を持つ)。
    fn set_asleep(&self, asleep: bool) {
        let mut i = self.info.lock().unwrap_or_else(|p| p.into_inner());
        if i.transport != Transport::Wireless24G || i.asleep == asleep {
            return;
        }
        i.asleep = asleep;
        let info = i.clone();
        drop(i);
        self.emit_info(info);
    }

    /// 16B コマンド → 17B 応答 (cmd echo を拾う。最大 3 回)。
    fn cmd_raw(&self, cmd: u8, addr: u16, data: &[u8]) -> HubResult<Vec<u8>> {
        let mut f = [0u8; 17];
        f[0] = RID;
        f[1] = cmd;
        f[3] = (addr >> 8) as u8;
        f[4] = addr as u8;
        f[5] = data.len().min(10) as u8;
        f[6..6 + data.len().min(10)].copy_from_slice(&data[..data.len().min(10)]);
        let s: u32 = f[..16].iter().map(|&b| b as u32).sum();
        f[16] = 85u32.wrapping_sub(s) as u8;
        let dev = self.dev.lock().unwrap_or_else(|p| p.into_inner());
        for _ in 0..3 {
            dev.write(&f)
                .map_err(|e| HubError::Io(format!("atk write: {e}")))?;
            let deadline = Instant::now() + Duration::from_millis(400);
            loop {
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    break;
                }
                let mut buf = [0u8; 64];
                match dev.read_timeout(&mut buf, left.as_millis().min(120) as i32) {
                    Ok(n) if n >= 17 && buf[1] == cmd => {
                        self.set_asleep(false);
                        return Ok(buf[..17].to_vec());
                    }
                    _ => continue,
                }
            }
            sleep(Duration::from_millis(20));
        }
        // ドングル経由で応答が無い = マウス休眠中とみなす。
        self.set_asleep(true);
        Err(HubError::Protocol("atk: response timeout".into()))
    }

    /// EEPROM 読み出し (10B チャンク、内部結合)。データは resp[6..]。
    fn ee_read(&self, addr: u16, len: u16) -> HubResult<Vec<u8>> {
        let mut out = Vec::with_capacity(len as usize);
        let mut a = addr;
        while (out.len() as u16) < len {
            let want = (len - out.len() as u16).min(CHUNK) as u8;
            // GetEEPROM: len は要求の [4] (=data.len()=0 ではなく len フィールド)。
            let mut f = [0u8; 17];
            f[0] = RID;
            f[1] = 8;
            f[3] = (a >> 8) as u8;
            f[4] = a as u8;
            f[5] = want;
            let s: u32 = f[..16].iter().map(|&b| b as u32).sum();
            f[16] = 85u32.wrapping_sub(s) as u8;
            let dev = self.dev.lock().unwrap_or_else(|p| p.into_inner());
            let mut got = None;
            for _ in 0..3 {
                dev.write(&f)
                    .map_err(|e| HubError::Io(format!("atk write: {e}")))?;
                let deadline = Instant::now() + Duration::from_millis(400);
                loop {
                    let left = deadline.saturating_duration_since(Instant::now());
                    if left.is_zero() {
                        break;
                    }
                    let mut buf = [0u8; 64];
                    match dev.read_timeout(&mut buf, left.as_millis().min(120) as i32) {
                        Ok(n) if n >= 17 && buf[1] == 8 => {
                            got = Some(buf);
                            break;
                        }
                        _ => continue,
                    }
                }
                if got.is_some() {
                    break;
                }
                sleep(Duration::from_millis(20));
            }
            drop(dev);
            let buf = got.ok_or_else(|| HubError::Protocol("atk: ee timeout".into()))?;
            let n = (buf[5] as usize).min(10).min(want as usize);
            out.extend_from_slice(&buf[6..6 + n]);
            if n < want as usize {
                break;
            }
            a += want as u16;
        }
        Ok(out)
    }

    /// EEPROM 書き込み (SetEEPROM cmd7、10B チャンク)。
    fn ee_write(&self, addr: u16, data: &[u8]) -> HubResult<()> {
        for (i, chunk) in data.chunks(CHUNK as usize).enumerate() {
            self.cmd_raw(7, addr + (i * CHUNK as usize) as u16, chunk)?;
            sleep(Duration::from_millis(10));
        }
        Ok(())
    }

    /// スカラ (v, 0x55-v) ペア書き込み。
    fn ee_write_val(&self, addr: u16, val: u8) -> HubResult<()> {
        self.ee_write(addr, &[val, 85u8.wrapping_sub(val)])
    }

    /// スカラ読み出し。ペア検証付き。
    fn ee_u8(&self, addr: u16) -> HubResult<u8> {
        let b = self.ee_read(addr, 2)?;
        if (b[0] as u16 + b[1] as u16) & 0xff == 85 {
            Ok(b[0])
        } else {
            Err(HubError::Protocol(format!("eeprom @{addr}: bad pair")))
        }
    }

    /// @0 の 10B ブロックを読み、byte idx (=偶数 offset) を差し替えて書き戻す。
    fn cfg_byte(&self, idx: usize, val: u8) -> HubResult<()> {
        let mut b = self.ee_read(0, 10)?;
        b[idx] = val;
        b[idx + 1] = 85u8.wrapping_sub(val);
        self.ee_write(0, &b)
    }

    /// KeyFunction @96 4B レコード → ButtonAction。type5=ShortcutKey は
    /// ボタンごとの領域 (@256+slot*32) を別途読む。
    fn decode_keyfn(&self, slot: usize, b: &[u8]) -> ButtonAction {
        match b[0] {
            1 => ButtonAction::Mouse(match b[1] {
                2 => MouseFn::Right,
                4 => MouseFn::Middle,
                8 => MouseFn::Back,
                16 => MouseFn::Forward,
                _ => MouseFn::Left,
            }),
            2 => ButtonAction::Dpi(match b[1] {
                2 => DpiFn::Up,
                3 => DpiFn::Down,
                _ => DpiFn::Cycle,
            }),
            4 if b[1] == 100 => ButtonAction::Mouse(MouseFn::DoubleClick),
            4 => ButtonAction::Fire {
                button: MouseFn::Left,
                count: b[2],
                interval_ms: b[1] as u16,
            },
            6 => ButtonAction::Macro {
                slot: b[1].saturating_sub(1),
                run: match b[2] {
                    253 => MacroRun::Toggle,
                    254 => MacroRun::WhileHeld,
                    n => MacroRun::Times(n as u16),
                },
            },
            5 => self.read_shortcut(slot).unwrap_or(ButtonAction::Disabled),
            7 => ButtonAction::PollingCycle,
            10 => ButtonAction::Dpi(DpiFn::Shift(((b[1] as u32) + 1) * 50)),
            11 => ButtonAction::Mouse(if b[1] == 2 {
                MouseFn::ScrollDown
            } else {
                MouseFn::ScrollUp
            }),
            _ => ButtonAction::Disabled,
        }
    }

    /// ShortcutKey 領域 (@256 + slot*32) を読む。
    /// 形式: [count, entry…, sum] — entry = {keyState, v1, v2}、
    /// keyState = 0x80(押下)|0x40(解放) + keyType (1=Keyboard 2=Fire 3=Combination 4=Macro)。
    /// sum = 0x55 - (count + 全 entry bytes)。
    fn read_shortcut(&self, slot: usize) -> Option<ButtonAction> {
        let b = self.ee_read(ee::SHORTCUT + (slot * 32) as u16, 32).ok()?;
        let n = *b.first()? as usize;
        if n == 0 || n > 10 {
            return None;
        }
        // 最初の「押下」エントリのキーを代表表示に使う。
        for i in 0..n {
            let e = &b[1 + i * 3..1 + i * 3 + 3];
            let (state, ty) = (e[0], e[0] & 0x3f);
            if state & 0x80 == 0 || ty != 1 {
                continue;
            }
            // keyType=Keyboard: v1 = 使用コード (公式は独自キー表)。修飾は v2 側。
            return Some(ButtonAction::Key(KeyStroke {
                mods: e[2],
                usage: e[1] as u16,
            }));
        }
        None
    }

    /// ButtonAction::Key → ShortcutKey 領域 (押下+解放の 2 エントリ)。
    fn encode_shortcut(&self, k: &KeyStroke) -> Vec<u8> {
        let mut b = vec![
            2,
            0x80 | 1,
            (k.usage & 0xff) as u8,
            k.mods,
            0x40 | 1,
            (k.usage & 0xff) as u8,
            k.mods,
        ];
        let s: u8 = b.iter().fold(0u8, |a, &x| a.wrapping_add(x));
        b.push(85u8.wrapping_sub(s));
        b.resize(32, 0);
        b
    }

    /// ButtonAction → KeyFunction 4B レコード。
    fn encode_keyfn(&self, a: &ButtonAction) -> HubResult<[u8; 4]> {
        use ButtonAction::*;
        let rec = |ty: u8, ph: u8, pl: u8| {
            let mut r = [ty, ph, pl, 0];
            r[3] = crc(&r);
            r
        };
        Ok(match a {
            Disabled => rec(0, 0, 0),
            Mouse(MouseFn::DoubleClick) => rec(4, 100, 2),
            Mouse(MouseFn::ScrollUp) => rec(11, 1, 0),
            Mouse(MouseFn::ScrollDown) => rec(11, 2, 0),
            Mouse(f) => {
                let mask = match f {
                    MouseFn::Left => 1,
                    MouseFn::Right => 2,
                    MouseFn::Middle => 4,
                    MouseFn::Back => 8,
                    MouseFn::Forward => 16,
                    _ => 1,
                };
                rec(1, mask, 0)
            }
            Dpi(DpiFn::Cycle) => rec(2, 1, 0),
            Dpi(DpiFn::Up) => rec(2, 2, 0),
            Dpi(DpiFn::Down) => rec(2, 3, 0),
            Dpi(DpiFn::Shift(d)) => rec(10, 0, ((*d / 50).max(1) - 1) as u8),
            Fire {
                count, interval_ms, ..
            } => rec(4, (*interval_ms).min(255) as u8, *count),
            Macro { slot: s, run } => {
                let t = match run {
                    MacroRun::Times(n) => (*n).min(252) as u8,
                    MacroRun::WhileHeld => 254,
                    MacroRun::Toggle => 253,
                };
                rec(6, *s + 1, t)
            }
            PollingCycle => rec(7, 0, 0),
            // ShortcutKey — 本体の割当は別領域 (@256+slot*32) に書く。
            Key(_) => rec(5, 0, 0),
            Media(_) => return Err(HubError::Unsupported("media button")),
            ProfileCycle => return Err(HubError::Unsupported("profile cycle")),
            ScrollModeToggle => return Err(HubError::Unsupported("scroll-mode toggle")),
            Unsupported { .. } => return Err(HubError::Unsupported("raw action")),
        })
    }
}

// ==================================================================== caps

fn lod_opts(m: &CompxModel) -> Vec<ChoiceOpt> {
    match m.lod {
        LodKind::Level3955 => (7..=17)
            .map(|v| ChoiceOpt::new(v, format!("{:.1}mm", v as f32 / 10.)))
            .collect(),
        LodKind::Table(t) => t
            .iter()
            .map(|&(v, _)| ChoiceOpt::new(v, format!("{:.1}mm", v as f32 / 10.)))
            .collect(),
    }
}

/// LOD wire コード → 表示 (mm×10)。
fn lod_decode(m: &CompxModel, code: u8) -> Option<i64> {
    match m.lod {
        LodKind::Level3955 => Some((code.clamp(1, 11) + 6) as i64),
        LodKind::Table(t) => t.iter().find(|e| e.1 == code).map(|e| e.0),
    }
}

fn lod_encode(m: &CompxModel, mm10: i64) -> Option<u8> {
    match m.lod {
        LodKind::Level3955 => Some((mm10.clamp(7, 17) - 6) as u8),
        LodKind::Table(t) => t.iter().find(|e| e.0 == mm10).map(|e| e.1),
    }
}

fn rate_code(hz: i64) -> Option<u8> {
    Some(match hz {
        125 => 8,
        250 => 4,
        500 => 2,
        1000 => 1,
        2000 => 16,
        4000 => 32,
        8000 => 64,
        _ => return None,
    })
}

fn rate_hz(code: u8) -> i64 {
    match code {
        8 => 125,
        4 => 250,
        2 => 500,
        16 => 2000,
        32 => 4000,
        64 => 8000,
        _ => 1000,
    }
}

pub fn build_caps(m: &CompxModel, transport: Transport) -> Capabilities {
    use section::*;
    let rates = if transport == Transport::Wireless24G {
        m.rates_wireless
    } else {
        m.rates_wired
    };
    let rate_opts: Vec<ChoiceOpt> = rates
        .iter()
        .map(|&hz| {
            let label = if hz >= 1000 {
                format!("{}K", hz / 1000)
            } else {
                hz.to_string()
            };
            ChoiceOpt::new(hz, &label)
        })
        .collect();
    let mut settings = vec![SettingSpec::choice(
        "polling_wireless",
        PERFORMANCE,
        "ポーリングレート",
        rate_opts,
    )
    .help("マウスの報告頻度 (Hz)")];
    if let Some(label) = m.perf_mode {
        settings.push(
            SettingSpec::toggle("hyper_mode", PERFORMANCE, label)
                .help("センサーを最高性能で動かします (電池の持ちは短くなります、@181)"),
        );
    }
    settings.extend([
        SettingSpec::choice("lod", SENSOR, "リフトオフ距離", lod_opts(m))
            .help("持ち上げて反応しなくなる高さ"),
        SettingSpec::toggle("motion_sync", SENSOR, "モーション同期")
            .help("ポインタとマウスの動きを同期させ、軌跡を滑らかにします"),
        SettingSpec::toggle("angle_snap", SENSOR, "直線補正")
            .help("移動のブレを直線に補正します"),
        SettingSpec::toggle("ripple", SENSOR, "リップル制御")
            .help("高DPI時の細かい揺れを抑えます"),
    ]);
    if m.atk_hub {
        settings.push(
            SettingSpec::slider("wheel_debounce", CLICK, "ホイール遅延", 0., 60., 1., "ms")
                .help("ホイールの入力遅延 (実機初期値 30)"),
        );
    }
    settings.push(
        SettingSpec::slider("debounce", CLICK, "クリックデバウンス", 0., m.debounce_max, 1., "ms")
            .help("ボタンのチャタリング防止 (@169 stabilizationTime)"),
    );
    settings.push(
        SettingSpec::choice(
            "sleep_min",
            POWER,
            "スリープまでの時間",
            m.sleep_opts
                .iter()
                .map(|&v| ChoiceOpt::new(v, format!("{v} 分")))
                .collect(),
        )
        .help("無操作でスリープするまでの時間 (10秒単位で保持)"),
    );
    if m.atk_hub {
        settings.extend([
            SettingSpec::toggle("move_close_led", LIGHTING, "移動中 LED 消灯")
                .help("操作している間は LED を消します (@179)"),
            SettingSpec::slider("bhop", CLICK, "ホイール誤操作防止", 0., 25., 1., "")
                .help("スクロール誤検知の抑止時間 (0=無効、公式は time/10 で保持)"),
        ]);
    }
    let actions = if m.factory_reset {
        vec![ActionSpec::new("factory_reset", SYSTEM, "工場出荷状態に戻す")
            .help("全設定を初期値に戻します (cmd9)")
            .danger("このデバイスの全設定が消去されます")]
    } else {
        vec![]
    };
    Capabilities {
        settings,
        actions,
        profiles: (m.profiles > 0).then_some(ProfileCaps {
            count: m.profiles,
            renamable: false,
        }),
        mouse: Some(MouseCaps {
            dpi_min: m.dpi_min,
            dpi_max: m.dpi_max,
            dpi_step: m.dpi_step,
            min_stages: 1,
            max_stages: m.max_stages,
            xy_split: m.xy_split,
            stage_colors: m.stage_colors,
            stage_toggle: false,
            buttons: m
                .buttons
                .iter()
                .map(|n| ButtonSlot { name: (*n).into() })
                .collect(),
            button_kinds: vec![
                ButtonActionKind::Mouse,
                ButtonActionKind::Key,
                ButtonActionKind::Dpi,
                ButtonActionKind::Fire,
                ButtonActionKind::Macro,
                ButtonActionKind::PollingCycle,
                ButtonActionKind::Disabled,
            ],
            require_left_click: true,
            macros: None,
            live_buttons: vec![],
            adaptive_dpi: None,
        }),
        keyboard: None,
        lighting: vec![],
        battery: true,
        peq: None,
        firmware_feed: None,
    }
}

// ==================================================================== DeviceDriver

impl DeviceDriver for AtkDriver {
    fn info(&self) -> DeviceInfo {
        self.maybe_detect();
        let mut i = self.info.lock().unwrap_or_else(|p| p.into_inner()).clone();
        // ドングル経由: cmd29 はドングル自身が応答 (マウス休眠中も返る)。
        let m = self.model;
        if i.transport == Transport::Wireless24G && m.atk_hub {
            if let Ok(r) = self.cmd_raw(29, 0, &[]) {
                if r[2] != 1 && (r[6] != 0 || r[7] != 0) {
                    i.dongle_firmware = Some(format!("{}.{}", r[6], r[7]));
                }
            }
        }
        // cmd18 が通らなければマウスは休眠中。
        match self.cmd_raw(18, 0, &[]) {
            Ok(r) => {
                i.asleep = false;
                if r[6] != 0 || r[7] != 0 {
                    i.firmware = Some(if m.fw_bcd {
                        format!("{:x}.{:02x}", r[6], r[7])
                    } else {
                        format!("{}.{}", r[6], r[7])
                    });
                }
            }
            Err(_) => {
                i.asleep = i.transport == Transport::Wireless24G;
            }
        }
        if let Ok(b) = self.battery() {
            i.battery = Some(b);
            i.asleep = false;
        }
        i.supported = true;
        i
    }

    fn caps(&self) -> Capabilities {
        self.caps.clone()
    }

    fn read(&self) -> HubResult<DeviceState> {
        self.maybe_detect();
        let m = self.model;
        // @0 ブロック: rate, stage数, active。
        let blk = self.ee_read(0, 10)?;
        let rate_code = blk[0];
        let stage_n = (blk[2] as usize).clamp(1, m.max_stages);
        let active = (blk[4] as usize).min(stage_n - 1);
        let rec_len = if m.dpi == DpiLayout::Sensor3955 { 6 } else { 4 };
        let base = if m.dpi == DpiLayout::Sensor3955 {
            ee::DPI_3955
        } else {
            ee::DPI_LEGACY
        };
        let stages_raw = self.ee_read(base, (stage_n * rec_len) as u16)?;
        let colors = self.ee_read(ee::DPI_COLOR, (stage_n * 4) as u16)?;
        let mut stages = vec![];
        for i in 0..stage_n {
            let rec = &stages_raw[i * rec_len..i * rec_len + rec_len];
            let (x, y) = if m.dpi == DpiLayout::Sensor3955 {
                parse_3955(rec)
            } else {
                parse_legacy(rec, m.dpi)
            };
            let c = &colors[i * 4..i * 4 + 4];
            stages.push(DpiStage {
                x,
                y: if y == x { None } else { Some(y) },
                color: Rgb {
                    r: c[0],
                    g: c[1],
                    b: c[2],
                },
                enabled: true,
            });
        }
        // ボタン @96 (4B × slot)。
        let nb = m.buttons.len();
        let keys = self.ee_read(ee::KEY_FUNCTION, (nb * 4) as u16)?;
        let buttons = (0..nb)
            .map(|i| self.decode_keyfn(i, &keys[i * 4..i * 4 + 4]))
            .collect();

        let mut settings = BTreeMap::new();
        let mut rate = rate_hz(rate_code);
        // 有線で 1K までの機種 (X2 CrazyLight): プロファイルには無線用の
        // 高いレートが残っていても実際は 1K で動く。
        if let Some(&max) = self.caps_rates().iter().max() {
            rate = rate.min(max);
        }
        settings.insert("polling_wireless".into(), SettingValue::Choice(rate));
        if let Ok(v) = self.ee_u8(ee::LOD) {
            if let Some(mm10) = lod_decode(m, v) {
                settings.insert("lod".into(), SettingValue::Choice(mm10));
            }
        }
        let mut flags = vec![
            (ee::MOTION_SYNC, "motion_sync"),
            (ee::ANGLE_SNAP, "angle_snap"),
            (ee::RIPPLE, "ripple"),
        ];
        if m.perf_mode.is_some() {
            flags.push((ee::PERF_MODE, "hyper_mode"));
        }
        if m.atk_hub {
            flags.push((ee::MOVE_LED, "move_close_led"));
        }
        for (addr, id) in flags {
            if let Ok(v) = self.ee_u8(addr) {
                settings.insert(id.into(), SettingValue::Bool(v != 0));
            }
        }
        if m.atk_hub {
            if let Ok(v) = self.ee_u8(ee::WHEEL_DEBOUNCE) {
                settings.insert("wheel_debounce".into(), SettingValue::Num(v as f32));
            }
            // @0 byte6 = bhop (ホイール誤操作防止、time/10)。
            settings.insert("bhop".into(), SettingValue::Num(blk[6] as f32));
        }
        if let Ok(v) = self.ee_u8(ee::DEBOUNCE) {
            settings.insert("debounce".into(), SettingValue::Num(v as f32));
        }
        let sleep_addr = if m.sleep_mirror {
            ee::SLEEP_TIME
        } else {
            ee::CLOSE_LED
        };
        if let Ok(v) = self.ee_u8(sleep_addr) {
            // wire = 10 秒単位 → 分。選択肢の中で最も近いものに寄せる。
            let min = ((v as i64 * 10) / 60).max(1);
            let near = m
                .sleep_opts
                .iter()
                .copied()
                .min_by_key(|o| (o - min).abs())
                .unwrap_or(min);
            settings.insert("sleep_min".into(), SettingValue::Choice(near));
        }
        // 注: cmd22/23 の長距離モードはドングル側機能 — 有線では cmd22 は ACK するが
        // cmd23 は常に 0 を返す (実機確認) ので設定には出さない。
        // オンボードプロファイル: cmd14 GetCurrentConfig → data[0] = index。
        let profile = if m.profiles > 0 {
            self.cmd_raw(14, 0, &[])
                .ok()
                .and_then(|r| r.get(6).copied())
                .unwrap_or(0)
                .min(m.profiles - 1)
        } else {
            0
        };
        Ok(DeviceState {
            profile: ProfileState {
                active: profile,
                names: vec![],
            },
            settings,
            mouse: Some(MouseState {
                stages,
                active_stage: active,
                buttons,
                macros: vec![],
                adaptive: None,
            }),
            keyboard: None,
            lighting: BTreeMap::new(),
            peq: None,
        })
    }

    fn apply(&self, change: &Change) -> HubResult<()> {
        let m = self.model;
        match change {
            Change::Setting { id, value } => match id.as_str() {
                "polling_wireless" => {
                    let hz = value.as_choice();
                    if !self.caps_rates().contains(&hz) {
                        return Err(HubError::Invalid(
                            "この接続では選べないポーリングレートです".into(),
                        ));
                    }
                    let code = rate_code(hz).ok_or(HubError::Invalid("polling rate".into()))?;
                    // 有線 1K 上限の機種: 保存済みの無線用高レートを 1K で潰さない
                    // (有線では実際に 1K で動いている — pulsar-mouse-linux と同じ判断)。
                    if !self.is_wireless()
                        && hz == 1000
                        && m.rates_wired.iter().max() < m.rates_wireless.iter().max()
                    {
                        let cur = self.ee_read(0, 1)?;
                        if rate_hz(cur[0]) > 1000 {
                            return Ok(());
                        }
                    }
                    self.cfg_byte(0, code)
                }
                "lod" => {
                    let code = lod_encode(m, value.as_choice())
                        .ok_or(HubError::Invalid("lod".into()))?;
                    self.ee_write_val(ee::LOD, code)
                }
                "motion_sync" => self.ee_write_val(ee::MOTION_SYNC, value.as_bool() as u8),
                "angle_snap" => self.ee_write_val(ee::ANGLE_SNAP, value.as_bool() as u8),
                "ripple" => self.ee_write_val(ee::RIPPLE, value.as_bool() as u8),
                "hyper_mode" if m.perf_mode.is_some() => {
                    self.ee_write_val(ee::PERF_MODE, value.as_bool() as u8)
                }
                "wheel_debounce" if m.atk_hub => {
                    self.ee_write_val(ee::WHEEL_DEBOUNCE, value.as_num().clamp(0., 255.) as u8)
                }
                "debounce" => self.ee_write_val(
                    ee::DEBOUNCE,
                    value.as_num().clamp(0., m.debounce_max) as u8,
                ),
                "sleep_min" => {
                    // 分 → 10 秒単位 (1分→6)。
                    let v = (value.as_choice().clamp(1, 42) * 6) as u8;
                    if m.sleep_mirror {
                        self.ee_write_val(ee::SLEEP_TIME, v)?;
                    }
                    self.ee_write_val(ee::CLOSE_LED, v)
                }
                "move_close_led" if m.atk_hub => {
                    self.ee_write_val(ee::MOVE_LED, value.as_bool() as u8)
                }
                "bhop" if m.atk_hub => self.cfg_byte(6, value.as_num().clamp(0., 25.) as u8),
                _ => Err(HubError::Unsupported("setting")),
            },
            Change::DpiStages { stages, active } => {
                if stages.is_empty() {
                    return Err(HubError::Invalid("dpi stage count".into()));
                }
                let n = stages.len().min(m.max_stages);
                self.ee_write_val(ee::MAX_DPI_STAGE, n as u8)?;
                self.ee_write_val(ee::CURRENT_DPI, (*active).min(n - 1) as u8)?;
                for (i, s) in stages[..n].iter().enumerate() {
                    let x = s.x.clamp(m.dpi_min, m.dpi_max);
                    let y = s.y.unwrap_or(s.x).clamp(m.dpi_min, m.dpi_max);
                    if m.dpi == DpiLayout::Sensor3955 {
                        self.ee_write(ee::DPI_3955 + (i * 6) as u16, &encode_3955(x, y))?;
                    } else {
                        self.ee_write(ee::DPI_LEGACY + (i * 4) as u16, &encode_legacy(x, y, m.dpi))?;
                    }
                    if m.stage_colors {
                        let mut c = vec![s.color.r, s.color.g, s.color.b, 0];
                        c[3] = crc(&c);
                        self.ee_write(ee::DPI_COLOR + (i * 4) as u16, &c)?;
                    }
                }
                Ok(())
            }
            Change::ActiveStage(a) => self.ee_write_val(ee::CURRENT_DPI, *a as u8),
            Change::Button { slot, action } => {
                if *slot >= m.buttons.len() {
                    return Err(HubError::Invalid("button slot".into()));
                }
                let rec = self.encode_keyfn(action)?;
                // ShortcutKey は本体領域を先に書いてから keyfn を指す。
                if let ButtonAction::Key(k) = action {
                    self.ee_write(ee::SHORTCUT + (*slot * 32) as u16, &self.encode_shortcut(k))?;
                }
                self.ee_write(ee::KEY_FUNCTION + (*slot * 4) as u16, &rec)
            }
            Change::Profile(i) if m.profiles > 0 => {
                self.cmd_raw(15, 0, &[(*i).min(m.profiles - 1)]).map(|_| ())
            }
            _ => Err(HubError::Unsupported("change")),
        }
    }

    fn run_action(&self, id: &str, _file: Option<&std::path::Path>) -> HubResult<ActionOutcome> {
        match id {
            "factory_reset" if self.model.factory_reset => {
                self.cmd_raw(9, 0, &[])?;
                Ok(ActionOutcome {
                    message: Some("工場出荷状態に戻しました".into()),
                    reload: true,
                })
            }
            _ => Err(HubError::Unsupported("action")),
        }
    }

    fn battery(&self) -> HubResult<Battery> {
        // cmd4: data[0]=percent, data[1]=chargingStatus。
        let r = self.cmd_raw(4, 0, &[])?;
        Ok(Battery {
            percent: Some(r.get(6).copied().unwrap_or(0).min(100)),
            charging: r.get(7).copied().unwrap_or(0) != 0,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sealed(b: &[u8]) -> bool {
        b.iter().fold(0u8, |a, &x| a.wrapping_add(x)) == 0x55
    }

    #[test]
    fn pulsar_dpi_matches_configurator_bytes() {
        // bbb.pulsar.gg: 800 DPI → raw 79, flags 0 (OpenMouse の実機照合)。
        let b = encode_legacy(800, 800, DpiLayout::PulsarFine);
        assert_eq!(&b[..3], &[79, 79, 0]);
        assert!(sealed(&b));
        // pulsar-mouse-linux: 0x27 → 400 DPI。
        assert_eq!(parse_legacy(&[0x27, 0x27, 0, 0], DpiLayout::PulsarFine), (400, 400));
    }

    #[test]
    fn pulsar_dpi_round_trips_all_bands() {
        for dpi in [10, 400, 1600, 3200, 10000, 10050, 12800, 25600, 30000, 30100, 32000] {
            let b = encode_legacy(dpi, dpi, DpiLayout::PulsarFine);
            assert!(sealed(&b), "{dpi}");
            assert_eq!(parse_legacy(&b, DpiLayout::PulsarFine), (dpi, dpi), "{dpi}");
        }
    }

    #[test]
    fn legacy50_round_trips_and_matches_atlantis_layout() {
        // 51200 上限の 10bit: 1600 → value 31, flags 0。
        let b = encode_legacy(1600, 1600, DpiLayout::Legacy50);
        assert_eq!(&b[..3], &[31, 31, 0]);
        for dpi in (50..=30000).step_by(50) {
            let b = encode_legacy(dpi, dpi, DpiLayout::Legacy50);
            assert_eq!(parse_legacy(&b, DpiLayout::Legacy50), (dpi, dpi));
        }
        // 26000 = value 519 = 0x207 → page 2 は x/y 両ニブルの bit2-3。
        let b = encode_legacy(26000, 26000, DpiLayout::Legacy50);
        assert_eq!(b[2], (2 << 2) | (2 << 6));
    }

    #[test]
    fn sensor3955_matches_vendor_encoder() {
        // 公式 encoder: 1600 DPI → 3f 06 3f 06 00 cb。
        assert_eq!(encode_3955(1600, 1600), vec![0x3f, 0x06, 0x3f, 0x06, 0x00, 0xcb]);
        assert_eq!(parse_3955(&[0x3f, 0x06, 0x3f, 0x06, 0x00, 0xcb]), (1600, 1600));
    }

    #[test]
    fn lod_tables() {
        assert_eq!(lod_encode(&SCYROX_V6, 7), Some(3));
        assert_eq!(lod_decode(&PULSAR_X2CL, 1), Some(10));
        assert_eq!(lod_encode(&ATK_F1V3U, 17), Some(11));
        assert_eq!(lod_encode(&SCYROX_V6, 13), None);
    }

    #[test]
    fn pulsar_init_frames_are_sealed() {
        for f in &PULSAR_INIT {
            assert!(sealed(f));
        }
    }

    #[test]
    fn caps_follow_transport() {
        let wired = build_caps(&PULSAR_X2CL, Transport::Wired);
        let opts = |c: &Capabilities| match &c.setting("polling_wireless").unwrap().kind {
            SettingKind::Choice { options } => options.iter().map(|o| o.value).max(),
            _ => None,
        };
        assert_eq!(opts(&wired), Some(1000));
        assert_eq!(opts(&build_caps(&PULSAR_X2CL, Transport::Wireless24G)), Some(8000));
        assert!(wired.setting("wheel_debounce").is_none());
        assert!(build_caps(&ATK_F1V3U, Transport::Wired).setting("wheel_debounce").is_some());
        assert_eq!(wired.mouse.unwrap().buttons.len(), 6);
    }
}
