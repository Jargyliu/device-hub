//! デザイントークン (Liquid Glass)。
//!
//! 色はすべてここから取る。明暗・アクセント・ガラスの有無はプロセス全体の
//! 設定 (atomics) で、`pal()` が今の配色を返す。

use gpui_kit::*;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

static LIGHT: AtomicBool = AtomicBool::new(false);
static GLASS: AtomicBool = AtomicBool::new(true);
static MOTION: AtomicBool = AtomicBool::new(true);
static ACCENT: AtomicU8 = AtomicU8::new(0);

pub fn set_light(v: bool) {
    LIGHT.store(v, Ordering::Relaxed);
}
pub fn is_light() -> bool {
    LIGHT.load(Ordering::Relaxed)
}
pub fn set_glass(v: bool) {
    GLASS.store(v, Ordering::Relaxed);
}
pub fn glass_on() -> bool {
    GLASS.load(Ordering::Relaxed)
}
pub fn set_motion(v: bool) {
    MOTION.store(v, Ordering::Relaxed);
}
pub fn motion_on() -> bool {
    MOTION.load(Ordering::Relaxed)
}
pub fn set_accent(i: u8) {
    ACCENT.store(i.min(ACCENTS.len() as u8 - 1), Ordering::Relaxed);
}
pub fn accent_index() -> u8 {
    ACCENT.load(Ordering::Relaxed)
}

/// アクセント候補 (名前, 色)。
pub const ACCENTS: &[(&str, u32)] = &[
    ("Ice", 0x4cb4ff),
    ("Mint", 0x34d6a8),
    ("Lilac", 0xa98bff),
    ("Sunset", 0xff8a5c),
    ("Rose", 0xff5c8a),
    ("Gold", 0xf5c24c),
];

static FONT_SEL: AtomicU8 = AtomicU8::new(0);

/// フォント選択肢: (id, 表示名, 主ファミリ, 日本語フォールバック)。
/// "Geist"/"IBM Plex Sans JP"/"Zen Maru Gothic" は fonts/ に同梱して
/// `install_fonts` で登録する。残りはシステムフォント前提だが、
/// 未インストールでもフォールバックで日本語が豆腐にならない。
pub const FONT_OPTIONS: &[(&str, &str, &str, &[&str])] = &[
    (
        "modern",
        "Modern (Geist + IBM Plex Sans JP)",
        "Geist",
        &["IBM Plex Sans JP", "Yu Gothic UI"],
    ),
    (
        "rounded",
        "Rounded (Geist + Zen Maru Gothic)",
        "Geist",
        &["Zen Maru Gothic", "IBM Plex Sans JP", "Yu Gothic UI"],
    ),
    (
        "plexjp",
        "IBM Plex Sans JP",
        "IBM Plex Sans JP",
        &["Yu Gothic UI"],
    ),
    (
        "zenmaru",
        "Zen Maru Gothic",
        "Zen Maru Gothic",
        &["IBM Plex Sans JP", "Yu Gothic UI"],
    ),
    (
        "mplus",
        "M PLUS Rounded 1c",
        "M PLUS Rounded 1c",
        &["IBM Plex Sans JP", "Yu Gothic UI"],
    ),
    (
        "bizudp",
        "BIZ UDPGothic",
        "BIZ UDPGothic",
        &["IBM Plex Sans JP", "Yu Gothic UI"],
    ),
    ("yugothic", "Yu Gothic UI", "Yu Gothic UI", &[]),
    ("meiryo", "Meiryo UI", "Meiryo UI", &["Yu Gothic UI"]),
];

/// 選択中のフォント (prefs.font の index)。範囲外は 0 (modern)。
pub fn set_font_id(id: &str) {
    let i = FONT_OPTIONS.iter().position(|o| o.0 == id).unwrap_or(0);
    FONT_SEL.store(i as u8, Ordering::Relaxed);
}
pub fn font_index() -> usize {
    (FONT_SEL.load(Ordering::Relaxed) as usize).min(FONT_OPTIONS.len() - 1)
}
pub fn font_id() -> &'static str {
    FONT_OPTIONS[font_index()].0
}

fn opt_font(i: usize) -> Font {
    let (_, _, family, fallbacks) = &FONT_OPTIONS[i.min(FONT_OPTIONS.len() - 1)];
    let mut f = font(*family);
    if !fallbacks.is_empty() {
        f.fallbacks = Some(FontFallbacks::from_fonts(
            fallbacks.iter().map(|s| s.to_string()).collect(),
        ));
    }
    f
}

/// UI 本文フォント (選択中のもの)。
pub fn ui_font() -> Font {
    opt_font(font_index())
}

/// 見出し・大きな数字用。tnum (表形式数字) を有効化する。
pub fn display_font() -> Font {
    let mut f = ui_font();
    f.features = FontFeatures(std::sync::Arc::new(vec![("tnum".into(), 1)]));
    f
}

/// Zen Maru を登録済みか (選択されるまで登録しない)。
static FONTS_ZEN: AtomicBool = AtomicBool::new(false);

/// 同梱フォントをテキストシステムへ登録。
/// Geist + IBM Plex Sans JP は常時 (ほぼ全選択肢の本文/フォールバック)。
/// Zen Maru Gothic (7.6MB) は rounded/zenmaru 選択時だけ登録する —
/// 使わないのに DirectWrite 側にも常駐させないため。
/// フォント選択の変更時にも呼ぶ (`set_font_id` の後)。
pub fn install_fonts(cx: &App) {
    use std::borrow::Cow;
    let mut raw: Vec<&'static [u8]> = Vec::new();
    if !FONTS_GEIST.swap(true, Ordering::Relaxed) {
        raw.extend([
            include_bytes!("../fonts/Geist.ttf").as_slice(),
            include_bytes!("../fonts/IBMPlexSansJP-Regular.ttf").as_slice(),
            include_bytes!("../fonts/IBMPlexSansJP-Bold.ttf").as_slice(),
        ]);
    }
    let need_zen = matches!(font_id(), "rounded" | "zenmaru");
    if need_zen && !FONTS_ZEN.swap(true, Ordering::Relaxed) {
        raw.extend([
            include_bytes!("../fonts/ZenMaruGothic-Regular.ttf").as_slice(),
            include_bytes!("../fonts/ZenMaruGothic-Bold.ttf").as_slice(),
        ]);
    }
    if raw.is_empty() {
        return;
    }
    let fonts: Vec<Cow<'static, [u8]>> = raw.into_iter().map(Cow::Borrowed).collect();
    if let Err(e) = cx.text_system().add_fonts(fonts) {
        eprintln!("[devhub] Failed to register font: {e}");
    }
}

/// Geist/IBM Plex を登録済みか。
static FONTS_GEIST: AtomicBool = AtomicBool::new(false);

/// 等幅 (生データ・コード表示用のシステムフォント)。
pub const FONT_MONO: &str = "Cascadia Mono";

pub const SIDEBAR_W: f32 = 248.;
pub const R_CARD: f32 = 22.;
pub const R_CTRL: f32 = 12.;
pub const GAP: f32 = 14.;
/// ページの左右余白。
pub const PAGE_PAD: f32 = 10.;
/// ページ上部 (タイトルバーの下)。
pub const PAGE_TOP: f32 = 36.;
/// ページ下部の余白。
pub const PAGE_BOT: f32 = 48.;
/// ページ内のセクション間隔。
pub const PAGE_GAP: f32 = 18.;

/// 16 進 RGB → Hsla (alpha 付き)。
pub fn hex(v: u32, a: f32) -> Hsla {
    let mut c: Hsla = rgb(v).into();
    c.a = a;
    c
}

pub fn rgb_of(c: devhub_core::Rgb) -> Hsla {
    rgb(c.to_hex()).into()
}

/// 配色。
#[derive(Clone, Copy)]
pub struct Pal {
    pub light: bool,
    /// 本文。
    pub fg: Hsla,
    /// 補助テキスト。
    pub sub: Hsla,
    /// さらに弱いテキスト (プレースホルダ・単位)。
    pub faint: Hsla,
    pub accent: Hsla,
    pub accent_soft: Hsla,
    /// アクセント上の文字。
    pub on_accent: Hsla,
    pub danger: Hsla,
    pub ok: Hsla,
    pub warn: Hsla,
    /// ガラス面の塗り (上 → 下)。
    pub glass_top: Hsla,
    pub glass_bot: Hsla,
    /// ガラスを切ったときの不透明な面。
    pub solid: Hsla,
    /// 細い縁。
    pub edge: Hsla,
    /// 上端の光。
    pub rim: Hsla,
    /// 操作部の面 (カードの上に載るもの)。
    pub ctrl: Hsla,
    pub ctrl_hover: Hsla,
    /// トラック (スライダー・トグルのオフ)。
    pub track: Hsla,
    /// 区切り線。
    pub hair: Hsla,
    /// モーダルの背後。
    pub scrim: Hsla,
    /// 影。
    pub shadow: Hsla,
    /// キーキャップ。
    pub key: Hsla,
    pub key_edge: Hsla,
}

pub fn pal() -> Pal {
    let light = is_light();
    let acc = ACCENTS[accent_index() as usize].1;
    let accent = hex(acc, 1.);
    if light {
        Pal {
            light,
            fg: hex(0x0d1320, 0.94),
            sub: hex(0x2a3348, 0.66),
            faint: hex(0x2a3348, 0.42),
            accent,
            accent_soft: hex(acc, 0.16),
            on_accent: hex(0xffffff, 1.),
            danger: hex(0xe5484d, 1.),
            ok: hex(0x1fae6d, 1.),
            warn: hex(0xd9901a, 1.),
            glass_top: hex(0xffffff, 0.62),
            glass_bot: hex(0xf4f7fc, 0.44),
            solid: hex(0xf7f9fc, 1.),
            edge: hex(0xffffff, 0.75),
            rim: hex(0xffffff, 0.95),
            ctrl: hex(0xffffff, 0.55),
            ctrl_hover: hex(0xffffff, 0.8),
            track: hex(0x7d8aa5, 0.22),
            hair: hex(0x1b2440, 0.08),
            scrim: hex(0x1b2235, 0.18),
            shadow: hex(0x1b2a4a, 0.16),
            key: hex(0xffffff, 0.7),
            key_edge: hex(0x8090b0, 0.28),
        }
    } else {
        Pal {
            light,
            fg: hex(0xf3f6ff, 0.95),
            sub: hex(0xd8e0f5, 0.62),
            faint: hex(0xd8e0f5, 0.38),
            accent,
            accent_soft: hex(acc, 0.2),
            on_accent: hex(0x061018, 1.),
            danger: hex(0xff6369, 1.),
            ok: hex(0x3fe08f, 1.),
            warn: hex(0xffc04d, 1.),
            glass_top: hex(0x2a3550, 0.42),
            glass_bot: hex(0x121a2c, 0.34),
            solid: hex(0x111725, 1.),
            edge: hex(0xffffff, 0.12),
            rim: hex(0xffffff, 0.34),
            ctrl: hex(0xffffff, 0.07),
            ctrl_hover: hex(0xffffff, 0.12),
            track: hex(0xffffff, 0.12),
            hair: hex(0xffffff, 0.07),
            scrim: hex(0x02040a, 0.45),
            shadow: hex(0x000000, 0.42),
            key: hex(0xffffff, 0.08),
            key_edge: hex(0xffffff, 0.14),
        }
    }
}

/// アクセントを少し明るく/暗くした派生色 (グラデーション用)。
pub fn accent_grad() -> Background {
    let p = pal();
    let mut a2 = p.accent;
    a2.h = (a2.h + 0.06) % 1.;
    a2.l = (a2.l + 0.08).min(0.85);
    linear_gradient(
        135.,
        linear_color_stop(a2, 0.),
        linear_color_stop(p.accent, 1.),
    )
}

/// 2 色の線形補間。
pub fn mix(a: Hsla, b: Hsla, t: f32) -> Hsla {
    let a: Rgba = a.into();
    let b: Rgba = b.into();
    let t = t.clamp(0., 1.);
    Rgba {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: a.a + (b.a - a.a) * t,
    }
    .into()
}

pub fn alpha(c: Hsla, a: f32) -> Hsla {
    Hsla { a, ..c }
}
