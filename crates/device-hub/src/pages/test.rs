//! テスト: クリック / ホイール / キー入力 / チャタリングをその場で確かめる (Web 版には無い機能)。
//!
//! すべて OS から届いた入力イベントで測るので、ドライバ無しでも動く。
//! 設定を変えた直後に「本当に効いているか」を確かめるための画面。

use crate::app::Shell;
use crate::pages::keyboard::{kb_parts, Cap};
use crate::ui::{self, Btn};
use crate::{keycode, motion, theme};
use devhub_core::*;
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::rc::Rc;
use std::time::Instant;

/// これより短い間隔の連続クリック/打鍵は「チャタリングの疑い」。
pub const CHATTER_MS: u128 = 25;

#[derive(Clone, Default)]
pub struct BtnStat {
    pub count: u32,
    pub last: Option<Instant>,
    pub last_gap: Option<u128>,
    pub min_gap: Option<u128>,
    pub chatter: u32,
}

impl BtnStat {
    fn hit(&mut self) {
        let now = Instant::now();
        if let Some(l) = self.last {
            let g = (now - l).as_millis();
            self.last_gap = Some(g);
            self.min_gap = Some(self.min_gap.map_or(g, |m| m.min(g)));
            if g < CHATTER_MS {
                self.chatter += 1;
            }
        }
        self.last = Some(now);
        self.count += 1;
    }
}

impl TestState {
    /// キーイベントを 1 件記録 (直近 3 秒に丸める)。
    fn ev_push(&mut self) {
        let now = Instant::now();
        self.ev_ts.push_back(now);
        while self
            .ev_ts
            .front()
            .is_some_and(|t| now.duration_since(*t).as_millis() > 3000)
        {
            self.ev_ts.pop_front();
        }
    }

    /// (直近 3 秒のイベント/秒, 連続イベントの最小間隔 ms)。
    fn ev_rate(&self) -> (usize, Option<f32>) {
        let n = self.ev_ts.len();
        let min = self
            .ev_ts
            .iter()
            .zip(self.ev_ts.iter().skip(1))
            .map(|(a, b)| b.duration_since(*a).as_secs_f32() * 1000.)
            .reduce(f32::min);
        (n / 3, min)
    }
}

#[derive(Default)]
pub struct TestState {
    /// 0 左, 1 右, 2 中, 3 戻る, 4 進む
    pub buttons: [BtnStat; 5],
    pub wheel_up: u32,
    pub wheel_down: u32,
    /// パッド上の移動イベントの時刻 (直近 1 秒)。
    pub moves: VecDeque<Instant>,
    pub move_peak: usize,
    /// キー: 押下中の usage、打鍵数、統計。
    pub down: BTreeSet<u16>,
    pub keys: BTreeMap<u16, BtnStat>,
    pub max_simul: usize,
    pub total: u32,
    pub last_key: Option<u16>,
    /// 修飾の前回状態 (ModifiersChangedEvent 差分検出用)。
    pub mods: Modifiers,
    /// キーイベント (押下・解放・修飾変化) の時刻 — 直近 3 秒。実測イベントレート用。
    pub ev_ts: VecDeque<Instant>,
    /// 磁気ボタンの深さ履歴 (直近 5 秒、(時刻, [L, R]))。
    pub btn_hist: VecDeque<(Instant, [f32; 2])>,
    /// このテスト画面での最大深さ [L, R]。
    pub btn_peak: [f32; 2],
    /// 作動 (pressed の立ち上がり) 回数 [L, R]。
    pub btn_presses: [u32; 2],
    btn_was_pressed: [bool; 2],
}

impl Shell {
    pub fn test_key(&mut self, e: &KeyDownEvent, _down: bool, cx: &mut Context<Self>) {
        if e.is_held {
            return;
        }
        let Some(u) = keycode::usage_of(&e.keystroke.key) else {
            return;
        };
        let t = &mut self.test;
        t.down.insert(u);
        t.keys.entry(u).or_default().hit();
        t.total += 1;
        t.last_key = Some(u);
        t.max_simul = t.max_simul.max(t.down.len());
        t.ev_push();
        cx.notify();
    }

    pub fn test_key_up(&mut self, e: &KeyUpEvent, cx: &mut Context<Self>) {
        if let Some(u) = keycode::usage_of(&e.keystroke.key) {
            self.test.down.remove(&u);
            self.test.ev_push();
            cx.notify();
        }
    }

    /// 修飾キーの押し/離しをテストに記録。OS は修飾単体に KeyDownEvent を出さず
    /// `ModifiersChangedEvent` を使う (左右は区別できないので左系 usage に記録)。
    pub fn test_mods(&mut self, e: &ModifiersChangedEvent, cx: &mut Context<Self>) {
        let prev = self.test.mods;
        let now = e.modifiers;
        let pairs = [
            (prev.control, now.control, 0xE0u16),
            (prev.shift, now.shift, 0xE1),
            (prev.alt, now.alt, 0xE2),
            (prev.platform, now.platform, 0xE3),
        ];
        let t = &mut self.test;
        let mut changed = false;
        for (a, b, u) in pairs {
            if a == b {
                continue;
            }
            changed = true;
            if b {
                t.down.insert(u);
                t.keys.entry(u).or_default().hit();
                t.total += 1;
                t.last_key = Some(u);
            } else {
                t.down.remove(&u);
            }
        }
        t.mods = now;
        if changed {
            t.max_simul = t.max_simul.max(t.down.len());
            t.ev_push();
            cx.notify();
        }
    }

    pub fn page_test(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut col = div().flex().flex_col().gap(px(theme::GAP));
        let (is_mouse, is_kb) = self
            .cur()
            .and_then(|s| s.caps.as_ref())
            .map(|c| (c.mouse.is_some(), c.keyboard.is_some()))
            .unwrap_or((false, false));
        if is_mouse {
            col = col.child(self.mouse_test(cx));
            if let Some(c) = self.rate_card(false, window, cx) {
                col = col.child(c);
            }
        }
        if is_kb {
            col = col.child(self.key_test(window, cx));
            if let Some(c) = self.rate_card(true, window, cx) {
                col = col.child(c);
            }
        }
        col
    }

    /// 磁気ボタンの押下深さを 1 行で (性能タブの磁気スイッチカード等に差し込む)。
    /// `live_buttons` を出しているデバイスでだけ呼ぶこと。
    pub fn btn_depth_row(&mut self) -> Div {
        let p = theme::pal();
        let btns = self.cur().map(|s| s.buttons.clone()).unwrap_or_default();
        motion::keep_alive();
        div()
            .flex()
            .items_center()
            .gap(px(14.))
            .pt(px(10.))
            .pb(px(4.))
            .child(ui::faint("Live"))
            .children(["L", "R"].iter().enumerate().map(|(i, n)| {
                let b = btns.get(i).copied().unwrap_or_default();
                let on = b.pressed.unwrap_or(false);
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(ui::faint((*n).to_string()))
                    .child(
                        div()
                            .relative()
                            .w(px(110.))
                            .h(px(10.))
                            .rounded(px(5.))
                            .bg(p.ctrl)
                            .child(
                                div()
                                    .absolute()
                                    .left_0()
                                    .top_0()
                                    .bottom_0()
                                    .w(relative(b.depth.clamp(0., 1.)))
                                    .rounded(px(5.))
                                    .bg(if on {
                                        p.accent
                                    } else {
                                        theme::alpha(p.sub, 0.55)
                                    }),
                            )
                            .when_some(b.trigger, |d, t| {
                                d.child(
                                    div()
                                        .absolute()
                                        .top(px(-2.))
                                        .bottom(px(-2.))
                                        .w(px(2.))
                                        .left(relative(t.clamp(0., 1.)))
                                        .bg(p.warn),
                                )
                            }),
                    )
                    .child(
                        div()
                            .w(px(34.))
                            .text_size(px(11.))
                            .text_color(if on { p.accent } else { p.sub })
                            .child(format!("{:.0}%", b.depth * 100.)),
                    )
            }))
    }

    fn mouse_test(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = theme::pal();
        // 磁気ボタンの押下深さを持つ機種では深さ計測を自動 ON (離脱で OFF)。
        let live_slots = self
            .cur()
            .and_then(|s| s.caps.as_ref())
            .and_then(|c| c.mouse.as_ref())
            .map(|m| m.live_buttons.clone())
            .unwrap_or_default();
        if !live_slots.is_empty()
            && self.cur().is_some_and(|s| !s.travel_on)
            && self.device_id().is_some()
        {
            let id = self.device_id().unwrap();
            self.hub.set_travel(&id, true);
        }
        // 深さの履歴・ピーク・作動回数を更新 (30Hz 程度で呼ばれる)。
        let now = Instant::now();
        if let Some(s) = self.cur() {
            if let (Some(&l), Some(&r)) = (s.buttons.first(), s.buttons.get(1)) {
                let t = &mut self.test;
                t.btn_hist.push_back((now, [l.depth, r.depth]));
                while t
                    .btn_hist
                    .front()
                    .is_some_and(|(t0, _)| now.duration_since(*t0).as_millis() > 5000)
                {
                    t.btn_hist.pop_front();
                }
                for (i, b) in [l, r].iter().enumerate() {
                    t.btn_peak[i] = t.btn_peak[i].max(b.depth);
                    let pr = b.pressed.unwrap_or(false);
                    if pr && !t.btn_was_pressed[i] {
                        t.btn_presses[i] += 1;
                    }
                    t.btn_was_pressed[i] = pr;
                }
            }
        }
        // 直近 1 秒の移動イベント数。
        let now = Instant::now();
        while self
            .test
            .moves
            .front()
            .is_some_and(|t| now.duration_since(*t).as_millis() > 1000)
        {
            self.test.moves.pop_front();
        }
        let rate = self.test.moves.len();
        if rate > 0 {
            motion::keep_alive();
        }
        let names = ["Left", "Right", "Middle", "Back", "Forward"];
        let mut stats = div().flex().gap(px(10.));
        for (i, n) in names.iter().enumerate() {
            let s = self.test.buttons[i].clone();
            let flash = s.last.is_some_and(|l| l.elapsed().as_millis() < 160);
            if flash {
                motion::keep_alive();
            }
            let warn = s.chatter > 0;
            stats = stats.child(
                div()
                    .flex_1()
                    .p(px(12.))
                    .rounded(px(14.))
                    .border_1()
                    .border_color(if warn {
                        theme::alpha(p.danger, 0.5)
                    } else {
                        p.hair
                    })
                    .bg(if flash { p.accent_soft } else { p.ctrl })
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(ui::faint(n.to_string()))
                    .child(ui::big_number(format!("{}", s.count), 26.))
                    .child(ui::faint(match s.last_gap {
                        Some(g) => format!("Interval {g}ms · shortest {}ms", s.min_gap.unwrap_or(g)),
                        None => "—".into(),
                    }))
                    .when(warn, |d| {
                        d.child(
                            div()
                                .text_size(px(11.))
                                .text_color(p.danger)
                                .child(format!("Possible chatter {} times", s.chatter)),
                        )
                    }),
            );
        }
        let pad = div()
            .id("test-pad")
            .relative()
            .h(px(220.))
            .rounded(px(18.))
            .border_1()
            .border_color(theme::alpha(p.accent, 0.35))
            .bg(theme::alpha(p.accent, 0.06))
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(6.))
            .child(ui::icon("mouse-pointer-click", 26., p.accent))
            .child(ui::sub("Click, scroll and move inside this box"))
            .child(ui::faint(format!(
                "Wheel ↑{} ↓{} · motion events {}/s (max {}) — motion values are approximate because the OS thins them out",
                self.test.wheel_up, self.test.wheel_down, rate, self.test.move_peak
            )))
            .on_any_mouse_down(cx.listener(|this, e: &MouseDownEvent, _, cx| {
                let i = match e.button {
                    MouseButton::Left => 0,
                    MouseButton::Right => 1,
                    MouseButton::Middle => 2,
                    MouseButton::Navigate(NavigationDirection::Back) => 3,
                    MouseButton::Navigate(NavigationDirection::Forward) => 4,
                };
                this.test.buttons[i].hit();
                cx.notify();
            }))
            .on_scroll(cx.listener(|this, e: &ScrollEvent, _, cx| {
                let dy = match e.delta {
                    ScrollDelta::Lines(l) => l.y,
                    ScrollDelta::Pixels(p) => f32::from(p.y),
                };
                if dy > 0. {
                    this.test.wheel_up += 1;
                } else if dy < 0. {
                    this.test.wheel_down += 1;
                }
                cx.stop_propagation();
                cx.notify();
            }))
            .on_mouse_move(cx.listener(|this, _: &MouseMoveEvent, _, cx| {
                this.test.moves.push_back(Instant::now());
                let n = this.test.moves.len();
                this.test.move_peak = this.test.move_peak.max(n);
                cx.notify();
            }));
        let reset = ui::button(
            "test-reset-m",
            "Reset",
            Some("rotate-ccw"),
            Btn::Ghost,
            cx,
            |this, _, cx| {
                this.test.buttons = Default::default();
                this.test.wheel_up = 0;
                this.test.wheel_down = 0;
                this.test.moves.clear();
                this.test.move_peak = 0;
                cx.notify();
            },
        );
        let clicks = ui::card_titled("Click / Wheel Test", "mouse-pointer-click", Some(reset.into_any_element())).child(
            ui::body()
                .gap(px(12.))
                .pt(px(8.))
                .pb(px(16.))
                .child(pad)
                .child(stats)
                .child(ui::faint(format!(
                    "Repeated inputs under {CHATTER_MS}ms are counted as possible chatter. If it keeps rising, increase the debounce in the Performance tab."
                ))),
        );
        let mut col = div().flex().flex_col().gap(px(theme::GAP)).child(clicks);
        if !live_slots.is_empty() {
            col = col.child(self.btn_depth_card(&live_slots));
        }
        col
    }

    /// 実測ポーリングレートのカード — 計測方式 (OS 配信 Raw Input / USB 転送 USBPcap) の
    /// 切替と、方式ごとの値。`keyboard=true` なら OS 配信のみ (キー入力実測は Raw Input 限定)。
    fn rate_card(
        &mut self,
        keyboard: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let (id, st) = {
            let s = self.cur()?;
            (s.id.clone(), s.poll.clone())
        };
        // USB 転送の実測は Windows の USBPcap 経路があるときだけ選べる。
        let usb_ok = devhub_polling::supported();
        let method = if keyboard || !usb_ok {
            0
        } else {
            self.meas_method
        };
        let st = if keyboard || !usb_ok {
            st?
        } else {
            st.unwrap_or_default()
        };
        let hist = st.hist.clone();
        let min_gap = st.min_gap_ms;
        // 背面では Windows 11 が raw input 配信を制限するので値が下がる — 信用不可。
        let bg = !keyboard && !self.last_window_active;
        let p = theme::pal();
        // 大きな数字は「ピーク」(動いている区間の最大レート — 静止時間を含まない)。
        // 平均 (1 秒) は減速・停止も混ざる下限値。
        let big = st.peak_hz.or(st.seg_hz).unwrap_or(st.hz);
        // 折れ線グラフ (250ms バケット × 60 = 15 秒)。最大値に自動スケール。
        let spark = canvas(
            move |_, _, _| hist.clone(),
            move |bounds, hist, window, _| {
                let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
                let max = hist.iter().copied().fold(10f32, f32::max).max(1.);
                let mut pb = PathBuilder::stroke(px(1.5));
                let n = hist.len().max(1) as f32;
                for (i, v) in hist.iter().enumerate() {
                    let x = px(w * i as f32 / (n - 1.).max(1.));
                    let y = px(h * (1. - (v / max).clamp(0., 1.) * 0.92));
                    let pt = bounds.origin + point(x, y);
                    if i == 0 {
                        pb.move_to(pt);
                    } else {
                        pb.line_to(pt);
                    }
                }
                if let Ok(path) = pb.build() {
                    window.paint_path(path, theme::hex(0x3ec7ff, 0.9));
                }
            },
        )
        .h(px(64.))
        .w_full();

        // 「5 秒間計測」— 計測中はカーソルを窓内に固定する (ゲーム式)。
        // 終了は app.rs の tick が poll_trial を見て行う。
        let measuring = self
            .poll_trial
            .as_ref()
            .is_some_and(|(_, tid, ..)| tid == &id);
        let usb_running = self.usb_cap.as_ref().is_some_and(|(cid, _)| cid == &id);
        let usb_busy = self.usb_cap.as_ref().is_some_and(|(cid, _)| cid != &id);
        let meas_btn = if method == 1 {
            if usb_running {
                Some(
                    ui::button(
                        "usb-stop",
                        "Stop",
                        Some("square"),
                        Btn::Ghost,
                        cx,
                        |this, _, cx| {
                            this.usb_stop();
                            cx.notify();
                        },
                    )
                    .into_any_element(),
                )
            } else if usb_busy {
                None
            } else {
                Some(
                    ui::button(
                        "usb-start",
                        "Start measuring",
                        Some("play"),
                        Btn::Ghost,
                        cx,
                        |this, _, cx| {
                            this.usb_start();
                            cx.notify();
                        },
                    )
                    .into_any_element(),
                )
            }
        } else {
            (!keyboard)
                .then(|| {
                    let label = if measuring {
                        "Measuring…"
                    } else {
                        "Measure for 5 seconds"
                    };
                    ui::button(
                        "poll-measure",
                        label,
                        Some("timer"),
                        Btn::Ghost,
                        cx,
                        |this, w, cx| {
                            if this.poll_trial.is_some() {
                                return;
                            }
                            let Some(device) = this.cur() else { return };
                            let start = Instant::now();
                            let end = start + std::time::Duration::from_secs(5);
                            this.poll_trial =
                                Some((end, device.id.clone(), device.info.vid, device.info.pid));
                            crate::rawinput::begin_trial(start, end);
                            this.poll_meas_result = None;
                            crate::cursor::clip_to_window(w);
                            cx.notify();
                        },
                    )
                    .when(measuring, |d| d.opacity(0.5))
                })
                .map(|b| b.into_any_element())
        };

        let mut col = ui::body().pt(px(8.)).pb(px(14.)).gap(px(6.));
        // 計測方式の切替 (USB 転送は Windows の USBPcap 経路があるときだけ)。
        if !keyboard && usb_ok {
            col = col.child(ui::segmented(
                "meas-method",
                &[
                    "OS delivery (Raw Input)".to_string(),
                    "USB transfers (USBPcap)".to_string(),
                ],
                method,
                cx,
                |this, i, _| this.meas_method = i,
            ));
        }
        if method == 1 {
            col = self.usb_rate_body(col, &id, window, cx);
        } else {
            col = col
                .child(
                    div()
                        .flex()
                        .items_baseline()
                        .gap(px(10.))
                        .child(
                            ui::big_number(format!("{big:.0} Hz"), 28.)
                                .when(bg, |d| d.text_color(p.faint)),
                        )
                        .child(ui::faint(if bg {
                            format!(
                                "Background receive {:.0}/s · approximate because Windows batches input",
                                st.hz
                            )
                        } else if big < 1. {
                            if keyboard {
                                "— measured while you press keys".to_string()
                            } else {
                                "— measured while you move the mouse".to_string()
                            }
                        } else if keyboard {
                            "(key events/s — not the configured polling rate)".to_string()
                        } else {
                            let mut s = format!("Peak · avg {:.0}", st.hz);
                            if let Some(seg) = st.seg_hz {
                                s = format!("{s} · now {seg:.0}");
                            }
                            format!("{s} (while moving)")
                        })),
                )
                .child(spark)
                .when_some(st.gap_stats, |d, (med, p95)| {
                    // 中央値ベースの実測 — 個数/秒より取りこぼしに強い。
                    // 8K が効いていれば ~0.125ms (=8000Hz) 付近に出る。
                    let mut line = format!("Median delivery interval {med:.3} ms → ~{:.0} Hz", 1000. / med);
                    if let Some(g) = min_gap {
                        line = format!("{line} · min {g:.2} ms");
                    }
                    if p95 > med * 2. {
                        line = format!("{line} · p95 {p95:.2} ms (high jitter)");
                    }
                    d.child(ui::faint(line))
                })
                .when(st.gap_stats.is_none(), |d| {
                    d.when_some(min_gap, |d, g| {
                        d.child(ui::faint(format!(
                            "{}{:.2} ms → ~{:.0} Hz equivalent",
                            if keyboard {
                                "Min event interval "
                            } else {
                                "Min report interval "
                            },
                            g,
                            1000. / g
                        )))
                    })
                });
            // 移動量ベースの上限診断 — カウント/秒 ≒ この速度・DPI で届く上限。
            // ほとんどが ±1 カウントなら DPI か速度が足りずレポートが間引かれている。
            if !keyboard {
                col = col.when_some(st.counts_per_sec, |d, c| {
                    let mut line = format!("Motion {c:.0}/s → limit at this speed/DPI ≈ {c:.0} Hz");
                    if st.low_frac.is_some_and(|f| f > 0.5) {
                        line = format!(
                            "{line} (about 1 count per report — raise DPI and move faster to go higher)"
                        );
                    }
                    d.child(ui::faint(line))
                });
            }
            // 計測中の案内 / 結果。
            if measuring {
                let left = self
                    .poll_trial
                    .as_ref()
                    .map(|(e, ..)| {
                        e.saturating_duration_since(std::time::Instant::now())
                            .as_secs_f32()
                    })
                    .unwrap_or(0.);
                col = col.child(
                    div()
                        .text_size(px(12.))
                        .text_color(p.accent)
                        .child(format!("Measuring — the cursor is locked inside the window. Draw fast, large circles inside it ({left:.1} s left)")),
                );
            } else if let Some((_, result)) =
                self.poll_meas_result.as_ref().filter(|(rid, _)| rid == &id)
            {
                let mut line = match result.peak_hz {
                    Some(peak) => format!("Result: peak {peak:.0} Hz"),
                    None => "Result: no continuous motion of 100 ms or longer was detected".to_string(),
                };
                if let Some((med, _)) = result.gap_stats {
                    line = format!("{line} · recent median delivery interval {med:.3} ms");
                }
                if let Some(cnt) = result.counts_per_sec {
                    line = format!("{line} · motion {cnt:.0}/s");
                }
                col = col.child(div().text_size(px(12.)).text_color(p.accent).child(line));
            }
            // USB 計測が裏で動いているなら方式切替で戻れる旨を出す。
            if usb_running {
                col = col.child(ui::faint(
                    "USB measurement is running — \"USB transfers\" shows the current value and Stop",
                ));
            }
            col = col.child(ui::faint(if keyboard {
                "Measured keyboard input reports reaching the OS. A report arrives whenever key state changes, so it only counts while typing (idle polling never reaches the OS)."
            } else {
                "Source: OS delivery (Raw Input). A mouse only sends reports when it moves, so \"Peak\" is the closest to the real polling rate. For accuracy, set DPI to 3200 or higher and draw fast, large circles."
            }));
        }
        Some(
            ui::card_titled("Measured Polling", "gauge", meas_btn)
                .child(col)
                .into_any_element(),
        )
    }

    /// USB 転送 (USBPcap) パネル — 対象の手入力、現在値、期間内の最高、エラーの理由。
    fn usb_rate_body(
        &mut self,
        mut col: Div,
        id: &DeviceId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        use devhub_polling::Status;
        let p = theme::pal();
        if self.usb_cap.as_ref().is_some_and(|(cid, _)| cid == id) {
            // 計測中: 現在値と期間内の最高をライブ表示 (ゲーム等を前面にしてよい)。
            let (target, snap) = self
                .usb_cap
                .as_ref()
                .map(|(_, c)| (c.target, self.usb_snap.unwrap_or_else(|| c.snapshot())))
                .unwrap();
            let (big, note) = match snap.status {
                Status::Waiting => (
                    "Waiting".to_string(),
                    "No target transfers received yet — use the target device".to_string(),
                ),
                Status::Live => (
                    format!("{} Hz", snap.hz),
                    if snap.hz == 0 {
                        "No transfers in the last second (idle)".to_string()
                    } else {
                        "USB transfers in the last second".to_string()
                    },
                ),
                st => (usb_status_reason(st).0, String::new()),
            };
            return col
                .child(
                    div()
                        .flex()
                        .items_baseline()
                        .gap(px(10.))
                        .child(ui::big_number(big, 28.))
                        .when(!note.is_empty(), |d| d.child(ui::faint(note))),
                )
                .child(ui::faint(format!(
                    "Target: {} · highest in period {} Hz (1 s continuous)",
                    target_spec(&target),
                    snap.peak_hz
                )))
                .child(ui::faint(
                    "Source: USB transfers (USBPcap). Keeps running when minimized — bring a game or other app to the front and move (stops when you quit).",
                ));
        }
        if self.usb_cap.as_ref().is_some_and(|(cid, _)| cid != id) {
            let name = self
                .usb_cap
                .as_ref()
                .and_then(|(cid, _)| self.hub.session(cid))
                .map(|s| s.info.name.clone())
                .unwrap_or_else(|| "Other device".into());
            return col.child(ui::faint(format!(
                "USB measurement running on {name} — stop it from that device's page"
            )));
        }
        // 対象デバイスの選択 — 製品名・接続場所から選ぶ。列挙は権限不要で、
        // 初回表示で読み込む。ハブ番号・USB アドレスは選択結果から自動で入る
        // (抜き差し・ポート変更後は「更新」で再解決)。詳細設定の手入力も下に残す。
        if self.usb_devs.is_none() {
            self.usb_refresh();
        }
        let t = self.usb_target;
        if let Some(devs) = self.usb_devs.as_ref().filter(|v| !v.is_empty()) {
            let mut order: Vec<usize> = (0..devs.len()).collect();
            order.sort_by_key(|&i| !devs[i].is_mouse);
            let pick = self.prefs.usb_pick.clone();
            // 通常は HID 入力 (マウス系コレクション) のあるものだけ見せる。
            // それ以外・ハブと照合できなかったものは「その他」に畳む — 内側に
            // 縦スクロールを作るとページの縦スクロールと競合するため畳み式にする。
            let hidden = order
                .iter()
                .filter(|&&i| {
                    !devs[i].is_mouse
                        && !devs[i].instance_id.eq_ignore_ascii_case(&pick)
                })
                .count();
            let show_all = self.usb_show_all;
            let mut list = div().id("usb-dev-list").flex().flex_col().gap(px(3.));
            for i in order {
                let d = &devs[i];
                if !show_all && !d.is_mouse && !d.instance_id.eq_ignore_ascii_case(&pick) {
                    continue;
                }
                let sel = !pick.is_empty() && d.instance_id.eq_ignore_ascii_case(&pick);
                let mut row = div()
                    .id(format!("usbdev-{i}"))
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .px(px(10.))
                    .py(px(5.))
                    .rounded(px(theme::R_CTRL))
                    .border_1()
                    .border_color(if sel {
                        p.accent
                    } else {
                        theme::alpha(p.fg, 0.08)
                    })
                    .when(sel, |d| d.bg(theme::alpha(p.accent, 0.08)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(div().text_size(px(12.)).child(d.name.clone()))
                            .child(ui::faint(format!(
                                "{:04x}:{:04x} · {}",
                                d.vid, d.pid, d.location
                            ))),
                    )
                    .child(ui::faint(if d.hub == 0 {
                        "Hub unknown — enter manually".to_string()
                    } else {
                        format!("USBPcap{} addr {}", d.hub, d.address)
                    }));
                if d.hub != 0 {
                    row = row.cursor_pointer().when(!sel, |d| {
                        d.hover(|s| s.bg(p.ctrl))
                    });
                } else {
                    row = row.opacity(0.55);
                }
                list = list.child(row.on_click(cx.listener(move |this, _, _, cx| {
                    this.usb_pick_device(i);
                    cx.notify();
                })));
            }
            if hidden > 0 {
                let toggle = if show_all {
                    format!("Hide {hidden} other devices")
                } else {
                    format!("Show {hidden} other devices")
                };
                list = list.child(
                    div()
                        .id("usb-dev-more")
                        .cursor_pointer()
                        .px(px(10.))
                        .py(px(4.))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.usb_show_all = !this.usb_show_all;
                            cx.notify();
                        }))
                        .child(ui::faint(toggle)),
                );
            }
            col = col
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(ui::faint("Target devices".to_string()))
                        .child(ui::button(
                            "usb-dev-refresh",
                            "Update",
                            Some("refresh-cw"),
                            Btn::Ghost,
                            cx,
                            |this, _, cx| {
                                this.usb_refresh();
                                cx.notify();
                            },
                        )),
                )
                .child(list);
        }
        // EP 自動検出 — 対象の interrupt IN 転送を 3 秒数えて最も多い EP を採用。
        // ハブを読み書きで開くので権限が要る (計測と同じ)。
        let scanning = self.usb_scan.is_some();
        col = col
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(ui::button(
                        "usb-ep-scan",
                        if scanning { "Detecting…" } else { "Auto-detect EP" },
                        Some("search"),
                        Btn::Ghost,
                        cx,
                        |this, _, cx| {
                            this.usb_detect_ep();
                            cx.notify();
                        },
                    ))
                    .child(ui::faint(if scanning {
                        "Use the target device (3 seconds)".to_string()
                    } else {
                        "Picks the EP with the most transfers".to_string()
                    })),
            )
            .when_some(self.usb_scan_msg.as_ref(), |d, m| {
                d.child(ui::faint(m.clone()))
            });
        let hub_in = self.num_field(
            "usb-hub",
            t.hub as f32,
            1.,
            255.,
            1.,
            "",
            window,
            cx,
            Rc::new(|this, v, _| {
                this.prefs.usb_pick.clear();
                this.usb_target.hub = v as u16;
                this.prefs.usb_hub = this.usb_target.hub;
                this.prefs.save();
            }),
        );
        let addr_in = self.num_field(
            "usb-addr",
            t.device as f32,
            1.,
            127.,
            1.,
            "",
            window,
            cx,
            Rc::new(|this, v, _| {
                this.prefs.usb_pick.clear();
                this.usb_target.device = v as u8;
                this.prefs.save();
            }),
        );
        let ep_in = self.num_field(
            "usb-ep",
            (t.endpoint & 0x7f) as f32,
            1.,
            15.,
            1.,
            "",
            window,
            cx,
            Rc::new(|this, v, _| {
                this.usb_target.endpoint = 0x80 | v as u8;
                this.prefs.usb_ep = v as u8;
                this.prefs.save();
            }),
        );
        let rid_in = self.num_field(
            "usb-rid",
            t.report.map_or(-1., |r| r as f32),
            -1.,
            255.,
            1.,
            "",
            window,
            cx,
            Rc::new(|this, v, _| {
                this.usb_target.report = if v < 0. { None } else { Some(v as u8) };
                this.prefs.usb_report = v as i16;
                this.prefs.save();
            }),
        );
        let field = |label: &str, e: &Entity<gpui_kit::component::input::InputState>| {
            div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .child(ui::faint(label.to_string()))
                .child(
                    div()
                        .w(px(72.))
                        .child(gpui_kit::component::input::Input::new(e)),
                )
        };
        col = col
            .child(
                div()
                    .flex()
                    .gap(px(14.))
                    .child(field("Hub number", &hub_in))
                    .child(field("USB address", &addr_in))
                    .child(field("IN EP number", &ep_in))
                    .child(field("Report ID", &rid_in)),
            )
            .child(ui::faint(
                "Find these values in USBPcap or Wireshark capture settings. EP is the number only (0x81 → 1); Report ID -1 means all. The address changes when replugged.",
            ));
        // 直近の結果 (開始したデバイスに紐付く — 選択を変えても別個体には移らない)。
        if let Some((_, target, snap)) = self.usb_result.as_ref().filter(|(rid, ..)| rid == id) {
            if snap.status.is_error() {
                let (reason, hint) = usb_status_reason(snap.status);
                col = col
                    .child(div().text_size(px(12.)).text_color(p.danger).child(reason))
                    .when(!hint.is_empty(), |d| d.child(ui::faint(hint.to_string())));
            } else {
                col = col.child(
                    div()
                        .text_size(px(12.))
                        .text_color(p.accent)
                        .child(format!("Result: max {} Hz (1 s continuous)", snap.peak_hz)),
                );
            }
            let name = self
                .hub
                .session(id)
                .map(|s| s.info.name.clone())
                .unwrap_or_default();
            col = col.child(ui::faint(format!(
                "Target: {name} — {} · Source: USB transfers (USBPcap)",
                target_spec(target)
            )));
        }
        col.child(ui::faint(
            "Source: USB transfers (USBPcap) — counts device→PC interrupt IN completions over the last second. Not limited by OS delivery, so you can measure with a game in the foreground.",
        ))
    }

    /// 「ボタンの押下深さ」カード — 左右の縦バー + 統計 + 直近 5 秒の折れ線。
    fn btn_depth_card(&mut self, live_slots: &[usize]) -> AnyElement {
        let p = theme::pal();
        let names: Vec<String> = live_slots
            .iter()
            .map(|&i| {
                self.cur()
                    .and_then(|s| s.caps.as_ref())
                    .and_then(|c| c.mouse.as_ref())
                    .and_then(|m| m.buttons.get(i))
                    .map(|b| b.name.clone())
                    .unwrap_or_else(|| format!("Button {i}"))
            })
            .collect();
        let btns = self.cur().map(|s| s.buttons.clone()).unwrap_or_default();
        // 縦バー (0-100%)。作動中はアクセント色、未作動は灰。作動点に線。
        let bar = |i: usize, name: &str| {
            let b = btns.get(i).copied().unwrap_or_default();
            let on = b.pressed.unwrap_or(false);
            let h = 120.;
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(6.))
                .child(ui::faint(name.to_string()))
                .child(
                    div()
                        .relative()
                        .w(px(30.))
                        .h(px(h))
                        .rounded(px(9.))
                        .bg(p.ctrl)
                        .child(
                            div()
                                .absolute()
                                .left_0()
                                .right_0()
                                .bottom_0()
                                .h(relative(b.depth.clamp(0., 1.)))
                                .rounded_b(px(9.))
                                .bg(if on {
                                    p.accent
                                } else {
                                    theme::alpha(p.sub, 0.55)
                                }),
                        )
                        .when_some(b.trigger, |d, t| {
                            d.child(
                                div()
                                    .absolute()
                                    .left_0()
                                    .right_0()
                                    .h(px(2.))
                                    .bottom(px(t.clamp(0., 1.) * h - 1.))
                                    .bg(p.warn),
                            )
                        }),
                )
                .child(ui::big_number(format!("{:.0}", b.depth * 100.), 20.))
                .child(ui::faint(format!(
                    "Max {:.0}% · act {}",
                    self.test.btn_peak.get(i).copied().unwrap_or(0.) * 100.,
                    self.test.btn_presses.get(i).copied().unwrap_or(0)
                )))
        };
        // 直近 5 秒の折れ線 (左右 2 色)。
        let hist: Vec<(Instant, [f32; 2])> = self.test.btn_hist.iter().cloned().collect();
        let spark = canvas(
            move |_, _, _| hist.clone(),
            move |bounds, hist, window, _| {
                let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
                let t_end = Instant::now();
                for (s, color) in [
                    (0usize, theme::hex(0x3ec7ff, 0.9)),
                    (1, theme::hex(0xff7a3d, 0.9)),
                ] {
                    let mut pb = PathBuilder::stroke(px(1.5));
                    let mut first = true;
                    for (t, v) in &hist {
                        let age = t_end.duration_since(*t).as_secs_f32().min(5.);
                        let x = px(w * (1. - age / 5.));
                        let y = px(h * (1. - v[s].clamp(0., 1.)));
                        let pt = bounds.origin + point(x, y);
                        if first {
                            pb.move_to(pt);
                            first = false;
                        } else {
                            pb.line_to(pt);
                        }
                    }
                    if let Ok(path) = pb.build() {
                        window.paint_path(path, color);
                    }
                }
            },
        )
        .h(px(80.))
        .w_full();
        ui::card_titled("Button Press Depth", "activity", None)
            .child(
                ui::body().gap(px(12.)).pt(px(10.)).pb(px(16.)).child(
                    div()
                        .flex()
                        .gap(px(24.))
                        .child(
                            div()
                                .flex()
                                .gap(px(28.))
                                .children(names.iter().enumerate().map(|(i, n)| bar(i, n))),
                        )
                        .child(
                            div().flex_1().child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(4.))
                                    .child(ui::faint("Last 5 s"))
                                    .child(spark)
                                    .child(ui::faint("Lines: L=cyan R=orange · orange horizontal line = actuation point")),
                            ),
                        ),
                ),
            )
            .into_any_element()
    }

    fn key_test(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = theme::pal();
        let Some((kc, ks)) = kb_parts(self) else {
            return div().into_any_element();
        };
        // live_travel 対応なら深さ計測を自動で ON (タブ移動・デバイス移動で OFF される)。
        let can_live = kc.perf.as_ref().is_some_and(|pc| pc.live_travel);
        if can_live && self.cur().is_some_and(|s| !s.travel_on) && self.device_id().is_some() {
            self.hub.set_travel(&self.device_id().unwrap(), true);
        }
        let live = self
            .cur()
            .map(|s| (s.travel.clone(), s.axis.clone(), s.travel_max))
            .unwrap_or_default();
        // 実測最大深さで正規化 (公称 4.0mm より実際の底は浅いため)。
        let live_max = live.2.max(0.5);
        // usage → レイアウト index (メインレイヤーの割り当てから引く)。
        let index_of = |u: u16| -> Option<usize> {
            ks.keymap
                .first()
                .and_then(|l| l.iter().position(|a| *a == KeyAction::Code(u as u32)))
        };
        let caps: Vec<Cap> = kc
            .layout
            .iter()
            .enumerate()
            .map(|(i, k)| {
                let usage = ks
                    .keymap
                    .first()
                    .and_then(|l| l.get(i))
                    .and_then(|a| match a {
                        KeyAction::Code(c) if *c < 0x100 => Some(*c as u16),
                        _ => None,
                    });
                let down = usage.is_some_and(|u| self.test.down.contains(&u))
                    || live.1.get(i).is_some_and(|a| a.status != 0);
                let st = usage.and_then(|u| self.test.keys.get(&u));
                let tint = if down {
                    Some(theme::alpha(p.accent, 0.75))
                } else if st.is_some_and(|s| s.chatter > 0) {
                    Some(theme::alpha(p.danger, 0.45))
                } else if st.is_some() {
                    Some(theme::alpha(p.ok, 0.25))
                } else {
                    None
                };
                Cap {
                    top: k.label.clone(),
                    sub: st.map(|s| format!("{}", s.count)),
                    tint,
                    depth: live.0.get(i).map(|d| d / live_max),
                    ..Default::default()
                }
            })
            .collect();
        let view = self.keyboard_view("keytest", &kc, caps, cx, Rc::new(|_, _, _, _| {}));
        let chatter: u32 = self.test.keys.values().map(|s| s.chatter).sum();
        let unknown: Vec<String> = self
            .test
            .keys
            .keys()
            .filter(|u| index_of(**u).is_none())
            .map(|u| devhub_core::keys::hid_name(*u))
            .collect();
        let focused = self.focus.is_focused(window);
        let reset = ui::button(
            "test-reset-k",
            "Reset",
            Some("rotate-ccw"),
            Btn::Ghost,
            cx,
            |this, w, cx| {
                this.test.down.clear();
                this.test.keys.clear();
                this.test.max_simul = 0;
                this.test.total = 0;
                this.test.last_key = None;
                w.focus(&this.focus, cx);
                cx.notify();
            },
        );
        let stat = |label: &str, v: String| {
            div()
                .flex_1()
                .p(px(12.))
                .rounded(px(14.))
                .bg(p.ctrl)
                .border_1()
                .border_color(p.hair)
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(ui::faint(label.to_string()))
                .child(ui::big_number(v, 24.))
        };
        ui::card_titled("Key Input Test", "keyboard", Some(reset.into_any_element())).child(
            ui::body()
                .gap(px(12.))
                .pt(px(8.))
                .pb(px(16.))
                .when(!focused, |d| {
                    d.child(
                        div()
                            .id("keytest-focus")
                            .p(px(10.))
                            .rounded(px(12.))
                            .cursor_pointer()
                            .bg(theme::alpha(p.warn, 0.12))
                            .border_1()
                            .border_color(theme::alpha(p.warn, 0.35))
                            .child(ui::sub("Click here, then press keys (it can't measure while a text field has focus)"))
                            .on_click(cx.listener(|this, _, w, cx| {
                                w.focus(&this.focus, cx);
                                cx.notify();
                            })),
                    )
                })
                .child(view)
                .child(
                    div()
                        .flex()
                        .gap(px(10.))
                        .child(stat("Total keystrokes", format!("{}", self.test.total)))
                        .child(stat("Simultaneous (max)", format!("{}", self.test.max_simul)))
                        .child(stat("Keys held now", format!("{}", self.test.down.len())))
                        .child(stat("Possible chatter", format!("{chatter}"))),
                )
                .child({
                    let (ev_s, _) = self.test.ev_rate();
                    if !self.test.ev_ts.is_empty() {
                        motion::keep_alive();
                    }
                    div()
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .child(ui::faint(format!("OS events {ev_s}/s")))
                })
                // 押下中キーの生データ (live_axis: 深さ mm / ADC / 状態)。
                .when(!live.1.is_empty(), |d| {
                    let rows: Vec<String> = self
                        .test
                        .down
                        .iter()
                        .filter_map(|u| {
                            let i = index_of(*u)?;
                            let label = kc.layout.get(i)?.label.clone();
                            let a = live.1.get(i)?;
                            Some(format!(
                                "{label}: {:.2}mm · ADC {} · status {} · cal {}",
                                a.route, a.adc, a.status, a.calibrated
                            ))
                        })
                        .collect();
                    let last = self
                        .test
                        .last_key
                        .and_then(|u| index_of(u).map(|i| (u, i)))
                        .and_then(|(_, i)| {
                            let a = live.1.get(i)?;
                            Some(format!(
                                "Last: {} {:.2}mm · ADC {} · status {}",
                                kc.layout.get(i)?.label, a.route, a.adc, a.status
                            ))
                        });
                    d.child(
                        div()
                            .p(px(10.))
                            .rounded(px(12.))
                            .bg(p.ctrl)
                            .border_1()
                            .border_color(p.hair)
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(ui::faint(format!(
                                "Raw data (held key: {}) · max seen {:.2}mm:",
                                if rows.is_empty() { "None" } else { "Yes" },
                                live.2
                            )))
                            .children(rows.into_iter().map(ui::sub))
                            .when_some(last, |d, s| d.child(ui::faint(s))),
                    )
                })
                .child(ui::faint(
                    "Pressed keys turn green, held keys blue, and double inputs under 25ms red. Left/right modifiers can't be told apart (recorded as left). Keys Windows intercepts (Win alone, Alt+Tab, etc.) and Fn never reach the OS, so they can't be measured.",
                ))
                .when(!unknown.is_empty(), |d| d.child(ui::faint(format!("Keys not in the diagram: {}", unknown.join(" "))))),
        )
        .into_any_element()
    }
}

/// USB 計測対象を短く表示 ("USBPcap2 addr 2 · EP 0x81 · ID 3")。
fn target_spec(t: &devhub_polling::Target) -> String {
    let mut s = format!(
        "USBPcap{} addr {} · EP 0x{:02X}",
        t.hub, t.device, t.endpoint
    );
    if let Some(r) = t.report {
        s = format!("{s} · ID {r}");
    }
    s
}

/// エラー状態 → (理由, 代わりに使える方法の案内)。OS コードも残す。
fn usb_status_reason(st: devhub_polling::Status) -> (String, &'static str) {
    use devhub_polling::Status::*;
    let os = |o: Option<i32>| o.map(|c| format!(" (OS error {c})")).unwrap_or_default();
    match st {
        DriverMissing { os: o } => (
            format!(
                "USBPcap not found — driver not installed or wrong hub number{}",
                os(o)
            ),
            "Install USBPcap and reboot, or estimate with the \"OS delivery\" method",
        ),
        AccessDenied { os: o } => (
            format!("Cannot open the hub: insufficient permission{}", os(o)),
            "Restart Device HUB as administrator, or use the \"OS delivery\" method",
        ),
        HubBusy { os: o } => (
            format!("Another process is measuring the same hub{}", os(o)),
            "Stop the OBS USB source or TShark/USBPcapCMD before starting",
        ),
        Disconnected { os: o } => (
            format!("Capture disconnected{}", os(o)),
            "Replugging can change the USB address — check the target and start again",
        ),
        Failed { os: o } => (
            format!("USB capture failed{}", os(o)),
            "Check the target values and start again",
        ),
        _ => (String::new(), ""),
    }
}
