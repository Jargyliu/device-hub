//! Ninjutso Sora V2 — NinjaForce 旧世代プロトコル (Nordic VID 1915、feature report 5 / 4)。
//!
//! 出典 (docs/PROTOCOL_NOTES.md §9): OpenMouse `@openmouse/protocol/ninjutso`
//! (NinjaForce の WebHID クライアントから転記、実機未確認) と Sora_tray
//! (Sora V2 実機で電池読み出しを確認) の事実情報を照合して自前で実装した。
//! **設定の読み書きは実機未確認** — 書き込みは毎回 report 4 を読み直して検証する。
//!
//! - 設定 IF: usage page FFA0 (Sora_tray と同じ)。
//! - コマンド: feature report 5 (31B): `[cmd, 0, 0, 1, 0, profile, 22, 0, value…]`。
//!   応答も report 5 を読む (15ms 待つ)。
//! - 設定一式: feature report 4 に `[38, 0, 0, 1, 0, profile]` を送り 90ms 後に読む。
//!   長さは 703B (8K 受信機 "Sora V2 8K" は 73B)。末尾 2B が先頭からの単純和 (u16LE)。
//!   データ (report ID 込みの 9 バイト目以降): [7]=電池%, [8]=充電中, [11]=段数,
//!   [12+2i]=段 i の DPI (u16LE, (v+1)×50), [20]=有効段, [21]=システムモード(1始まり),
//!   [22]=LOD (0低 1中 2高), [23]=モーション同期, [24]=レート (0..3 = 1K..8K),
//!   [25]=Slam Click (0..2)。
//! - 書き込み cmd: 5=DPI 段 (value 位置に段数 + 段×u16LE), 6=モーション同期,
//!   10=レート (log2(Hz/1000)), 11=LOD, 14=有効段, 23=システムモード (+1),
//!   36=Slam Click (profile 位置に 1)。読み: 9=FW, 13=プロファイル, 21=電池, 40=ペア先 PID。

use devhub_core::backend::*;
use devhub_core::model::*;
use std::collections::BTreeMap;
use std::sync::Mutex;
use std::thread::sleep;
use std::time::Duration;

const RID_CMD: u8 = 5;
const RID_SETTINGS: u8 = 4;
/// report 5 = ID + 31B。
const CMD_LEN: usize = 32;
/// report 4 の最大 (ID + 703B)。
const SETTINGS_LEN: usize = 704;

/// 8K 受信機 (1K–8K を選べる)。1K 受信機 AE1C と有線は 1000Hz 固定。
const HIGH_RATE_RECEIVERS: &[u16] = &[0xAE8C, 0xAE8A];
/// PAW3950 版 Sora V2 (LOD「低」を持つ)。
const SORA_3950: &[u16] = &[0xAE14, 0xAE15, 0xAE16];
const RATES: &[i64] = &[1000, 2000, 4000, 8000];

/// コマンドフレーム (report ID 込み 32B)。
pub fn cmd_frame(command: u8, profile: u8, value: Option<u8>) -> [u8; CMD_LEN] {
    let mut f = [0u8; CMD_LEN];
    f[0] = RID_CMD;
    f[1] = command;
    f[4] = 1;
    f[6] = profile;
    f[7] = 22;
    if let Some(v) = value {
        f[9] = v;
    }
    f
}

pub fn encode_dpi(dpi: u32) -> [u8; 2] {
    let v = (dpi.clamp(50, 30000) / 50 - 1) as u16;
    v.to_le_bytes()
}

pub fn decode_dpi(lo: u8, hi: u8) -> u32 {
    u16::from_le_bytes([lo, hi]) as u32 * 50 + 50
}

/// report 4 の検証 (先頭から n-3 までの和 = 末尾 u16LE) とデータ部の切り出し。
pub fn parse_settings(buf: &[u8]) -> Option<Vec<u8>> {
    let n = buf.len();
    if n < 40 {
        return None;
    }
    let sum: u32 = buf[..n - 3].iter().map(|&b| b as u32).sum();
    let want = u16::from_le_bytes([buf[n - 2], buf[n - 1]]) as u32;
    (sum & 0xffff == want).then(|| buf[9..].to_vec())
}

pub struct NinjutsoDriver {
    dev: Mutex<hidapi::HidDevice>,
    info: Mutex<DeviceInfo>,
    /// ペア先 (有線は自分) の PID。3950 判定用。
    effective_pid: Mutex<Option<u16>>,
}

pub fn open(dev: hidapi::HidDevice, info: DeviceInfo) -> HubResult<NinjutsoDriver> {
    Ok(NinjutsoDriver {
        dev: Mutex::new(dev),
        info: Mutex::new(info),
        effective_pid: Mutex::new(None),
    })
}

impl NinjutsoDriver {
    fn wireless(&self) -> bool {
        self.info
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .transport
            == Transport::Wireless24G
    }

    fn pid(&self) -> u16 {
        self.info.lock().unwrap_or_else(|p| p.into_inner()).pid
    }

    fn high_rate(&self) -> bool {
        HIGH_RATE_RECEIVERS.contains(&self.pid())
    }

    /// report 5 を送って応答を読む。
    fn xfer(&self, f: &[u8; CMD_LEN]) -> HubResult<[u8; CMD_LEN]> {
        let dev = self.dev.lock().unwrap_or_else(|p| p.into_inner());
        dev.send_feature_report(f)
            .map_err(|e| HubError::Io(format!("ninjutso write: {e}")))?;
        sleep(Duration::from_millis(15));
        let mut r = [0u8; CMD_LEN];
        r[0] = RID_CMD;
        dev.get_feature_report(&mut r)
            .map_err(|e| HubError::Io(format!("ninjutso read: {e}")))?;
        Ok(r)
    }

    /// 書き込み (応答は読み捨て — 結果は report 4 で検証する)。
    fn write(&self, f: &[u8; CMD_LEN]) -> HubResult<()> {
        self.xfer(f).map(|_| ())
    }

    fn profile(&self) -> HubResult<u8> {
        let f = cmd_frame(13, 0, None);
        let mut r = self.xfer(&f)?;
        if r[9] == 0 {
            r = self.xfer(&f)?;
        }
        Ok(r[9])
    }

    /// 設定一式 (profile, data)。
    fn settings(&self) -> HubResult<(u8, Vec<u8>)> {
        let profile = self.profile()?;
        let dev = self.dev.lock().unwrap_or_else(|p| p.into_inner());
        let mut req = vec![0u8; SETTINGS_LEN];
        req[0] = RID_SETTINGS;
        req[1..7].copy_from_slice(&[38, 0, 0, 1, 0, profile]);
        dev.send_feature_report(&req)
            .map_err(|e| HubError::Io(format!("ninjutso settings write: {e}")))?;
        sleep(Duration::from_millis(90));
        let mut buf = vec![0u8; SETTINGS_LEN];
        buf[0] = RID_SETTINGS;
        let n = dev
            .get_feature_report(&mut buf)
            .map_err(|e| HubError::Io(format!("ninjutso settings read: {e}")))?;
        drop(dev);
        let data = parse_settings(&buf[..n.min(SETTINGS_LEN)])
            .or_else(|| parse_settings(&buf[..74.min(n)]))
            .ok_or_else(|| HubError::Protocol("ninjutso: settings checksum".into()))?;
        if data.len() < 27 {
            return Err(HubError::Protocol("ninjutso: short settings".into()));
        }
        Ok((profile, data))
    }

    fn effective_pid(&self) -> u16 {
        if let Some(p) = *self.effective_pid.lock().unwrap_or_else(|p| p.into_inner()) {
            return p;
        }
        let pid = if self.wireless() {
            let mut f = [0u8; CMD_LEN];
            f[0] = RID_CMD;
            f[1..8].copy_from_slice(&[40, 0, 0, 1, 0, 0, 2]);
            match self.xfer(&f) {
                Ok(r) if r[9] != 0 || r[10] != 0 => u16::from_le_bytes([r[9], r[10]]),
                _ => return self.pid(),
            }
        } else {
            self.pid()
        };
        *self.effective_pid.lock().unwrap_or_else(|p| p.into_inner()) = Some(pid);
        pid
    }

    fn is_3950(&self) -> bool {
        SORA_3950.contains(&self.effective_pid())
    }

    fn firmware(&self, selector: u8) -> Option<String> {
        let mut f = cmd_frame(9, 0, None);
        f[6] = selector;
        let r = self.xfer(&f).ok()?;
        let s = format!("{:02X}{:02X}{:02X}{:02X}", r[12], r[11], r[10], r[9]);
        (s != "00000000").then_some(s)
    }

    /// 書いて、設定を読み直して `check` を満たすか確かめる。
    fn write_verified(
        &self,
        f: [u8; CMD_LEN],
        what: &str,
        check: impl Fn(&[u8]) -> bool,
    ) -> HubResult<()> {
        self.write(&f)?;
        let (_, d) = self.settings()?;
        if check(&d) {
            Ok(())
        } else {
            Err(HubError::Protocol(format!("{what} didn't take effect")))
        }
    }
}

fn opts(v: &[(i64, &str)]) -> Vec<ChoiceOpt> {
    v.iter().map(|(a, b)| ChoiceOpt::new(*a, *b)).collect()
}

pub fn build_caps(high_rate: bool, sora_3950: bool) -> Capabilities {
    use section::*;
    let mut settings = vec![];
    if high_rate {
        settings.push(
            SettingSpec::choice(
                "polling_wireless",
                PERFORMANCE,
                "Polling rate",
                opts(&[(1000, "1K"), (2000, "2K"), (4000, "4K"), (8000, "8K")]),
            )
            .help("Only on the 8K receiver (wired and 1K receiver are fixed at 1000Hz)"),
        );
    }
    settings.extend([
        SettingSpec::choice(
            "system_mode",
            PERFORMANCE,
            "System mode",
            opts(&[(1, "High-Speed"), (2, "Competitive")]),
        )
        .help("Competitive mode prioritizes response; battery life is shorter"),
        SettingSpec::choice(
            "lod",
            SENSOR,
            "Lift-off distance",
            if sora_3950 {
                opts(&[(0, "Low"), (1, "Medium"), (2, "High")])
            } else {
                opts(&[(1, "Medium"), (2, "High")])
            },
        )
        .help("Height at which it stops tracking when lifted"),
        SettingSpec::toggle("motion_sync", SENSOR, "Motion sync")
            .help("Syncs pointer and mouse movement for smoother tracking"),
        SettingSpec::choice(
            "slam_click",
            CLICK,
            "Slam Click",
            opts(&[(0, "Low"), (1, "Medium"), (2, "High")]),
        )
        .help("How strongly to suppress false clicks from the impact of setting the mouse down"),
    ]);
    Capabilities {
        settings,
        actions: vec![],
        profiles: None,
        mouse: Some(MouseCaps {
            dpi_min: 50,
            dpi_max: 30000,
            dpi_step: 50,
            min_stages: 1,
            max_stages: 4,
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

impl DeviceDriver for NinjutsoDriver {
    fn info(&self) -> DeviceInfo {
        let mut i = self.info.lock().unwrap_or_else(|p| p.into_inner()).clone();
        match self.battery() {
            Ok(b) => {
                i.battery = Some(b);
                i.asleep = false;
            }
            Err(HubError::Disconnected) => i.asleep = i.transport == Transport::Wireless24G,
            Err(_) => {}
        }
        if !i.asleep {
            let wireless = i.transport == Transport::Wireless24G;
            i.firmware = self.firmware(if wireless { 0 } else { 4 });
            if wireless {
                i.dongle_firmware = self.firmware(4);
            }
        }
        i.supported = true;
        i
    }

    fn caps(&self) -> Capabilities {
        build_caps(self.high_rate(), self.is_3950())
    }

    fn read(&self) -> HubResult<DeviceState> {
        let (_, d) = self.settings()?;
        let n = (d[11] as usize).clamp(1, 4);
        let active = (d[20] as usize).min(n - 1);
        let stages = (0..n)
            .map(|i| DpiStage {
                x: decode_dpi(d[12 + i * 2], d[13 + i * 2]),
                y: None,
                color: Rgb::new(255, 255, 255),
                enabled: true,
            })
            .collect();
        let mut settings = BTreeMap::new();
        if self.high_rate() {
            settings.insert(
                "polling_wireless".into(),
                SettingValue::Choice(RATES.get(d[24] as usize).copied().unwrap_or(1000)),
            );
        }
        settings.insert(
            "system_mode".into(),
            SettingValue::Choice(d[21].clamp(1, 2) as i64),
        );
        settings.insert("lod".into(), SettingValue::Choice(d[22].min(2) as i64));
        settings.insert("motion_sync".into(), SettingValue::Bool(d[23] == 1));
        settings.insert(
            "slam_click".into(),
            SettingValue::Choice(d[25].min(2) as i64),
        );
        Ok(DeviceState {
            profile: ProfileState::default(),
            settings,
            mouse: Some(MouseState {
                stages,
                active_stage: active,
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
        let profile = self.profile()?;
        match change {
            Change::Setting { id, value } => match id.as_str() {
                "polling_wireless" if self.high_rate() => {
                    let hz = value.as_choice();
                    let code = RATES
                        .iter()
                        .position(|&r| r == hz)
                        .ok_or(HubError::Invalid("polling rate".into()))?
                        as u8;
                    self.write_verified(
                        cmd_frame(10, profile, Some(code)),
                        "Polling rate",
                        |d| d[24] == code,
                    )
                }
                "lod" => {
                    let v = value.as_choice();
                    let min = if self.is_3950() { 0 } else { 1 };
                    if !(min..=2).contains(&v) {
                        return Err(HubError::Invalid("lod".into()));
                    }
                    let v = v as u8;
                    self.write_verified(cmd_frame(11, profile, Some(v)), "LOD", |d| d[22] == v)
                }
                "motion_sync" => {
                    let v = value.as_bool() as u8;
                    self.write_verified(
                        cmd_frame(6, profile, Some(v)),
                        "Motion sync",
                        |d| d[23] == v,
                    )
                }
                "system_mode" => {
                    let v = value.as_choice().clamp(1, 2) as u8;
                    self.write_verified(
                        cmd_frame(23, profile, Some(v)),
                        "System mode",
                        |d| d[21] == v,
                    )
                }
                "slam_click" => {
                    let v = value.as_choice().clamp(0, 2) as u8;
                    // NinjaForce は Slam Click だけ profile 位置に 1 を置く。
                    let mut f = cmd_frame(36, profile, Some(v));
                    f[6] = 1;
                    self.write_verified(f, "Slam Click", |d| d[25] == v)
                }
                _ => Err(HubError::Unsupported("setting")),
            },
            Change::DpiStages { stages, active } => {
                if stages.is_empty() {
                    return Err(HubError::Invalid("dpi stage count".into()));
                }
                let n = stages.len().min(4);
                let mut f = cmd_frame(5, profile, Some(n as u8));
                let mut want = vec![];
                for (i, s) in stages[..n].iter().enumerate() {
                    let b = encode_dpi(s.x);
                    f[10 + i * 2] = b[0];
                    f[11 + i * 2] = b[1];
                    want.push(b);
                }
                self.write_verified(f, "DPI", |d| {
                    d[11] as usize == n
                        && want
                            .iter()
                            .enumerate()
                            .all(|(i, b)| d[12 + i * 2] == b[0] && d[13 + i * 2] == b[1])
                })?;
                let a = (*active).min(n - 1) as u8;
                self.write_verified(cmd_frame(14, profile, Some(a)), "Active DPI stage", |d| {
                    d[20] == a
                })
            }
            Change::ActiveStage(a) => {
                let a = *a as u8;
                self.write_verified(cmd_frame(14, profile, Some(a)), "Active DPI stage", |d| {
                    d[20] == a
                })
            }
            _ => Err(HubError::Unsupported("change")),
        }
    }

    fn battery(&self) -> HubResult<Battery> {
        // Sora_tray (実機で確認済みのフレームそのまま): [5,21,0,0,1,0…]。
        // 応答 [9]=電池%, [10]=充電中, [12]=接続中。
        let mut f = [0u8; CMD_LEN];
        f[0] = RID_CMD;
        f[1] = 21;
        f[4] = 1;
        let r = self.xfer(&f)?;
        if self.wireless() && r[12] == 0 {
            return Err(HubError::Disconnected);
        }
        Ok(Battery {
            percent: Some(r[9].min(100)),
            charging: r[10] != 0,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_frame_layout() {
        // NinjaForce 旧世代: [cmd, 0, 0, 1, 0, profile, 22, 0, value]。
        let f = cmd_frame(11, 2, Some(1));
        assert_eq!(&f[..10], &[5, 11, 0, 0, 1, 0, 2, 22, 0, 1]);
        assert_eq!(f.len(), 32);
    }

    #[test]
    fn dpi_codec() {
        assert_eq!(encode_dpi(800), [15, 0]);
        assert_eq!(decode_dpi(15, 0), 800);
        for d in (50..=30000).step_by(50) {
            let b = encode_dpi(d);
            assert_eq!(decode_dpi(b[0], b[1]), d);
        }
    }

    #[test]
    fn settings_checksum() {
        let mut buf = vec![0u8; 74];
        buf[0] = 4;
        buf[9 + 11] = 2;
        buf[9 + 12] = 15;
        let sum: u32 = buf[..71].iter().map(|&b| b as u32).sum();
        buf[72..74].copy_from_slice(&(sum as u16).to_le_bytes());
        let d = parse_settings(&buf).expect("valid");
        assert_eq!(d[11], 2);
        assert_eq!(decode_dpi(d[12], d[13]), 800);
        buf[20] ^= 1;
        assert!(parse_settings(&buf).is_none());
    }
}
