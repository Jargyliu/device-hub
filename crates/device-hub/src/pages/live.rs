//! ライブライティング (DirectDrive) — 公式に無い PC 駆動のキー照明。
//!
//! エンジンは `crate::livefx`。ここは (1) 30fps ティッカーから呼ばれる `fx_tick`
//! (タブ/デバイスを離れても・最小化中も動く) と (2) 設定画面。

use crate::app::{Route, Shell, Tab};
use crate::livefx::{focus_preset, FxMode, FOCUS_PRESETS};
use crate::procmon::foreground_app;

/// Caps Lock のトグル状態。
#[cfg(windows)]
fn caps_lock_on() -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_CAPITAL};
    unsafe { GetAsyncKeyState(VK_CAPITAL.0 as i32) & 1 != 0 }
}
#[cfg(not(windows))]
fn caps_lock_on() -> bool {
    false
}
use crate::pages::keyboard::Cap;
use crate::ui::{self, Btn};
use crate::{overlay, theme};
use devhub_core::*;
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

/// 主色・副色・背景のクイックパレット。
const SWATCHES: [u32; 10] = [
    0x4cb4ff, 0x34d6a8, 0xa98bff, 0xff8a5c, 0xff5c8a, 0xf5c24c, 0xffffff, 0xff2a2a, 0x22c55e,
    0x2a5bff,
];

impl Shell {
    // ------------------------------------------------------------ 実行

    pub fn fx_start(&mut self, id: DeviceId, cx: &mut Context<Self>) {
        self.fx = Default::default();
        self.fx_last = Instant::now();
        self.prefs.livefx.device = Some(id.0.clone());
        self.prefs.save();
        self.hub.set_travel(&id, true);
        self.hub.set_dd(&id, true);
        self.fx_on = Some(id);
        cx.notify();
    }

    pub fn fx_stop(&mut self, cx: &mut Context<Self>) {
        if let Some(id) = self.fx_on.take() {
            self.hub.set_dd(&id, false);
            // テスト/トリガー画面で同じデバイスを見ているなら測定は続ける。
            let watching = self.device_id().as_ref() == Some(&id)
                && matches!(self.tab, Tab::Test | Tab::Trigger);
            if !watching {
                self.hub.set_travel(&id, false);
            }
        }
        cx.notify();
    }

    /// 30fps ティッカーから毎回呼ばれる。止まっていれば何もしない。
    pub fn fx_tick(&mut self, cx: &mut Context<Self>) {
        // 起動時の自動開始 (対象デバイスの読み込みが終わったら 1 回だけ)。
        if !self.fx_autostarted && self.fx_on.is_none() && self.prefs.livefx.autostart {
            if let Some(dev) = self.prefs.livefx.device.clone() {
                let id = DeviceId(dev);
                if self
                    .hub
                    .session(&id)
                    .is_some_and(|s| s.caps.is_some() && s.state.is_some())
                {
                    self.fx_autostarted = true;
                    self.fx_start(id, cx);
                }
            }
        }
        let Some(id) = self.fx_on.clone() else {
            return;
        };
        let Some(s) = self.hub.session(&id) else {
            return; // 切断中: 再接続を待つ
        };
        if s.switching_profile || !s.ready() {
            return; // 確認済みの構成を再読するまでフレームを生成しない。
        }
        let (Some(caps), Some(state)) = (s.caps.clone(), s.state.clone()) else {
            return;
        };
        let Some(kc) = caps.keyboard.clone() else {
            return;
        };
        let (travel_on, dd_on) = (s.travel_on, s.dd_on);
        let depths = s.travel.clone();
        let tmax = s.travel_max;
        // DirectDrive が外から止まった (送信失敗など) → ライブも止める。
        if !dd_on {
            if self.fx.armed {
                self.fx_on = None;
                self.hub.set_travel(&id, false);
                self.toast(
                    "DirectDrive stopped, so live lighting was stopped",
                    true,
                    cx,
                );
                return;
            }
            self.hub.set_dd(&id, true);
        } else {
            self.fx.armed = true;
        }
        // 測定が止まった (失敗) → 2 秒おきに再開を試す。
        if !travel_on
            && self
                .fx
                .rearm
                .is_none_or(|t| t.elapsed() > Duration::from_secs(2))
        {
            self.fx.rearm = Some(Instant::now());
            self.hub.set_travel(&id, true);
        }
        let act: Vec<f32> = state
            .keyboard
            .as_ref()
            .map(|k| k.perf.iter().map(|p| p.actuation).collect())
            .unwrap_or_default();
        let nominal = kc.perf.as_ref().map(|p| p.travel).unwrap_or(4.);
        // 実測の底が浅い個体に合わせる (公称より深くは押せない)。
        let full = if tmax > 1.5 {
            tmax.min(nominal)
        } else {
            nominal
        };
        let dt = self.fx_last.elapsed().as_secs_f32().min(0.2);
        self.fx_last = Instant::now();
        let cfg = self.prefs.livefx.clone();
        let env = self.fx_env(&cfg);
        let frame = self.fx.frame(&cfg, &kc, &depths, &act, full, dt, &env);
        let cells: BTreeMap<usize, Rgb> = if cfg.hybrid {
            // ハイブリッド: 無効セル (背景色と同じ) は送らずファームの
            // エフェクトに任せる。離れたセルはドライバが [5,4] で解放する。
            let off = crate::livefx::off_color(&cfg);
            frame
                .iter()
                .copied()
                .enumerate()
                .filter(|(_, c)| *c != off)
                .collect()
        } else {
            frame.iter().copied().enumerate().collect()
        };
        self.hub.dd_frame(&id, cells);
        if self.tab == Tab::Live && self.route == Route::Device(id) {
            cx.notify();
        }
    }

    /// フレーム計算の外部入力 (CapsLock / 電池 / 音 / 前面アプリ)。
    fn fx_env(&self, cfg: &crate::livefx::FxPrefs) -> crate::livefx::FxEnv {
        let caps_lock = caps_lock_on();
        // 他のワイヤレス機を含む最低電池 (このキーボード自身も対象)。
        let battery = self
            .hub
            .devices
            .iter()
            .filter_map(|d| d.battery.and_then(|b| b.percent.map(|pc| (pc, b.charging))))
            .min_by_key(|(pc, _)| *pc);
        let audio = if cfg.mode == FxMode::Audio {
            crate::audio::start();
            crate::audio::bands()
        } else {
            Vec::new()
        };
        let fg_app = foreground_app();
        let app_focus = if cfg.mode == FxMode::AppFocus {
            fg_app
                .as_deref()
                .and_then(|exe| cfg.app_map.iter().find(|(m, _)| exe.contains(m.as_str())))
                .map(|(_, preset)| focus_preset(preset))
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        crate::livefx::FxEnv {
            caps_lock,
            battery,
            audio,
            app_focus,
            fg_app,
        }
    }

    // ------------------------------------------------------------ 画面

    pub fn page_live(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let _ = window;
        let p = theme::pal();
        let Some(id) = self.device_id() else {
            return div();
        };
        let Some(kc) = self
            .cur()
            .and_then(|s| s.caps.as_ref())
            .and_then(|c| c.keyboard.clone())
        else {
            return div().child(ui::faint("Loading keyboard info…"));
        };
        let running_here = self.fx_on.as_ref() == Some(&id);
        let running_other = self.fx_on.is_some() && !running_here;
        let cfg = self.prefs.livefx.clone();

        // ---- ヘッダー: 開始/停止 ----
        let status = if running_here {
            ui::chip("Running", p.ok)
        } else if running_other {
            ui::chip("Running on another device", p.warn)
        } else {
            ui::chip("Stopped", p.faint)
        };
        let id2 = id.clone();
        let toggle_btn = if running_here {
            ui::button(
                "fx-stop",
                "Stop",
                Some("square"),
                Btn::Glass,
                cx,
                |this, _, cx| this.fx_stop(cx),
            )
        } else {
            ui::button(
                "fx-start",
                "Start",
                Some("play"),
                Btn::Primary,
                cx,
                move |this, _, cx| {
                    this.fx_stop(cx);
                    this.fx_start(id2.clone(), cx);
                },
            )
        };
        let trailing = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .child(status)
            .child(toggle_btn);

        // ---- モード (4+3 の2行) ----
        let mut modes = div().flex().flex_col().gap(px(8.));
        for chunk in FxMode::ALL.chunks(4) {
            let mut row = div().flex().gap(px(8.));
            for &m in chunk {
                let on = cfg.mode == m;
                row = row.child(
                    div()
                        .id(ElementId::Name(format!("fx-mode-{m:?}").into()))
                        .flex_1()
                        .min_w_0()
                        .p(px(12.))
                        .rounded(px(12.))
                        .cursor_pointer()
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .bg(if on {
                            p.accent_soft
                        } else {
                            theme::alpha(p.faint, 0.06)
                        })
                        .border_1()
                        .border_color(if on { p.accent } else { p.hair })
                        .hover(|s| s.bg(theme::alpha(p.faint, 0.12)))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.))
                                .child(ui::icon(m.icon(), 16., if on { p.accent } else { p.sub }))
                                .child(
                                    div()
                                        .text_size(px(13.))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(if on { p.fg } else { p.sub })
                                        .child(m.label()),
                                ),
                        )
                        .child(div().text_size(px(11.)).text_color(p.faint).child(m.desc()))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.prefs.livefx.mode = m;
                            this.prefs.save();
                            cx.notify();
                        })),
                );
            }
            modes = modes.child(row);
        }

        // ---- プレビュー (実際に送っている色) ----
        let frame = if running_here {
            self.fx.last.clone()
        } else {
            vec![]
        };
        let depths = self.cur().map(|s| s.travel.clone()).unwrap_or_default();
        let full = kc.perf.as_ref().map(|p| p.travel).unwrap_or(4.).max(0.5);
        let focus_mode = cfg.mode == FxMode::Focus;
        // 強調中のキー列 (Focus=手動、AppFocus=前面アプリから解決)。
        let sel_keys: Vec<String> = match cfg.mode {
            FxMode::AppFocus => foreground_app()
                .as_deref()
                .and_then(|exe| {
                    cfg.app_map
                        .iter()
                        .find(|(m, _)| exe.contains(m.as_str()))
                        .map(|(_, pr)| focus_preset(pr))
                })
                .unwrap_or_else(|| cfg.focus.clone()),
            _ => cfg.focus.clone(),
        };
        let off = cfg.hybrid.then(|| crate::livefx::off_color(&cfg));
        let caps_v: Vec<Cap> = kc
            .layout
            .iter()
            .enumerate()
            .map(|(i, k)| {
                let tint = frame
                    .get(i)
                    .map(|c| {
                        // ハイブリッド: ファームに任せるセルは薄く表示。
                        if Some(*c) == off {
                            theme::alpha(p.faint, 0.10)
                        } else {
                            theme::alpha(theme::rgb_of(*c), 0.9)
                        }
                    })
                    .or_else(|| {
                        // 停止中は設定のイメージ (キー強調のみ意味がある)
                        (focus_mode && sel_keys.iter().any(|l| l.eq_ignore_ascii_case(&k.label)))
                            .then(|| theme::alpha(theme::rgb_of(cfg.a), 0.7))
                    });
                Cap {
                    top: k.label.clone(),
                    tint,
                    depth: running_here
                        .then(|| depths.get(i).map(|d| (d / full).clamp(0., 1.)))
                        .flatten(),
                    selected: focus_mode
                        && sel_keys.iter().any(|l| l.eq_ignore_ascii_case(&k.label)),
                    badge: (cfg.mode == FxMode::Heat && running_here)
                        .then(|| self.fx.heat.get(i).copied().filter(|h| *h > 0))
                        .flatten()
                        .map(|h| h.to_string()),
                    ..Default::default()
                }
            })
            .collect();
        let labels: Vec<String> = kc.layout.iter().map(|k| k.label.clone()).collect();
        let view = self.keyboard_view(
            "fx-preview",
            &kc,
            caps_v,
            cx,
            Rc::new(move |this, i, _, cx| {
                // キー強調モードではクリックで点灯キーを追加/削除。
                if this.prefs.livefx.mode != FxMode::Focus {
                    return;
                }
                let l = labels[i].clone();
                let f = &mut this.prefs.livefx.focus;
                if let Some(pos) = f.iter().position(|x| x.eq_ignore_ascii_case(&l)) {
                    f.remove(pos);
                } else {
                    f.push(l);
                }
                this.prefs.save();
                cx.notify();
            }),
        );
        let hint = match cfg.mode {
            FxMode::Focus => "Click keys to add/remove lit keys",
            FxMode::Status => {
                "Caps Lock lights the Caps key / the number row becomes a gauge when a wireless device's battery drops below 40%"
            }
            FxMode::Audio => "Shows the spectrum of audio playing on the PC on the keyboard (Hybrid recommended)",
            FxMode::AppFocus => "Highlighted keys switch automatically with the foreground app (assign below)",
            _ if running_here && cfg.hybrid => {
                "Dim keys are the keyboard's own effect — only pressed keys are driven by the PC"
            }
            _ if running_here => "The colors being sent to the device right now (press depth shown as bars)",
            _ => "Press \"Start\" and the device's keys start to light up",
        };
        let main = ui::card_titled("Live Lighting", "zap", Some(trailing.into_any_element())).child(
            ui::body()
                .pt(px(8.))
                .gap(px(12.))
                .child(div().text_size(px(12.)).text_color(p.sub).child(
                    "Not in the official app: drives the LEDs directly from the PC while reading press depth (DirectDrive). Keeps running when you switch tabs or minimize, and returns to the keyboard's own lighting when stopped.",
                ))
                .child(modes)
                .child(view)
                .child(ui::faint(hint)),
        );

        // ---- カスタマイズ ----
        let mut body = ui::body();
        body = body.child(ui::row_first(
            if matches!(cfg.mode, FxMode::Depth) {
                "Light press color"
            } else {
                "Primary color"
            },
            Some(match cfg.mode {
                FxMode::Depth => "Color from press start to the actuation point",
                FxMode::Focus | FxMode::AppFocus => "Highlighted key color",
                FxMode::Ripple => "Outer ripple color",
                FxMode::Audio => "Low end of the spectrum",
                FxMode::Status => "Not used (indicators use the secondary color)",
                FxMode::Heat => "Not used in heatmap",
            }),
            self.fx_swatches("fx-a", cfg.a, |c| &mut c.a, window, cx),
        ));
        body = body.child(ui::row(
            if matches!(cfg.mode, FxMode::Depth) {
                "Color after actuation"
            } else {
                "Pressed color"
            },
            Some("Color past the actuation point / at the moment of press"),
            self.fx_swatches("fx-b", cfg.b, |c| &mut c.b, window, cx),
        ));
        body = body.child(ui::row(
            "Use the keyboard's effect as background",
            Some("Hybrid: only pressed/highlighted keys are driven by the PC; other keys keep the firmware's own lighting (verified on hardware)"),
            ui::toggle("fx-hybrid", cfg.hybrid, cx, |this, v, cx| {
                this.prefs.livefx.hybrid = v;
                this.prefs.save();
                cx.notify();
            }),
        ));
        if !cfg.hybrid {
            body = body.child(ui::row(
                "Background color",
                Some("Color of idle keys"),
                self.fx_swatches("fx-base", cfg.base, |c| &mut c.base, window, cx),
            ));
            body = body.child(ui::row(
                "Background brightness",
                None,
                div().child(ui::slider_value(
                    "fx-base-lv",
                    cfg.base_level as f32,
                    0.,
                    100.,
                    1.,
                    "%",
                    cx,
                    |this, v, cx| {
                        this.prefs.livefx.base_level = v as u8;
                        this.prefs.save();
                        cx.notify();
                    },
                )),
            ));
        }
        body = body.child(ui::row(
            "Overall brightness",
            None,
            div().child(ui::slider_value(
                "fx-bright",
                cfg.brightness as f32,
                5.,
                100.,
                1.,
                "%",
                cx,
                |this, v, cx| {
                    this.prefs.livefx.brightness = v as u8;
                    this.prefs.save();
                    cx.notify();
                },
            )),
        ));
        body = body.child(ui::row(
            "Afterglow",
            Some("How long the light takes to fade after release"),
            div().child(ui::slider_value(
                "fx-decay",
                cfg.decay_ms as f32,
                0.,
                2000.,
                10.,
                "ms",
                cx,
                |this, v, cx| {
                    this.prefs.livefx.decay_ms = v as u32;
                    this.prefs.save();
                    cx.notify();
                },
            )),
        ));
        if cfg.mode == FxMode::Focus {
            let names: Vec<String> = FOCUS_PRESETS.iter().map(|s| s.to_string()).collect();
            let cur = FOCUS_PRESETS
                .iter()
                .position(|n| {
                    let mut a = focus_preset(n);
                    let mut b = cfg.focus.clone();
                    a.sort();
                    b.sort();
                    a == b
                })
                .unwrap_or(usize::MAX);
            body = body.child(ui::row(
                "Highlighted keys",
                Some(if cur == usize::MAX {
                    "Custom (click keys in the preview to edit)"
                } else {
                    "Presets"
                }),
                div().w(px(320.)).child(ui::segmented(
                    "fx-focus",
                    &names,
                    cur,
                    cx,
                    |this, i, cx| {
                        this.prefs.livefx.focus = focus_preset(FOCUS_PRESETS[i]);
                        this.prefs.save();
                        cx.notify();
                    },
                )),
            ));
        }
        if cfg.mode == FxMode::Audio {
            body = body.child(ui::row(
                "Sensitivity",
                Some("Raise it if the light is weak or doesn't react"),
                div().child(ui::slider_value(
                    "fx-again",
                    cfg.audio_gain as f32,
                    10.,
                    400.,
                    5.,
                    "%",
                    cx,
                    |this, v, cx| {
                        this.prefs.livefx.audio_gain = v as u8;
                        this.prefs.save();
                        cx.notify();
                    },
                )),
            ));
        }
        if cfg.mode == FxMode::AppFocus {
            // 前面アプリ → プリセットの割り当て。
            let fg = foreground_app();
            let names: Vec<String> = FOCUS_PRESETS.iter().map(|s| s.to_string()).collect();
            let fg_label = fg.clone().unwrap_or_else(|| "(unknown)".into());
            let cur_preset = fg
                .as_deref()
                .and_then(|exe| cfg.app_map.iter().find(|(m, _)| exe.contains(m.as_str())))
                .map(|(_, p)| p.clone());
            let sel = cur_preset
                .as_deref()
                .and_then(|p| FOCUS_PRESETS.iter().position(|n| *n == p))
                .unwrap_or(usize::MAX);
            body = body.child(ui::row(
                format!("Foreground app: {fg_label}").as_str(),
                Some("Choosing a preset registers it as this app's highlighted keys"),
                div().w(px(320.)).child(ui::segmented(
                    "fx-app-sel",
                    &names,
                    sel,
                    cx,
                    move |this, i, cx| {
                        if let Some(exe) = foreground_app() {
                            let preset = FOCUS_PRESETS[i].to_string();
                            let map = &mut this.prefs.livefx.app_map;
                            if let Some(e) = map.iter_mut().find(|(m, _)| exe.contains(m.as_str()))
                            {
                                e.1 = preset;
                            } else {
                                map.push((exe, preset));
                            }
                            this.prefs.save();
                        }
                        cx.notify();
                    },
                )),
            ));
            // 登録済み割り当て (削除可)。
            for (mi, (m, preset)) in cfg.app_map.iter().enumerate() {
                body = body.child(ui::row(
                    format!("…{m}").as_str(),
                    Some(preset.as_str()),
                    ui::button(
                        format!("fx-app-del-{mi}"),
                        "Delete",
                        Some("trash-2"),
                        Btn::Ghost,
                        cx,
                        move |this, _, cx| {
                            this.prefs.livefx.app_map.remove(mi);
                            this.prefs.save();
                            cx.notify();
                        },
                    ),
                ));
            }
        }
        if cfg.mode == FxMode::Heat {
            let top = self.fx.top_keys(&kc, 6);
            let top_txt = if top.is_empty() {
                "No keystrokes yet".to_string()
            } else {
                top.iter()
                    .map(|(k, n)| format!("{k} {n}"))
                    .collect::<Vec<_>>()
                    .join(" · ")
            };
            body = body.child(ui::row(
                format!("{} keystrokes", self.fx.presses).as_str(),
                Some(&top_txt),
                ui::button(
                    "fx-heat-reset",
                    "Reset",
                    Some("rotate-ccw"),
                    Btn::Ghost,
                    cx,
                    |this, _, cx| {
                        this.fx.reset_heat();
                        cx.notify();
                    },
                ),
            ));
        }
        body = body.child(ui::row(
            "Start automatically on launch",
            Some("Starts live lighting automatically when the app launches and finds this keyboard (just open it before gaming)"),
            ui::toggle("fx-auto", cfg.autostart, cx, |this, v, cx| {
                this.prefs.livefx.autostart = v;
                if v && this.prefs.livefx.device.is_none() {
                    this.prefs.livefx.device = this.device_id().map(|d| d.0);
                }
                this.prefs.save();
                cx.notify();
            }),
        ));
        let custom = ui::card_titled("Customize", "palette", None).child(body);

        div()
            .flex()
            .flex_col()
            .gap(px(theme::GAP))
            .child(main)
            .child(custom)
    }

    /// 色のクイック選択 (パレット + 任意色)。
    fn fx_swatches(
        &mut self,
        id: &str,
        cur: Rgb,
        field: fn(&mut crate::livefx::FxPrefs) -> &mut Rgb,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let mut row = div().flex().items_center().gap(px(6.));
        for hex in SWATCHES {
            let c = Rgb::hex(hex);
            row = row.child(
                div()
                    .id(ElementId::Name(format!("{id}-{hex:06x}").into()))
                    .cursor_pointer()
                    .child(ui::swatch(theme::rgb_of(c), 20., c == cur))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        *field(&mut this.prefs.livefx) = c;
                        this.prefs.save();
                        cx.notify();
                    })),
            );
        }
        let custom = !SWATCHES.iter().any(|h| Rgb::hex(*h) == cur);
        let title = "Pick a color".to_string();
        let pal = theme::pal();
        let face = if custom {
            ui::swatch(theme::rgb_of(cur), 20., true)
        } else {
            div()
                .size(px(20.))
                .rounded_full()
                .border_1()
                .border_color(pal.hair)
                .flex()
                .items_center()
                .justify_center()
                .child(ui::icon("plus", 12., pal.sub))
        };
        row.child(
            div()
                .id(ElementId::Name(format!("{id}-custom").into()))
                .cursor_pointer()
                .child(face)
                .on_click(cx.listener(move |this, _, w, cx| {
                    let cur = *field(&mut this.prefs.livefx);
                    overlay::pick_color(this, &title, cur, w, cx, move |this, c, _, cx| {
                        *field(&mut this.prefs.livefx) = c;
                        this.prefs.save();
                        cx.notify();
                    });
                })),
        )
    }
}
