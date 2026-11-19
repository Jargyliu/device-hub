//! Finalmouse UltralightX (XPANEL) — 2.4G ドングル 361D:0100 の 64B report プロトコル。
//!
//! 出典 (docs/PROTOCOL_NOTES.md §8): ulx_reverse (MIT、ULX 実機の USB キャプチャ) と
//! OpenMouse `@openmouse/protocol/finalmouse` (xpanel の JS から転記) の事実情報を
//! 照合して自前で実装した。実機未確認。
//!
//! - 設定 IF: usage page FF00 / usage 01 (XPANEL も同じ IF にしか繋がない)。
//!   XPANEL はドングル経由専用 — 有線のマウス本体 (361D:0102) では設定できない。
//! - 送信 output report 4: `[4, 2+n, 0x80|cmd, n, payload…]` (64B)。
//!   payload 無しで送ると「問い合わせ」。
//! - 受信 input report 5: `[5, len, cmd, n, payload…]`。ドングル自身の情報は
//!   output 2 → input 3。
//! - 現在値は個別の問い合わせ + `wakeAll` (96) で状態一式が別々の report で届く。
//!   ULX は DPI を cmd 3、ポーリングを cmd 4 で報告する (設定は 16 / 17)。
//! - DPI は単一値 (ステージ無し)、ポーリングは Hz の u16LE。

use devhub_core::backend::*;
use devhub_core::model::*;
use std::collections::BTreeMap;
use std::sync::Mutex;
use std::thread::sleep;
use std::time::{Duration, Instant};

const REPORT_MAIN: u8 = 4;
const REPORT_MAIN_IN: u8 = 5;
const REPORT_DONGLE: u8 = 2;
const REPORT_DONGLE_IN: u8 = 3;
const FRAME: usize = 64;

mod cmd {
    pub const DPI_REPORT: u8 = 3;
    pub const RATE_REPORT: u8 = 4;
    pub const BATTERY_MV: u8 = 5;
    pub const DONGLE_INFO: u8 = 10;
    pub const DONGLE_RF_FW: u8 = 11;
    pub const MOUSE_FW: u8 = 12;
    pub const MOUSE_SERIAL: u8 = 14;
    pub const DPI: u8 = 16;
    pub const RATE: u8 = 17;
    pub const MOTION_SYNC: u8 = 18;
    pub const DONGLE_LED: u8 = 20;
    pub const LOD: u8 = 21;
    pub const LINK: u8 = 36;
    pub const CHARGING: u8 = 37;
    pub const BATTERY: u8 = 38;
    pub const WAKE_ALL: u8 = 96;
}

const RATES: &[i64] = &[500, 1000, 2000, 4000, 8000];
const DPI_MIN: u32 = 50;
const DPI_MAX: u32 = 26000;

/// 64B フレーム (report ID 込み)。
pub fn frame(report: u8, command: u8, payload: &[u8]) -> [u8; FRAME] {
    let n = payload.len().min(FRAME - 4);
    let mut f = [0u8; FRAME];
    f[0] = report;
    f[1] = 2 + n as u8;
    f[2] = 0x80 | (command & 0x7f);
    f[3] = n as u8;
    f[4..4 + n].copy_from_slice(&payload[..n]);
    f
}

/// 受信した状態の寄せ集め。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Telemetry {
    pub dpi: Option<u32>,
    pub rate: Option<u32>,
    pub battery_mv: Option<u16>,
    pub soc: Option<u8>,
    pub charging: Option<bool>,
    pub link: Option<bool>,
    pub motion_sync: Option<bool>,
    pub dongle_led: Option<u8>,
    pub lod: Option<u8>,
    pub mouse_fw: Option<String>,
    pub dongle_rf_fw: Option<String>,
    pub dongle_usb_fw: Option<String>,
    pub serial: Option<String>,
}

fn ascii(p: &[u8]) -> String {
    // XPANEL は末尾 1 バイトを除いて読む (終端)。
    let n = if p.len() > 1 { p.len() - 1 } else { p.len() };
    String::from_utf8_lossy(&p[..n])
        .trim_end_matches(['\0', ' '])
        .to_string()
}

impl Telemetry {
    /// input report 1 件 (report ID 込み) を取り込む。取り込めたら true。
    pub fn absorb(&mut self, buf: &[u8]) -> bool {
        if buf.len() < 4 {
            return false;
        }
        let rid = buf[0];
        if rid != REPORT_MAIN_IN && rid != REPORT_DONGLE_IN {
            return false;
        }
        let inner = (buf[1] as usize).min(buf.len() - 2);
        let body = &buf[2..2 + inner];
        if body.len() < 2 {
            return false;
        }
        let c = body[0] & 0x7f;
        let n = (body[1] as usize).min(body.len() - 2);
        let p = &body[2..2 + n];
        let u16le = || (p.len() >= 2).then(|| u16::from_le_bytes([p[0], p[1]]));
        let b0 = p.first().copied();
        if rid == REPORT_DONGLE_IN {
            if c == cmd::DONGLE_INFO && !p.is_empty() {
                self.dongle_usb_fw = Some(ascii(p));
                return true;
            }
            return false;
        }
        match c {
            cmd::DPI_REPORT | cmd::DPI => self.dpi = u16le().map(u32::from),
            cmd::RATE_REPORT | cmd::RATE => self.rate = u16le().map(u32::from),
            cmd::BATTERY_MV => self.battery_mv = u16le(),
            cmd::DONGLE_RF_FW => self.dongle_rf_fw = Some(ascii(p)),
            cmd::MOUSE_FW => self.mouse_fw = Some(ascii(p)),
            cmd::MOUSE_SERIAL => self.serial = Some(ascii(p)),
            cmd::MOTION_SYNC => self.motion_sync = b0.map(|v| v == 1),
            cmd::DONGLE_LED => self.dongle_led = b0,
            cmd::LOD => self.lod = b0,
            cmd::LINK => self.link = b0.map(|v| v != 0),
            cmd::CHARGING => self.charging = b0.map(|v| v == 1),
            cmd::BATTERY => self.soc = b0,
            _ => return false,
        }
        true
    }

    pub fn battery_percent(&self) -> Option<u8> {
        self.soc
            .map(|s| s.min(100))
            .or_else(|| self.battery_mv.filter(|&mv| mv > 0).map(battery_curve))
    }
}

/// XPANEL の ULX 電圧カーブ (mV → %)。
pub fn battery_curve(mv: u16) -> u8 {
    const V: [f32; 7] = [3.0, 3.62, 3.66, 3.74, 3.88, 4.17, 4.38];
    const P: [f32; 7] = [0.2, 5., 10., 25., 50., 75., 100.];
    let v = mv as f32 / 1000.;
    if v >= V[6] {
        return 100;
    }
    if v <= V[0] {
        return 0;
    }
    for i in 0..6 {
        if v >= V[i] && v <= V[i + 1] {
            let t = (v - V[i]) / (V[i + 1] - V[i]);
            return (P[i] + (P[i + 1] - P[i]) * t).round() as u8;
        }
    }
    0
}

pub struct FinalmouseDriver {
    dev: Mutex<hidapi::HidDevice>,
    info: Mutex<DeviceInfo>,
    tel: Mutex<Telemetry>,
}

pub fn open(dev: hidapi::HidDevice, info: DeviceInfo) -> HubResult<FinalmouseDriver> {
    dev.set_blocking_mode(false).ok();
    Ok(FinalmouseDriver {
        dev: Mutex::new(dev),
        info: Mutex::new(info),
        tel: Mutex::new(Telemetry::default()),
    })
}

impl FinalmouseDriver {
    fn send(
        &self,
        dev: &hidapi::HidDevice,
        report: u8,
        command: u8,
        payload: &[u8],
    ) -> HubResult<()> {
        dev.write(&frame(report, command, payload))
            .map(|_| ())
            .map_err(|e| HubError::Io(format!("finalmouse write: {e}")))
    }

    /// 届いている input report を `until` まで読み、テレメトリへ取り込む。
    fn pump(&self, dev: &hidapi::HidDevice, until: Instant) {
        let mut tel = self.tel.lock().unwrap_or_else(|p| p.into_inner());
        let mut buf = [0u8; FRAME + 1];
        loop {
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            match dev.read_timeout(&mut buf, left.as_millis().min(50) as i32) {
                Ok(n) if n > 0 => {
                    tel.absorb(&buf[..n]);
                }
                Ok(_) => {}
                Err(_) => break,
            }
        }
    }

    /// 状態一式を問い合わせる (XPANEL の readStatus と同じ順)。
    /// マウスが応答すれば DPI とポーリングが埋まる。
    fn refresh(&self) -> HubResult<Telemetry> {
        let dev = self.dev.lock().unwrap_or_else(|p| p.into_inner());
        {
            let mut t = self.tel.lock().unwrap_or_else(|p| p.into_inner());
            t.dpi = None;
            t.rate = None;
        }
        self.send(&dev, REPORT_MAIN, cmd::LOD, &[])?;
        self.send(&dev, REPORT_MAIN, cmd::WAKE_ALL, &[])?;
        self.send(&dev, REPORT_DONGLE, cmd::DONGLE_INFO, &[])?;
        let deadline = Instant::now() + Duration::from_millis(1000);
        loop {
            self.pump(
                &dev,
                (Instant::now() + Duration::from_millis(75)).min(deadline),
            );
            let t = self.tel.lock().unwrap_or_else(|p| p.into_inner()).clone();
            if (t.dpi.is_some() && t.rate.is_some()) || Instant::now() >= deadline {
                // 状態は別々の report で続けて届く — 少し余計に拾う。
                self.pump(&dev, Instant::now() + Duration::from_millis(75));
                break;
            }
        }
        drop(dev);
        let t = self.tel.lock().unwrap_or_else(|p| p.into_inner()).clone();
        self.info.lock().unwrap_or_else(|p| p.into_inner()).asleep = t.dpi.is_none();
        Ok(t)
    }

    /// 設定コマンドを送り、届いた応答を取り込む。
    fn set(&self, command: u8, payload: &[u8]) -> HubResult<()> {
        let dev = self.dev.lock().unwrap_or_else(|p| p.into_inner());
        self.send(&dev, REPORT_MAIN, command, payload)?;
        self.pump(&dev, Instant::now() + Duration::from_millis(60));
        Ok(())
    }
}

pub fn build_caps() -> Capabilities {
    use section::*;
    let opts = |v: &[(i64, &str)]| v.iter().map(|(a, b)| ChoiceOpt::new(*a, *b)).collect();
    Capabilities {
        settings: vec![
            SettingSpec::choice(
                "polling_wireless",
                PERFORMANCE,
                "ポーリングレート",
                RATES
                    .iter()
                    .map(|&hz| {
                        let l = if hz >= 1000 {
                            format!("{}K", hz / 1000)
                        } else {
                            hz.to_string()
                        };
                        ChoiceOpt::new(hz, l)
                    })
                    .collect(),
            )
            .help("マウスの報告頻度 (Hz)"),
            SettingSpec::choice(
                "lod",
                SENSOR,
                "リフトオフ距離",
                opts(&[(10, "1mm"), (20, "2mm")]),
            )
            .help("持ち上げて反応しなくなる高さ"),
            SettingSpec::toggle("motion_sync", SENSOR, "モーション同期")
                .help("ポインタとマウスの動きを同期させ、軌跡を滑らかにします"),
            SettingSpec::choice(
                "dongle_led",
                LIGHTING,
                "ドングル LED",
                opts(&[(0, "オフ"), (1, "バッテリー表示"), (2, "白")]),
            )
            .help("受信機のインジケーター"),
        ],
        actions: vec![],
        profiles: None,
        mouse: Some(MouseCaps {
            dpi_min: DPI_MIN,
            dpi_max: DPI_MAX,
            dpi_step: 50,
            min_stages: 1,
            max_stages: 1,
            xy_split: false,
            stage_colors: false,
            stage_toggle: false,
            buttons: vec![],
            button_kinds: vec![],
            require_left_click: false,
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

impl DeviceDriver for FinalmouseDriver {
    fn info(&self) -> DeviceInfo {
        let t = self.refresh().unwrap_or_default();
        let mut i = self.info.lock().unwrap_or_else(|p| p.into_inner()).clone();
        i.firmware = t.mouse_fw.clone().filter(|s| !s.is_empty());
        i.dongle_firmware = match (&t.dongle_usb_fw, &t.dongle_rf_fw) {
            (Some(u), Some(r)) => Some(format!("USB {u} / RF {r}")),
            (Some(u), None) => Some(u.clone()),
            (None, Some(r)) => Some(r.clone()),
            _ => None,
        };
        if i.serial.is_none() {
            i.serial = t.serial.clone().filter(|s| !s.is_empty());
        }
        i.battery = t.battery_percent().map(|p| Battery {
            percent: Some(p),
            charging: t.charging.unwrap_or(false),
        });
        i.supported = true;
        i
    }

    fn caps(&self) -> Capabilities {
        build_caps()
    }

    fn read(&self) -> HubResult<DeviceState> {
        let t = self.refresh()?;
        let Some(dpi) = t.dpi else {
            return Err(HubError::Disconnected);
        };
        let mut settings = BTreeMap::new();
        if let Some(r) = t.rate {
            settings.insert("polling_wireless".into(), SettingValue::Choice(r as i64));
        }
        if let Some(l) = t.lod {
            settings.insert("lod".into(), SettingValue::Choice(l as i64 * 10));
        }
        if let Some(m) = t.motion_sync {
            settings.insert("motion_sync".into(), SettingValue::Bool(m));
        }
        if let Some(l) = t.dongle_led {
            settings.insert("dongle_led".into(), SettingValue::Choice(l as i64));
        }
        Ok(DeviceState {
            profile: ProfileState::default(),
            settings,
            mouse: Some(MouseState {
                stages: vec![DpiStage {
                    x: dpi,
                    y: None,
                    color: Rgb::new(255, 255, 255),
                    enabled: true,
                }],
                active_stage: 0,
                buttons: vec![],
                macros: vec![],
                adaptive: None,
            }),
            keyboard: None,
            lighting: BTreeMap::new(),
            peq: None,
        })
    }

    fn apply(&self, change: &Change) -> HubResult<()> {
        match change {
            Change::Setting { id, value } => match id.as_str() {
                "polling_wireless" => {
                    let hz = value.as_choice();
                    if !RATES.contains(&hz) {
                        return Err(HubError::Invalid("polling rate".into()));
                    }
                    self.set(cmd::RATE, &(hz as u16).to_le_bytes())
                }
                "lod" => match value.as_choice() {
                    10 => self.set(cmd::LOD, &[1]),
                    20 => self.set(cmd::LOD, &[2]),
                    _ => Err(HubError::Invalid("lod".into())),
                },
                "motion_sync" => self.set(cmd::MOTION_SYNC, &[value.as_bool() as u8]),
                "dongle_led" => {
                    let v = value.as_choice();
                    if !(0..=2).contains(&v) {
                        return Err(HubError::Invalid("dongle led".into()));
                    }
                    self.set(cmd::DONGLE_LED, &[v as u8])
                }
                _ => Err(HubError::Unsupported("setting")),
            },
            Change::DpiStages { stages, .. } => {
                let s = stages.first().ok_or(HubError::Invalid("dpi".into()))?;
                let dpi = s.x.clamp(DPI_MIN, DPI_MAX) as u16;
                self.set(cmd::DPI, &dpi.to_le_bytes())?;
                // XPANEL は書いた値を応答で返す — 少し待ってから次へ。
                sleep(Duration::from_millis(20));
                Ok(())
            }
            Change::ActiveStage(_) => Ok(()),
            _ => Err(HubError::Unsupported("change")),
        }
    }

    fn battery(&self) -> HubResult<Battery> {
        let t = self.refresh()?;
        t.battery_percent()
            .map(|p| Battery {
                percent: Some(p),
                charging: t.charging.unwrap_or(false),
            })
            .ok_or(HubError::Disconnected)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_match_ulx_capture() {
        // ulx_reverse "Mouse Info": DPI 1600 = 04 04 90 02 40 06、
        // Polling 4000 = 04 04 91 02 a0 0f、Motion Sync On = 04 03 92 01 01、
        // LOD 2mm = 04 03 95 01 02、Dongle LED White = 04 03 94 01 02。
        assert_eq!(
            &frame(4, cmd::DPI, &1600u16.to_le_bytes())[..6],
            &[4, 4, 0x90, 2, 0x40, 6]
        );
        assert_eq!(
            &frame(4, cmd::RATE, &4000u16.to_le_bytes())[..6],
            &[4, 4, 0x91, 2, 0xa0, 0x0f]
        );
        assert_eq!(&frame(4, cmd::MOTION_SYNC, &[1])[..5], &[4, 3, 0x92, 1, 1]);
        assert_eq!(&frame(4, cmd::LOD, &[2])[..5], &[4, 3, 0x95, 1, 2]);
        assert_eq!(&frame(4, cmd::DONGLE_LED, &[2])[..5], &[4, 3, 0x94, 1, 2]);
        assert_eq!(frame(4, cmd::WAKE_ALL, &[]).len(), 64);
        assert_eq!(&frame(4, cmd::WAKE_ALL, &[])[..4], &[4, 2, 0xe0, 0]);
    }

    #[test]
    fn absorbs_status_burst() {
        let mut t = Telemetry::default();
        assert!(t.absorb(&[5, 4, 3, 2, 0x40, 0x06]));
        assert!(t.absorb(&[5, 4, 4, 2, 0xa0, 0x0f]));
        assert!(t.absorb(&[5, 3, 18, 1, 1]));
        assert!(t.absorb(&[5, 3, 38, 1, 77]));
        assert!(!t.absorb(&[1, 0, 0, 0]));
        assert_eq!(t.dpi, Some(1600));
        assert_eq!(t.rate, Some(4000));
        assert_eq!(t.motion_sync, Some(true));
        assert_eq!(t.battery_percent(), Some(77));
    }

    #[test]
    fn battery_curve_endpoints() {
        assert_eq!(battery_curve(4400), 100);
        assert_eq!(battery_curve(2900), 0);
        assert_eq!(battery_curve(3880), 50);
    }
}
