//! LAMZU Aurora (www.lamzu.net) LM20 世代 — と同じ 64B feature report
//! プロトコルを話す WLMOUSE (gm.wlmouse.gg) — の機種表と Capabilities。
//!
//! ドライバ (`devhub-hid::lamzu`) とデモ (`demo.rs`) の両方が使う。
//! 値はすべて公式 `Config/env-models.json` (v1.0.38) と設定画面
//! `index-Bn9q780G.js` の LM20 経路 (`T2()`) から取っている
//! (docs/LAMZU_MAYA_X_V2_AUDIT2.md)。表示名は公式 Aurora の日本語表示に合わせる。

use crate::model::*;

/// LM20 世代の 1 機種。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LamzuModel {
    /// 有線 PID (ドングル経由ではペア先 PID として返る)。
    pub pid: u16,
    /// 製品名 (公式製品ページの表記)。
    pub name: &'static str,
    /// 内部モデル id。
    pub model: &'static str,
    /// 公式機種定義の行名。
    pub row: &'static str,
    pub sensor_offset: bool,
    pub adaptive_dpi: bool,
    pub glass_mode: bool,
    /// メーカー差 (LAMZU / WLMOUSE)。
    pub v: &'static Vendor,
}

/// 同じプロトコル上のメーカー差。LAMZU LM20 と WLMOUSE (WL MOUSE HUB v1.0.47) は
/// フレーム・コマンド番号・ポーリング符号が同一で、機能の有無と一部の引数だけが違う。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vendor {
    pub vid: u16,
    pub brand: &'static str,
    /// 高速トリガー (cmd 154/26)・スクロール Bhop (153/25)・受信機 LED。
    pub lamzu_extras: bool,
    /// cmd 139/11 のラベル。
    pub hyper_label: &'static str,
    pub hyper_help: &'static str,
    /// cmd 147/19 のラベル (cmd 139 が ON のときだけ有効)。
    pub tracking_label: &'static str,
    pub tracking_help: &'static str,
    /// センサー型番コマンド (143) を持つか。持たなければ `sensor` を使う。
    pub sensor_cmd: bool,
    pub sensor: u8,
    pub max_stages: usize,
    pub profiles: u8,
    /// LOD 書き込みが `[p, lod, 3]` (WLMOUSE) か `[p, lod]` (LAMZU) か。
    pub lod_mode3: bool,
    /// スリープ選択肢 (分)。0 = スリープしない (0xFFFF を書く)。
    pub sleep_opts: &'static [i64],
    pub rates_wired: &'static [i64],
    pub rates_wireless: &'static [i64],
    /// 1K 受信機の PID (この受信機経由では 125–1000 Hz)。
    pub dongle_1k: &'static [u16],
}

pub const LAMZU: Vendor = Vendor {
    vid: 0x37B0,
    brand: "LAMZU",
    lamzu_extras: true,
    hyper_label: "競技モード",
    hyper_help: "OFF 時はハイスピードモード。最高の性能と電池持ちを両立するよう調整されたモード",
    tracking_label: "20K FPS",
    tracking_help: "20kHz スキャンの極限性能モード。競技モードより電力消費が大きい",
    sensor_cmd: true,
    sensor: 4,
    max_stages: 5,
    profiles: 4,
    lod_mode3: false,
    sleep_opts: &[1, 5, 10, 15, 20, 25],
    // 公式: 無線は PollingRateWireless = 1000;2000;4000;8000。
    rates_wired: &[500, 1000, 2000, 4000, 8000],
    rates_wireless: &[1000, 2000, 4000, 8000],
    dongle_1k: &[],
};

/// WLMOUSE — models.json (WL MOUSE HUB) の BEAST G 行: IsNewProtocol=1,
/// IsSensorModelCmd=0 (PAW3950 固定)、DPIMaxStageNum=6、LOD 0.7/1/2、
/// 有線・8K 受信機とも 125–8000、1K 受信機 (A882) は 125–1000。
pub const WLMOUSE: Vendor = Vendor {
    vid: 0x36A7,
    brand: "WLMOUSE",
    lamzu_extras: false,
    hyper_label: "ハイスピードモード",
    hyper_help: "センサーを高性能モードで動かします (電池の持ちは短くなります)",
    tracking_label: "トラッキングモード",
    tracking_help: "ハイスピードモード中だけ使える追加のトラッキング性能モード",
    sensor_cmd: false,
    sensor: 2,
    max_stages: 6,
    profiles: 3,
    lod_mode3: true,
    sleep_opts: &[0, 1, 2, 5, 10, 15, 20, 30],
    rates_wired: &[125, 250, 500, 1000, 2000, 4000, 8000],
    rates_wireless: &[125, 250, 500, 1000, 2000, 4000, 8000],
    dongle_1k: &[0xA882],
};

/// 公式機種定義の LM20 行 (Mini は受信機が 0054)。
pub const MODELS: &[LamzuModel] = &[
    LamzuModel {
        pid: 0x0044,
        name: "LAMZU MAYA X V2",
        model: "maya-x-v2",
        row: "Maya X(LM20)",
        sensor_offset: true,
        adaptive_dpi: true,
        glass_mode: true,
        v: &LAMZU,
    },
    LamzuModel {
        pid: 0x0050,
        name: "LAMZU MAYA V2",
        model: "maya-v2",
        row: "Maya(LM20)",
        sensor_offset: true,
        adaptive_dpi: true,
        glass_mode: true,
        v: &LAMZU,
    },
    LamzuModel {
        pid: 0x0052,
        name: "LAMZU MAYA M",
        model: "maya-m",
        row: "MAYA M(LM20)",
        sensor_offset: true,
        adaptive_dpi: true,
        glass_mode: true,
        v: &LAMZU,
    },
    LamzuModel {
        pid: 0x0056,
        name: "LAMZU ORCUS V2",
        model: "orcus-v2",
        row: "Orcus V2(LM20)",
        sensor_offset: false,
        adaptive_dpi: false,
        glass_mode: false,
        v: &LAMZU,
    },
    // WLMOUSE BEAST G (Firenzai 等のカラー違いも同じ PID)。有線 A861、8K 受信機 A860、
    // 1K 受信機 A882 — 受信機経由は GetDevicePID のペア先 (A861) で引く。
    LamzuModel {
        pid: 0xA861,
        name: "WLMOUSE BEAST G",
        model: "wl-beast-g",
        row: "BEAST G",
        sensor_offset: false,
        adaptive_dpi: false,
        glass_mode: false,
        v: &WLMOUSE,
    },
    LamzuModel {
        pid: 0x0046,
        name: "LAMZU ATLANTIS MINI V2",
        model: "atlantis-mini-v2",
        row: "Mini(LM20)",
        sensor_offset: true,
        adaptive_dpi: true,
        glass_mode: true,
        v: &LAMZU,
    },
];

pub fn model_by_pid(pid: u16) -> Option<&'static LamzuModel> {
    MODELS.iter().find(|m| m.pid == pid)
}

/// VID も照合する版 (受信機のペア先問い合わせはメーカーごと)。
pub fn model_by_id(vid: u16, pid: u16) -> Option<&'static LamzuModel> {
    MODELS.iter().find(|m| m.v.vid == vid && m.pid == pid)
}

/// 受信機の VID で既定の機種 (ペア先が分かるまでの caps 用)。
pub fn default_for_vid(vid: u16) -> &'static LamzuModel {
    MODELS.iter().find(|m| m.v.vid == vid).unwrap_or(&MODELS[0])
}

/// センサー型番コマンド (cmd 143) の応答 → (表示名, DPI 上限, DPI 刻み, LOD 候補 (mm×10))。
/// 公式 `T2()`: 1=PAW3395 (26000, 1/2mm)、2=PAW3950 (30000, 0.7/1/2mm)、
/// 4=PAW3955 (50000, 0.7/1/1.3/1.5/1.7mm、1 刻み)。不明は機種定義の既定 (50000, 0.7/1/2)。
pub fn sensor_profile(code: u8) -> (&'static str, u32, u32, &'static [i64]) {
    match code {
        1 => ("PAW3395", 26000, 50, &[10, 20]),
        2 => ("PAW3950", 30000, 50, &[7, 10, 20]),
        4 => ("PAW3955", 50000, 1, &[7, 10, 13, 15, 17]),
        _ => ("", 50000, 50, &[7, 10, 20]),
    }
}

/// 公式の速度連動 DPI プリセット (設定画面 `AdaDPI*PointList`)。
pub fn adaptive_presets() -> Vec<(String, Vec<AdaptivePoint>)> {
    let p = |v: &[(u16, f32)]| {
        v.iter()
            .map(|&(speed, ratio)| AdaptivePoint { speed, ratio })
            .collect::<Vec<_>>()
    };
    vec![
        ("クラシック".into(), p(&[(0, 1.0), (20, 1.0), (40, 1.0)])),
        ("ナチュラル".into(), p(&[(0, 1.5), (20, 0.75), (40, 0.1)])),
        ("リープ".into(), p(&[(0, 0.1), (20, 0.75), (40, 1.5)])),
    ]
}

fn opts(v: &[(i64, &str)]) -> Vec<ChoiceOpt> {
    v.iter().map(|(a, b)| ChoiceOpt::new(*a, *b)).collect()
}

/// 機種・接続・センサーに合わせた Capabilities。
pub fn caps(model: &LamzuModel, transport: Transport, sensor: u8) -> Capabilities {
    caps_for(model, transport, sensor, 0)
}

/// `receiver_pid` = 接続している受信機の PID (有線なら 0)。1K 受信機の判定に使う。
pub fn caps_for(model: &LamzuModel, transport: Transport, sensor: u8, receiver_pid: u16) -> Capabilities {
    use section::*;
    let v = model.v;
    let wireless = transport == Transport::Wireless24G;
    let sensor = if v.sensor_cmd { sensor } else { v.sensor };
    let (_, dpi_max, dpi_step, lods) = sensor_profile(sensor);
    let rates: Vec<i64> = if !wireless {
        v.rates_wired.to_vec()
    } else if v.dongle_1k.contains(&receiver_pid) {
        v.rates_wireless.iter().copied().filter(|&r| r <= 1000).collect()
    } else {
        v.rates_wireless.to_vec()
    };
    let polling: Vec<ChoiceOpt> = rates
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
    let lod_opts: Vec<ChoiceOpt> = lods
        .iter()
        .map(|&v| {
            let label = if v % 10 == 0 {
                format!("{}mm", v / 10)
            } else {
                format!("{:.1}mm", v as f32 / 10.)
            };
            ChoiceOpt::new(v, &label)
        })
        .collect();

    let mut settings = vec![
        SettingSpec::choice("polling_wireless", PERFORMANCE, "ポーリングレート", polling)
            .help("マウスのデータ更新頻度 (Hz)"),
        SettingSpec::toggle("hyper_mode", PERFORMANCE, v.hyper_label).help(v.hyper_help),
        SettingSpec::toggle("tracking_mode", PERFORMANCE, v.tracking_label)
            .help(v.tracking_help)
            .enabled_when(
                "hyper_mode",
                SettingValue::Bool(true),
                &format!("先に{}を ON にしてください", v.hyper_label),
            )
            .confirm(&format!(
                "{}は電池の持ちが大幅に短くなります。切り替えますか？",
                v.tracking_label
            )),
        SettingSpec::choice("lod", SENSOR, "LOD", lod_opts)
            .help("マウスがトラッキング面から離れ、センサーが反応しなくなる高さ"),
    ];
    if model.glass_mode {
        settings.push(
            SettingSpec::toggle("glass_mode", SENSOR, "ガラスモード")
                .help("ガラス製マウスパッドを自動検知・適応し、LOD 調整を無効にします"),
        );
        // ガラスモード中は LOD を触らせない (公式と同じ)。
        if let Some(l) = settings.iter_mut().find(|s| s.id == "lod") {
            l.enabled_when = Some((
                "glass_mode".into(),
                SettingValue::Bool(false),
                "ガラスモード中は LOD を変更できません".into(),
            ));
        }
    }
    settings.extend([
        SettingSpec::toggle("motion_sync", SENSOR, "移動同期")
            .help("PC の処理にマウスの動きを同期させ、軌跡をより正確にします"),
        SettingSpec::toggle("angle_snap", SENSOR, "直線補正")
            .help("マウス入力をより直線的で正確にします"),
        SettingSpec::toggle("ripple", SENSOR, "リップル")
            .help("高速なマウス動作中にポインタの波状の揺れを排除します"),
        SettingSpec::slider("angle_tune", SENSOR, "角度調整", -30., 30., 1., "°")
            .help("グリップに合わせてセンサー角度を補正し、斜めに持っても水平に動かせます"),
    ]);
    if model.sensor_offset {
        settings.push(
            SettingSpec::slider("sensor_offset", SENSOR, "センサー中心オフセット", -5., 5., 1., "")
                .help("センサーの中心位置の補正 (公式 SensorCenterOffset)"),
        );
    }
    settings.push(
        SettingSpec::slider("debounce", CLICK, "デバウンスタイム", 0., 15., 1., "ms").help(
            "低すぎるとダブルクリックが起きることがあります。ゲーム向けの推奨は 2–4ms",
        ),
    );
    if v.lamzu_extras {
        settings.extend([
            SettingSpec::toggle("rapid_left", CLICK, "高速トリガー (左クリック)")
                .help("アルゴリズムでキー応答を最速化 — 誤ってダブルクリックする可能性があります"),
            SettingSpec::toggle("rapid_right", CLICK, "高速トリガー (右クリック)")
                .help("アルゴリズムでキー応答を最速化 — 誤ってダブルクリックする可能性があります"),
            SettingSpec::choice(
                "bhop_dir",
                CLICK,
                "スクロールBhop",
                opts(&[(0, "オフ"), (2, "スクロールアップ"), (3, "スクロールダウン")]),
            )
            .help("最初のスクロール信号を無効化し、応答時間内に 2 回目が無ければスクロールしません"),
            SettingSpec::choice(
                "bhop_ms",
                CLICK,
                "ホイール応答時間",
                opts(&[
                    (200, "200ms"),
                    (400, "400ms"),
                    (600, "600ms"),
                    (800, "800ms"),
                    (1000, "1000ms"),
                ]),
            )
            .help("スクロールBhop で 2 回目のスクロールを待つ時間"),
        ]);
    }
    settings.push(
        SettingSpec::choice(
            "sleep_min",
            POWER,
            "スリープ時間",
            v.sleep_opts
                .iter()
                .map(|&m| {
                    ChoiceOpt::new(m, if m == 0 { "しない".to_string() } else { format!("{m}分") })
                })
                .collect(),
        )
        .help("一定時間無操作だとスリープします"),
    );
    if wireless && v.lamzu_extras {
        settings.push(
            SettingSpec::toggle("dongle_led", LIGHTING, "ドングルLED")
                .help("レシーバーのライトでマウスの状態を表示"),
        );
        for (id, name) in [
            ("dongle_led1", "LED 1"),
            ("dongle_led2", "LED 2"),
            ("dongle_led3", "LED 3"),
        ] {
            settings.push(
                SettingSpec::choice(
                    id,
                    LIGHTING,
                    name,
                    opts(&[(6, "バッテリー"), (11, "DPI"), (10, "ポーリングレート"), (12, "信号")]),
                )
                .when("dongle_led", SettingValue::Bool(true)),
            );
        }
    }
    Capabilities {
        settings,
        actions: vec![
            ActionSpec::new("reset_profile", section::SYSTEM, "プロファイルをリセット")
                .help("現在のプロファイルだけを工場出荷時の値に戻します")
                .danger("現在のプロファイルの設定がすべて初期化されます"),
            ActionSpec::new("factory_reset", section::SYSTEM, "完全初期化")
                .help(if wireless {
                    "マウスとレシーバーを工場出荷時のデフォルト設定に戻します"
                } else {
                    "マウスを工場出荷時のデフォルト設定に戻します"
                })
                .danger("OK を押すと、マウスは工場出荷時のデフォルト設定に戻ります"),
        ],
        profiles: Some(ProfileCaps {
            count: v.profiles,
            renamable: false,
        }),
        mouse: Some(MouseCaps {
            dpi_min: 50,
            dpi_max,
            dpi_step,
            min_stages: 1,
            max_stages: v.max_stages,
            xy_split: true,
            stage_colors: true,
            stage_toggle: false,
            buttons: ["左クリック", "右クリック", "ホイールクリック", "戻る", "進む"]
                .iter()
                .map(|n| ButtonSlot { name: (*n).into() })
                .collect(),
            button_kinds: vec![
                ButtonActionKind::Mouse,
                ButtonActionKind::Key,
                ButtonActionKind::Media,
                ButtonActionKind::Dpi,
                ButtonActionKind::Fire,
                ButtonActionKind::PollingCycle,
                ButtonActionKind::ProfileCycle,
                ButtonActionKind::Disabled,
            ],
            require_left_click: true,
            macros: None,
            live_buttons: vec![],
            adaptive_dpi: model.adaptive_dpi.then(|| AdaptiveCaps {
                min_points: 2,
                max_points: 5,
                speed_max: 105,
                ratio_max: 1.5,
                presets: adaptive_presets(),
            }),
        }),
        keyboard: None,
        lighting: vec![],
        battery: true,
        peq: None,
        firmware_feed: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rates(c: &Capabilities) -> Vec<i64> {
        match &c.setting("polling_wireless").unwrap().kind {
            SettingKind::Choice { options } => options.iter().map(|o| o.value).collect(),
            _ => vec![],
        }
    }

    #[test]
    fn beast_g_caps() {
        let m = model_by_id(0x36A7, 0xA861).expect("beast g");
        let c = caps_for(m, Transport::Wireless24G, 4, 0xA860);
        assert_eq!(rates(&c), vec![125, 250, 500, 1000, 2000, 4000, 8000]);
        assert_eq!(c.mouse.as_ref().unwrap().max_stages, 6);
        assert_eq!(c.mouse.as_ref().unwrap().dpi_max, 30000); // PAW3950 固定
        assert!(c.setting("rapid_left").is_none());
        assert!(c.setting("dongle_led").is_none());
        assert!(c.setting("glass_mode").is_none());
        let one_k = caps_for(m, Transport::Wireless24G, 4, 0xA882);
        assert_eq!(rates(&one_k), vec![125, 250, 500, 1000]);
    }

    #[test]
    fn lamzu_caps_unchanged() {
        let c = caps(&MODELS[0], Transport::Wireless24G, 4);
        assert_eq!(rates(&c), vec![1000, 2000, 4000, 8000]);
        assert!(c.setting("rapid_left").is_some());
        assert!(c.setting("dongle_led").is_some());
        assert_eq!(c.mouse.unwrap().max_stages, 5);
    }
}
