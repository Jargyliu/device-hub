//! アプリ (Device HUB) 側で編集する OBS オーバーレイの見た目と表示キー。
//!
//! `%APPDATA%\DeviceHub\obs_overlay.json` を Device HUB の「OBS オーバーレイ」画面が書き、
//! OBS プラグインは 1 秒ごとに更新時刻を見て読み直す (OBS を再起動しなくても反映)。
//! OBS 側のプロパティ (キー単位チェック等) と**和集合**で効く。

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

/// 見た目のテーマ。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Theme {
    /// 半透明の暗いパネル (既定)。
    #[default]
    Glass,
    /// 明るいパネル。
    Light,
    /// 背景なし (キーだけ。配信画面に直接重ねる用)。
    Clear,
    /// 黒地 + アクセントの輪郭と発光。
    Neon,
}

impl Theme {
    pub const ALL: [Theme; 4] = [Theme::Glass, Theme::Light, Theme::Clear, Theme::Neon];
    pub fn label(self) -> &'static str {
        match self {
            Theme::Glass => "Glass",
            Theme::Light => "Light",
            Theme::Clear => "No background",
            Theme::Neon => "Neon",
        }
    }
}

/// マウスの描き方。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum MouseStyle {
    /// マウスの形 + 横バー (既定)。
    #[default]
    Both,
    /// マウスの形だけ。
    Shape,
    /// 横バーだけ。
    Bars,
}

impl MouseStyle {
    pub const ALL: [MouseStyle; 3] = [MouseStyle::Both, MouseStyle::Shape, MouseStyle::Bars];
    pub fn label(self) -> &'static str {
        match self {
            MouseStyle::Both => "Shape + bar",
            MouseStyle::Shape => "Shape only",
            MouseStyle::Bars => "Bar only",
        }
    }
}

/// 押し込みフィルの方向。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum FillMode {
    /// 下から満ちる (既定)。
    #[default]
    Bottom,
    /// 上から沈む。
    Top,
    /// 深さに応じてキー全体が発色する。
    Fade,
}

impl FillMode {
    pub const ALL: [FillMode; 3] = [FillMode::Bottom, FillMode::Top, FillMode::Fade];
    pub fn label(self) -> &'static str {
        match self {
            FillMode::Bottom => "From bottom",
            FillMode::Top => "From top",
            FillMode::Fade => "Fade",
        }
    }
}

/// 統合ソースの配置。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Arrange {
    /// キーボード左・マウス右 (既定)。
    #[default]
    Side,
    /// キーボード上・マウス下。
    Stack,
    /// マウス左・キーボード右。
    Swap,
}

impl Arrange {
    pub const ALL: [Arrange; 3] = [Arrange::Side, Arrange::Stack, Arrange::Swap];
    pub fn label(self) -> &'static str {
        match self {
            Arrange::Side => "Side by side",
            Arrange::Stack => "Stacked",
            Arrange::Swap => "Swap left/right",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OverlayCfg {
    pub theme: Theme,
    /// アクセント色 (RGB)。OBS 側で色を選んだソースはそちらが優先。
    pub accent: [u8; 3],
    /// 解像度の倍率 (1.0 / 1.5 / 2.0)。大きくすると拡大してもにじまない。
    pub scale: f32,
    pub title: bool,
    pub labels: bool,
    /// 押し込み量 (mm / %) の数字を出す。
    pub numbers: bool,
    pub mouse_style: MouseStyle,
    /// 表示するキー (`LayoutKey::id`)。**空 = すべて表示**。選んだキーだけが出る。
    pub show_keys: Vec<u16>,
    /// 表示するマウスボタン (表示名)。空 = すべて表示。
    pub show_buttons: Vec<String>,
    /// 非表示にするキー (`LayoutKey::id`)。旧フィールド — 保存済み設定の互換用に残す。
    pub hide_keys: Vec<u16>,
    /// 非表示にするマウスボタン (表示名)。
    pub hide_buttons: Vec<String>,

    // ---- レイアウト ----
    /// キーの大きさ係数 (0.7–1.4)。scale とは別にキー密度だけ変える。
    #[serde(default = "one")]
    pub key_size: f32,
    /// 角丸係数 (0–2)。1 = 標準の丸み。
    #[serde(default = "one")]
    pub radius: f32,
    /// パネルの余白係数 (0.5–2)。
    #[serde(default = "one")]
    pub pad: f32,
    /// 統合ソースの配置。
    pub arrange: Arrange,

    // ---- 色 ----
    /// 背景パネルの不透明度 % (0–100)。テーマ色に掛ける。
    #[serde(default = "hundred")]
    pub panel_alpha: u8,
    /// 押し込み色 (RGB)。None = アクセントと同じ。
    pub pressed: Option<[u8; 3]>,

    // ---- 表示要素 ----
    /// 押下時の発光強度 % (0–200)。
    #[serde(default = "hundred")]
    pub glow: u8,
    /// 押し込みフィルの方向。
    pub fill_mode: FillMode,
    /// 作動点ライン (キー内の横線)。
    #[serde(default = "yes")]
    pub trigger_line: bool,
    /// キー内の AP/FT/R テキスト。
    #[serde(default = "yes")]
    pub badges: bool,
    /// 数字を mm ではなく % で出す。
    pub num_pct: bool,
    /// タイトル文字 (空 = デバイス名)。
    #[serde(default)]
    pub title_text: String,
    /// キーとパネルの影。
    #[serde(default = "yes")]
    pub shadow: bool,
}

fn one() -> f32 {
    1.0
}
fn hundred() -> u8 {
    100
}
fn yes() -> bool {
    true
}

impl Default for OverlayCfg {
    fn default() -> Self {
        Self {
            theme: Theme::Glass,
            accent: [60, 190, 255],
            scale: 1.0,
            title: true,
            labels: true,
            numbers: false,
            mouse_style: MouseStyle::Both,
            show_keys: vec![],
            show_buttons: vec![],
            hide_keys: vec![],
            hide_buttons: vec![],
            key_size: 1.0,
            radius: 1.0,
            pad: 1.0,
            arrange: Arrange::Side,
            panel_alpha: 100,
            pressed: None,
            glow: 100,
            fill_mode: FillMode::Bottom,
            trigger_line: true,
            badges: true,
            num_pct: false,
            title_text: String::new(),
            shadow: true,
        }
    }
}

pub fn path() -> Option<PathBuf> {
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("DeviceHub").join("obs_overlay.json"))
}

pub fn load() -> OverlayCfg {
    path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save(cfg: &OverlayCfg) -> std::io::Result<()> {
    let Some(p) = path() else { return Ok(()) };
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = p.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(cfg).unwrap_or_default())?;
    std::fs::rename(&tmp, &p)
}

struct Cache {
    cfg: OverlayCfg,
    mtime: Option<SystemTime>,
    checked: Option<Instant>,
    gen: u64,
}

static CACHE: Mutex<Option<Cache>> = Mutex::new(None);

/// 現在の設定と世代番号 (変わったら +1)。ファイルの確認は 1 秒に 1 回まで。
pub fn current() -> (OverlayCfg, u64) {
    let mut g = CACHE.lock().unwrap_or_else(|p| p.into_inner());
    let c = g.get_or_insert_with(|| Cache {
        cfg: OverlayCfg::default(),
        mtime: None,
        checked: None,
        gen: 0,
    });
    if c.checked
        .is_none_or(|t| t.elapsed() > Duration::from_secs(1))
    {
        c.checked = Some(Instant::now());
        let m = path()
            .and_then(|p| std::fs::metadata(p).ok())
            .and_then(|m| m.modified().ok());
        if m != c.mtime || c.gen == 0 {
            c.mtime = m;
            let n = load();
            if n != c.cfg || c.gen == 0 {
                c.cfg = n;
                c.gen += 1;
            }
        }
    }
    (c.cfg.clone(), c.gen)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_file_without_new_fields_loads_with_defaults() {
        // 新フィールドを持たない旧フォーマットが既定値で読める。
        let c: OverlayCfg = serde_json::from_str(r#"{"theme":"Neon","accent":[1,2,3]}"#).unwrap();
        assert_eq!(c.theme, Theme::Neon);
        assert_eq!(c.key_size, 1.0);
        assert_eq!(c.radius, 1.0);
        assert_eq!(c.pad, 1.0);
        assert_eq!(c.arrange, Arrange::Side);
        assert_eq!(c.panel_alpha, 100);
        assert_eq!(c.pressed, None);
        assert_eq!(c.glow, 100);
        assert_eq!(c.fill_mode, FillMode::Bottom);
        assert!(c.trigger_line && c.badges && c.shadow);
        assert!(!c.num_pct);
        assert!(c.title_text.is_empty());
    }

    #[test]
    fn new_fields_roundtrip() {
        let c = OverlayCfg {
            key_size: 0.85,
            radius: 1.6,
            pad: 1.5,
            arrange: Arrange::Stack,
            panel_alpha: 40,
            pressed: Some([255, 150, 60]),
            glow: 180,
            fill_mode: FillMode::Top,
            trigger_line: false,
            badges: false,
            num_pct: true,
            title_text: "TEST".into(),
            shadow: false,
            ..Default::default()
        };
        let j = serde_json::to_string(&c).unwrap();
        let d: OverlayCfg = serde_json::from_str(&j).unwrap();
        assert_eq!(d, c);
    }
}
