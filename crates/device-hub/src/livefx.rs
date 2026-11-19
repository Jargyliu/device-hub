//! ライブライティング (DirectDrive) エンジン — 公式アプリに無い PC 駆動のキー照明。
//!
//! 磁気キーボードの **押し込み深さ (`live_travel`)** を読み、キーごとの色を計算して
//! DirectDrive (`DeviceDriver::direct_drive`) で実機 LED に直接流す。
//! 計算はこのモジュールの純関数 (UI スレッドの 30fps ティッカーから呼ぶ)。
//! I/O は `store::Hub` (travel 読み出し / dd_frame) が受け持つ。
//!
//! モード:
//! - `Depth` … 押し込んだ深さで色と明るさが変わる (アナログ感をそのまま光に)。
//! - `Heat`  … このセッションの打鍵回数ヒートマップ (よく使うキーほど赤く)。
//! - `Focus` … 選んだキー (WASD など) だけ点灯、他は暗く。押すと光る。
//! - `Ripple`… 押したキーから波紋が広がる (深く押すほど強い)。

use devhub_core::model::{KeyboardCaps, LayoutKey};
use devhub_core::Rgb;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum FxMode {
    #[default]
    Depth,
    Heat,
    Focus,
    Ripple,
    Status,
    Audio,
    AppFocus,
}

impl FxMode {
    pub const ALL: [FxMode; 7] = [
        FxMode::Depth,
        FxMode::Heat,
        FxMode::Focus,
        FxMode::Ripple,
        FxMode::Status,
        FxMode::Audio,
        FxMode::AppFocus,
    ];
    pub fn label(self) -> &'static str {
        match self {
            FxMode::Depth => "Press Depth",
            FxMode::Heat => "Keystroke Heatmap",
            FxMode::Focus => "Key Highlight",
            FxMode::Ripple => "Ripple",
            FxMode::Status => "Status",
            FxMode::Audio => "Audio",
            FxMode::AppFocus => "App Link",
        }
    }
    pub fn desc(self) -> &'static str {
        match self {
            FxMode::Depth => {
                "Color and brightness change continuously with press depth; the color switches past the actuation point"
            }
            FxMode::Heat => "Counts keystrokes and maps them to color. Most-used keys glow red (resettable)",
            FxMode::Focus => "Lights only the keys you use in games, like WASD. They glow when pressed",
            FxMode::Ripple => "Ripples spread from the pressed key. Deeper presses are stronger and faster",
            FxMode::Status => "Overlays Caps Lock and wireless low-battery indicators on the lighting (Hybrid recommended)",
            FxMode::Audio => "Shows a spectrum of the playing audio on the keyboard",
            FxMode::AppFocus => "Detects the foreground app and switches the highlighted keys automatically",
        }
    }
    pub fn icon(self) -> &'static str {
        match self {
            FxMode::Depth => "arrow-down-to-line",
            FxMode::Heat => "flame",
            FxMode::Focus => "crosshair",
            FxMode::Ripple => "radio",
            FxMode::Status => "activity",
            FxMode::Audio => "music",
            FxMode::AppFocus => "app-window",
        }
    }
}

/// 設定 (prefs.json の `livefx`)。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct FxPrefs {
    pub mode: FxMode,
    /// ハイブリッド: 無効セルは DD に送らずキーボード側のエフェクトに
    /// 任せる (ファームの光を背景に、押下/強調だけ PC 駆動 — 実機確認済み)。
    pub hybrid: bool,
    /// 背景色と明るさ (0..100)。hybrid では「無効セル」の判定にだけ使う。
    pub base: Rgb,
    pub base_level: u8,
    /// 主色 (浅い/強調キー/波紋の色)。
    pub a: Rgb,
    /// 副色 (深い/作動後/押下時)。
    pub b: Rgb,
    /// オーディオモードの感度 (%)。
    pub audio_gain: u8,
    /// 全体の明るさ 0..100。
    pub brightness: u8,
    /// 残光 (ms)。0 で即消灯。
    pub decay_ms: u32,
    /// キー強調で点灯するキー (`LayoutKey::label`)。
    pub focus: Vec<String>,
    /// アプリ連携: exe 名の部分一致 (小文字) → プリセット名。
    /// 前面アプリが一致すれば `focus_preset` のキー列で強調する。
    pub app_map: Vec<(String, String)>,
    /// 起動時 (このデバイスが見つかったら) 自動で開始。
    pub autostart: bool,
    /// 最後に動かしたデバイス (DeviceId)。
    pub device: Option<String>,
}

impl Default for FxPrefs {
    fn default() -> Self {
        Self {
            mode: FxMode::Depth,
            hybrid: false,
            base: Rgb::hex(0x4cb4ff),
            base_level: 8,
            a: Rgb::hex(0x34d6a8),
            b: Rgb::hex(0xff5c8a),
            audio_gain: 150,
            brightness: 90,
            decay_ms: 350,
            focus: focus_preset("FPS"),
            app_map: vec![
                ("valorant".into(), "FPS".into()),
                ("cs2".into(), "FPS".into()),
                ("apex".into(), "FPS".into()),
                ("dota".into(), "MOBA".into()),
                ("league".into(), "MOBA".into()),
            ],
            autostart: false,
            device: None,
        }
    }
}

/// キー強調のプリセット (刻印ラベル)。
pub const FOCUS_PRESETS: [&str; 4] = ["FPS", "WASD", "MOBA", "Number row"];

pub fn focus_preset(name: &str) -> Vec<String> {
    let v: &[&str] = match name {
        "WASD" => &["W", "A", "S", "D"],
        "MOBA" => &[
            "Q", "W", "E", "R", "D", "F", "1", "2", "3", "4", "5", "6", "B", "Tab",
        ],
        "Number row" => &["1", "2", "3", "4", "5", "6", "7", "8", "9", "0"],
        _ => &[
            "W", "A", "S", "D", "Q", "E", "R", "F", "C", "X", "G", "Z", "1", "2", "3", "4",
            "Shift", "LShift", "Ctrl", "LCtrl", "Space", "Tab", "Esc",
        ],
    };
    v.iter().map(|s| s.to_string()).collect()
}

/// 波紋 1 個。
#[derive(Clone, Copy, Debug)]
struct Ripple {
    x: f32,
    y: f32,
    age: f32,
    power: f32,
}

/// フレーム計算に使う外部入力 (Shell が 30fps ごとに集めて渡す)。
#[derive(Clone, Default)]
pub struct FxEnv {
    /// Caps Lock が有効か。
    pub caps_lock: bool,
    /// 他デバイスを含む最低のワイヤレス電池 (残量%, 充電中)。無ければ None。
    pub battery: Option<(u8, bool)>,
    /// オーディオバンド 0..1 (低音→高音)。`FxMode::Audio` 用。
    pub audio: Vec<f32>,
    /// アプリ連携で解決した強調キー (空なら `cfg.focus` にフォールバック)。
    pub app_focus: Vec<String>,
    /// 前面アプリの exe 名 (表示用)。
    pub fg_app: Option<String>,
}

/// 実行時状態 (セッション中だけ)。
#[derive(Default)]
pub struct Engine {
    /// 残光 0..1 (layout index)。
    glow: Vec<f32>,
    /// 作動中か (立ち上がり検出)。
    down: Vec<bool>,
    /// 打鍵回数。
    pub heat: Vec<u32>,
    ripples: Vec<Ripple>,
    /// 最後に作ったフレーム (UI のプレビューと共有)。
    pub last: Vec<Rgb>,
    /// 総打鍵数。
    pub presses: u64,
    /// DirectDrive が一度でも有効になったか (外部停止の検出用)。
    pub armed: bool,
    /// ライブ測定の再開を最後に試した時刻。
    pub rearm: Option<std::time::Instant>,
    /// エンジンの経過時間 (点滅・パルス用)。
    t: f32,
}

fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let t = t.clamp(0., 1.);
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Rgb::new(l(a.r, b.r), l(a.g, b.g), l(a.b, b.b))
}

fn scale(c: Rgb, k: f32) -> Rgb {
    let k = k.clamp(0., 1.);
    Rgb::new(
        (c.r as f32 * k).round() as u8,
        (c.g as f32 * k).round() as u8,
        (c.b as f32 * k).round() as u8,
    )
}

/// 「無効セル」の表示色。`frame` の各モードは未作動セルで
/// `scale(base, brightness/100)` を出すので、ハイブリッドでは
/// この色と一致するセルを DD に送らずファームのエフェクトに任せる。
/// (flag=0 = 「触らない」は実機確認済み — ピン留めしていないセルは
/// ファームがそのまま駆動する)
pub fn off_color(cfg: &FxPrefs) -> Rgb {
    scale(
        scale(cfg.base, cfg.base_level as f32 / 100.),
        cfg.brightness as f32 / 100.,
    )
}

/// 加算 (飽和)。
fn add(a: Rgb, b: Rgb) -> Rgb {
    Rgb::new(
        a.r.saturating_add(b.r),
        a.g.saturating_add(b.g),
        a.b.saturating_add(b.b),
    )
}

/// ヒートマップの配色 (0 = 青 → 緑 → 黄 → 赤)。
fn heat_color(t: f32) -> Rgb {
    const STOPS: [(f32, u32); 5] = [
        (0.0, 0x1e3a8a),
        (0.25, 0x0ea5e9),
        (0.5, 0x22c55e),
        (0.75, 0xf5c24c),
        (1.0, 0xff3b3b),
    ];
    let t = t.clamp(0., 1.);
    for w in STOPS.windows(2) {
        let (t0, c0) = w[0];
        let (t1, c1) = w[1];
        if t <= t1 {
            return mix(Rgb::hex(c0), Rgb::hex(c1), (t - t0) / (t1 - t0));
        }
    }
    Rgb::hex(0xff3b3b)
}

fn center(k: &LayoutKey) -> (f32, f32) {
    (k.x + k.w / 2., k.y + k.h / 2.)
}

/// 数字キー ("1".."0") → 0..9 のゲージ位置。
fn digit_pos(label: &str) -> Option<usize> {
    match label {
        "1" => Some(0),
        "2" => Some(1),
        "3" => Some(2),
        "4" => Some(3),
        "5" => Some(4),
        "6" => Some(5),
        "7" => Some(6),
        "8" => Some(7),
        "9" => Some(8),
        "0" => Some(9),
        _ => None,
    }
}

impl Engine {
    pub fn reset_heat(&mut self) {
        self.heat.iter_mut().for_each(|h| *h = 0);
        self.presses = 0;
    }

    /// 1 フレーム計算する。
    /// - `depths` … 押し込み深さ mm (layout 順。足りなければ 0)。
    /// - `actuation` … キーごとの作動点 mm (layout 順。足りなければ 1.2)。
    /// - `full` … 深さの正規化に使う全ストローク mm。
    /// - `dt` … 前フレームからの秒。
    /// - `env` … CapsLock / 電池 / オーディオ / 前面アプリ (Status・Audio・AppFocus 用)。
    pub fn frame(
        &mut self,
        cfg: &FxPrefs,
        kc: &KeyboardCaps,
        depths: &[f32],
        actuation: &[f32],
        full: f32,
        dt: f32,
        env: &FxEnv,
    ) -> &[Rgb] {
        let n = kc.layout.len();
        self.t += dt;
        self.glow.resize(n, 0.);
        self.down.resize(n, false);
        self.heat.resize(n, 0);
        self.last.resize(n, Rgb::new(0, 0, 0));
        let full = full.max(0.5);
        // 残光の減衰係数 (decay_ms で 1/e)。
        let fade = if cfg.decay_ms == 0 {
            0.
        } else {
            (-dt * 1000. / cfg.decay_ms as f32).exp()
        };
        let base = scale(cfg.base, cfg.base_level as f32 / 100.);
        // 押下の検出 (作動点 + ヒステリシス)
        for i in 0..n {
            let d = depths.get(i).copied().unwrap_or(0.).max(0.);
            let act = actuation
                .get(i)
                .copied()
                .filter(|a| *a > 0.05)
                .unwrap_or(1.2);
            let was = self.down[i];
            let now = if was {
                d >= (act - 0.15).max(0.05)
            } else {
                d >= act
            };
            if now && !was {
                self.heat[i] = self.heat[i].saturating_add(1);
                self.presses += 1;
                let (x, y) = center(&kc.layout[i]);
                if cfg.mode == FxMode::Ripple && self.ripples.len() < 48 {
                    self.ripples.push(Ripple {
                        x,
                        y,
                        age: 0.,
                        power: (d / full).clamp(0.4, 1.),
                    });
                }
            }
            self.down[i] = now;
            let lvl = (d / full).clamp(0., 1.);
            self.glow[i] = (self.glow[i] * fade).max(lvl);
        }
        // 波紋の更新
        for r in self.ripples.iter_mut() {
            r.age += dt * (0.8 + r.power);
        }
        self.ripples.retain(|r| r.age < 1.6);
        let max_heat = self.heat.iter().copied().max().unwrap_or(0).max(1) as f32;
        // Audio モードの座標系 (下端 y と左端 x の正規化)。
        let xmax = kc
            .layout
            .iter()
            .map(|k| k.x + k.w)
            .fold(0f32, f32::max)
            .max(1.);
        let ymax = kc
            .layout
            .iter()
            .map(|k| k.y + k.h)
            .fold(0f32, f32::max)
            .max(1.);
        // 電池ゲージの点滅位相 (40% 以下でゆっくり、15% 以下で速く)。
        let batt_pulse = self.t
            * if env.battery.is_some_and(|(p, _)| p <= 15) {
                6.
            } else {
                2.5
            };
        for i in 0..n {
            let k = &kc.layout[i];
            let g = self.glow[i];
            let act = actuation
                .get(i)
                .copied()
                .filter(|a| *a > 0.05)
                .unwrap_or(1.2);
            let c = match cfg.mode {
                FxMode::Depth => {
                    // 浅い→深いのグラデーション。作動点を越えたら副色側へ寄せる。
                    let d = depths.get(i).copied().unwrap_or(0.);
                    let t = if self.down[i] || d >= act {
                        0.65 + 0.35 * g
                    } else {
                        0.5 * (d / act).clamp(0., 1.)
                    };
                    let col = mix(cfg.a, cfg.b, t);
                    add(scale(base, 1. - g), scale(col, (g * 1.25).min(1.)))
                }
                FxMode::Heat => {
                    let h = self.heat[i];
                    let under = if h == 0 {
                        base
                    } else {
                        heat_color((h as f32 / max_heat).sqrt())
                    };
                    add(
                        scale(under, 1. - 0.6 * g),
                        scale(Rgb::new(255, 255, 255), g * 0.6),
                    )
                }
                FxMode::Focus => {
                    let on = cfg.focus.iter().any(|l| l.eq_ignore_ascii_case(&k.label));
                    let under = if on { cfg.a } else { base };
                    mix(under, cfg.b, g)
                }
                FxMode::Ripple => {
                    let (x, y) = center(k);
                    let mut s = 0f32;
                    for r in &self.ripples {
                        let rad = r.age * 9.;
                        let dist = ((x - r.x).powi(2) + ((y - r.y) * 1.1).powi(2)).sqrt();
                        let band = (1. - ((dist - rad).abs() / 1.3)).max(0.);
                        s += band * r.power * (1. - r.age / 1.6);
                    }
                    let s = s.min(1.);
                    let ring = mix(cfg.a, cfg.b, (s * 1.4).min(1.));
                    add(scale(base, 1. - s), scale(ring, s.max(g)))
                }
                FxMode::Status => {
                    // 背景はファーム/ベース色のまま、情報のあるキーだけ重ねる。
                    let mut c = base;
                    // CapsLock → Caps キーを副色で点灯。
                    if env.caps_lock
                        && (k.label.eq_ignore_ascii_case("caps")
                            || k.label.eq_ignore_ascii_case("capslock"))
                    {
                        c = cfg.b;
                    }
                    // ワイヤレス機の電池が 40% 以下 → 数字行をゲージに
                    // (1 キー = 10%、色は残量で赤→黄→緑、点滅で注意喚起)。
                    if let Some((pct, chg)) = env.battery {
                        if pct <= 40 {
                            if let Some(pos) = digit_pos(&k.label) {
                                let lit = pos * 10 < pct.max(1) as usize;
                                if lit {
                                    let meter = mix(
                                        Rgb::hex(0xff3b3b),
                                        Rgb::hex(0x22c55e),
                                        pct as f32 / 100.,
                                    );
                                    let pulse = 0.55 + 0.45 * batt_pulse.sin().abs();
                                    c = if chg { meter } else { scale(meter, pulse) };
                                }
                            }
                        }
                    }
                    add(scale(c, 1. - g), scale(cfg.b, g * 0.4))
                }
                FxMode::Audio => {
                    // 列ごとのバンドレベルを「下からの高さ」に変換した
                    // スペクトル。上に行くほど色が副色に近づく。
                    let nb = env.audio.len();
                    if nb == 0 {
                        base
                    } else {
                        let bi = (((k.x + k.w / 2.) / xmax) * nb as f32) as usize;
                        let lvl =
                            (env.audio[bi.min(nb - 1)] * cfg.audio_gain as f32 / 100.).min(1.);
                        let h = lvl * ymax;
                        let depth = ymax - (k.y + k.h); // 下端からの高さ
                        if depth < h {
                            // 頂点に近いほど副色・明るく。
                            let top = (1. - (h - depth).abs() / 1.2).clamp(0., 1.);
                            let col = mix(cfg.a, cfg.b, (depth / ymax) * 0.8 + top * 0.4);
                            add(scale(base, 0.4), scale(col, 0.25 + 0.75 * top))
                        } else {
                            scale(base, 0.4)
                        }
                    }
                }
                FxMode::AppFocus => {
                    // 前面アプリに対応するキー (未解決なら手動リスト)。
                    let keys = if env.app_focus.is_empty() {
                        &cfg.focus
                    } else {
                        &env.app_focus
                    };
                    let on = keys.iter().any(|l| l.eq_ignore_ascii_case(&k.label));
                    let under = if on { cfg.a } else { base };
                    mix(under, cfg.b, g)
                }
            };
            self.last[i] = scale(c, cfg.brightness as f32 / 100.);
        }
        &self.last
    }

    /// 打鍵回数の多い順 (刻印, 回数)。
    pub fn top_keys(&self, kc: &KeyboardCaps, n: usize) -> Vec<(String, u32)> {
        let mut v: Vec<(String, u32)> = kc
            .layout
            .iter()
            .zip(&self.heat)
            .filter(|(_, h)| **h > 0)
            .map(|(k, h)| (k.label.clone(), *h))
            .collect();
        v.sort_by_key(|b| std::cmp::Reverse(b.1));
        v.truncate(n);
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use devhub_core::model::LayoutKey;

    fn kc() -> KeyboardCaps {
        let mut kc = devhub_core::demo::keyboard_caps().keyboard.unwrap();
        kc.layout = vec![
            LayoutKey {
                id: 1,
                x: 0.,
                y: 0.,
                w: 1.,
                h: 1.,
                label: "W".into(),
            },
            LayoutKey {
                id: 2,
                x: 1.,
                y: 0.,
                w: 1.,
                h: 1.,
                label: "K".into(),
            },
        ];
        kc
    }

    #[::core::prelude::v1::test]
    fn counts_presses_once_with_hysteresis() {
        let k = kc();
        let cfg = FxPrefs::default();
        let mut e = Engine::default();
        for d in [0., 1.5, 1.2, 1.5, 0.2, 1.6] {
            e.frame(
                &cfg,
                &k,
                &[d, 0.],
                &[1.2, 1.2],
                4.,
                0.033,
                &FxEnv::default(),
            );
        }
        assert_eq!(e.heat[0], 2);
        assert_eq!(e.presses, 2);
    }

    #[::core::prelude::v1::test]
    fn hybrid_off_cells_match_off_color() {
        let k = kc();
        let mut e = Engine::default();
        // どのモードでも「何もしていないキー」は off_color と一致する
        // (= DD に送らずファームに任せる判定が成り立つ)。
        for mode in FxMode::ALL {
            let mut cfg = FxPrefs {
                mode,
                hybrid: true,
                brightness: 100,
                ..Default::default()
            };
            if mode == FxMode::Focus {
                cfg.focus = vec!["W".into()];
            }
            let f = e
                .frame(&cfg, &k, &[0., 0.], &[], 4., 0.033, &FxEnv::default())
                .to_vec();
            assert_eq!(f[1], off_color(&cfg), "{mode:?} idle cell");
            if mode == FxMode::Focus {
                assert_eq!(f[0], cfg.a);
            }
        }
    }

    #[::core::prelude::v1::test]
    fn focus_lights_only_selected() {
        let k = kc();
        let cfg = FxPrefs {
            mode: FxMode::Focus,
            base_level: 0,
            brightness: 100,
            ..Default::default()
        };
        let mut e = Engine::default();
        let f = e
            .frame(&cfg, &k, &[0., 0.], &[], 4., 0.033, &FxEnv::default())
            .to_vec();
        assert_eq!(f[0], cfg.a);
        assert_eq!(f[1], Rgb::new(0, 0, 0));
    }

    #[::core::prelude::v1::test]
    fn status_caps_and_battery() {
        // "Caps"(y=0) と "1"(数字キー) のレイアウトで検証。
        let mut k = kc();
        k.layout[0].label = "Caps".into();
        k.layout[1].label = "1".into();
        let cfg = FxPrefs {
            mode: FxMode::Status,
            base_level: 0,
            brightness: 100,
            ..Default::default()
        };
        let mut e = Engine::default();
        // Caps ON + 電池 25% (充電中) → Caps=副色、"1"(pos0) は 25% 内で点灯。
        let env = FxEnv {
            caps_lock: true,
            battery: Some((25, true)),
            ..Default::default()
        };
        let f = e.frame(&cfg, &k, &[0., 0.], &[], 4., 0.033, &env).to_vec();
        assert_eq!(f[0], cfg.b, "caps on");
        assert_ne!(f[1], Rgb::new(0, 0, 0), "battery gauge lit");
        // 電池が 50% → ゲージ消灯。Caps も OFF。
        let env2 = FxEnv {
            caps_lock: false,
            battery: Some((50, false)),
            ..Default::default()
        };
        let f2 = e.frame(&cfg, &k, &[0., 0.], &[], 4., 0.033, &env2).to_vec();
        assert_eq!(f2[0], Rgb::new(0, 0, 0));
        assert_eq!(f2[1], Rgb::new(0, 0, 0));
    }

    #[::core::prelude::v1::test]
    fn audio_column_height() {
        // 2列 × y0(上) のキー。低バンドしか音が無い → 低バンド列だけ点灯。
        let mut k = kc();
        k.layout[0].x = 0.;
        k.layout[1].x = 10.; // 別バンド列
        let cfg = FxPrefs {
            mode: FxMode::Audio,
            base_level: 0,
            brightness: 100,
            ..Default::default()
        };
        let mut audio = vec![0f32; crate::audio::NBANDS];
        audio[0] = 1.;
        let env = FxEnv {
            audio,
            ..Default::default()
        };
        let mut e = Engine::default();
        let f = e.frame(&cfg, &k, &[0., 0.], &[], 4., 0.033, &env).to_vec();
        assert_ne!(f[0], Rgb::new(0, 0, 0), "loud band lit");
        assert_eq!(f[1], Rgb::new(0, 0, 0), "quiet band dark");
    }

    #[::core::prelude::v1::test]
    fn appfocus_uses_resolved_keys() {
        let k = kc(); // "W", "K"
        let cfg = FxPrefs {
            mode: FxMode::AppFocus,
            base_level: 0,
            brightness: 100,
            focus: vec!["K".into()],
            ..Default::default()
        };
        let mut e = Engine::default();
        let env = FxEnv {
            app_focus: vec!["W".into()],
            ..Default::default()
        };
        let f = e.frame(&cfg, &k, &[0., 0.], &[], 4., 0.033, &env).to_vec();
        assert_eq!(f[0], cfg.a, "resolved W lit");
        assert_eq!(f[1], Rgb::new(0, 0, 0), "K not in app list");
    }
}
