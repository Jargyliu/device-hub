//! デバイスのライブ計測。バックグラウンド 1 本で HID を叩き、
//! 描画側は最新フレームを読むだけ。

use devhub_core::backend::{DeviceDriver, HubBackend};
use devhub_core::model::{AxisSample, ButtonDepth, KeyPerf, LayoutKey};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// 描画側が読むスナップショット。
#[derive(Clone, Default)]
pub struct Live {
    pub kb: Option<KbLive>,
    pub mouse: Option<MouseLive>,
    /// デバイスがまだ見つかっていないときの案内。
    pub status: String,
    /// フレーム番号 (変化検出用)。
    pub seq: u64,
}

#[derive(Clone)]
pub struct KbLive {
    pub name: String,
    pub layout: Vec<LayoutKey>,
    /// フルストローク mm (正規化に使う)。
    pub travel: f32,
    /// キーごとの作動点 mm (layout 順。RT キーは First touch)。
    /// `DeviceDriver::live_actuation` 未対応/読み取り失敗時は空。
    pub actuation: Vec<f32>,
    pub perf: Vec<KeyPerf>,
    /// layout と同じ並び。
    pub samples: Vec<AxisSample>,
}

#[derive(Clone)]
pub struct MouseLive {
    pub name: String,
    /// `live_buttons` と同じ並びの表示名。
    pub names: Vec<String>,
    pub depths: Vec<ButtonDepth>,
    pub fast: Vec<Option<u8>>,
}

/// キーボード側のターゲット: ドライバ / 名前 / レイアウト / 移動量 (mm) / 作動点 (mm、layout 順)。
type KbTarget = (
    Arc<dyn DeviceDriver>,
    String,
    Vec<LayoutKey>,
    f32,
    Vec<f32>,
    Vec<KeyPerf>,
);
/// マウス側のターゲット: ドライバ / 名前 / ボタン名一覧 / fast フラグ。
type MouseTarget = (Arc<dyn DeviceDriver>, String, Vec<String>, Vec<Option<u8>>);

struct Targets {
    kb: Option<KbTarget>,
    mouse: Option<MouseTarget>,
}

fn find_targets(hub: &dyn HubBackend, kb: bool, mouse: bool) -> Targets {
    let mut t = Targets {
        kb: None,
        mouse: None,
    };
    for info in hub.scan().unwrap_or_default() {
        if !info.supported
            || match info.kind {
                devhub_core::DeviceKind::Keyboard => !kb || t.kb.is_some(),
                devhub_core::DeviceKind::Mouse => !mouse || t.mouse.is_some(),
                _ => true,
            }
        {
            continue;
        }
        let Ok(d) = hub.driver(&info.id) else {
            continue;
        };
        let caps = d.caps();
        if let Some(k) = &caps.keyboard {
            if t.kb.is_none() && k.perf.as_ref().is_some_and(|p| p.live_travel) {
                // 作動点は発見時に 1 度だけ読む (失敗しても表示は動くので空扱い)。
                let perf = d.live_key_perf().unwrap_or_default();
                let actuation = if perf.is_empty() {
                    d.live_actuation().unwrap_or_default()
                } else {
                    vec![]
                };
                t.kb = Some((
                    d.clone(),
                    info.name.clone(),
                    k.layout.clone(),
                    k.perf.as_ref().unwrap().travel,
                    actuation,
                    perf,
                ));
            }
        }
        if let Some(m) = &caps.mouse {
            if t.mouse.is_none() && !m.live_buttons.is_empty() {
                let names = m
                    .live_buttons
                    .iter()
                    .map(|&i| {
                        m.buttons
                            .get(i)
                            .map(|b| b.name.clone())
                            .unwrap_or_else(|| format!("B{i}"))
                    })
                    .collect();
                t.mouse = Some((
                    d.clone(),
                    info.name.clone(),
                    names,
                    d.live_button_fast().unwrap_or_default(),
                ));
            }
        }
    }
    t
}

/// 片側の応答がこの回数続けて失敗したらターゲットを捨て、再探索に回す
/// (抜けたデバイスを掴んだままにしない。連続 ~8 失敗 ≒ 数秒以内の無応答)。
const FAIL_DROP: u32 = 8;

fn loop_body(shared: &Arc<Mutex<Live>>, stop: &Arc<AtomicBool>) {
    let Ok(hub) = devhub_hid::connect() else {
        shared.lock().unwrap().status = "Cannot open HID".into();
        return;
    };
    let mut targets = find_targets(hub.as_ref(), true, true);
    let mut retry = Instant::now() - Duration::from_secs(10);
    let mut fails = (0u32, 0u32);
    while !stop.load(Ordering::Relaxed) {
        // 片側でも欠けていれば 2 秒ごとに、欠けている側だけ探し直す (抜き差し対応)。
        if (targets.kb.is_none() || targets.mouse.is_none())
            && retry.elapsed() > Duration::from_secs(2)
        {
            retry = Instant::now();
            let t = find_targets(hub.as_ref(), targets.kb.is_none(), targets.mouse.is_none());
            if targets.kb.is_none() {
                targets.kb = t.kb;
            }
            if targets.mouse.is_none() {
                targets.mouse = t.mouse;
            }
        }
        if targets.kb.is_none() && targets.mouse.is_none() {
            {
                let mut l = shared.lock().unwrap();
                l.kb = None;
                l.mouse = None;
                l.status = "Looking for supported devices…".into();
                l.seq += 1;
            }
            std::thread::sleep(Duration::from_millis(50));
            continue;
        }
        // HID の応答待ちで OBS の描画を止めない。測定結果は最後にまとめて公開する。
        let mut next = Live::default();
        let mut drop_side = (false, false);
        if let Some((d, name, layout, actuation_travel, actuation, perf)) = &targets.kb {
            match d.live_axis() {
                Ok(samples) => {
                    fails.0 = 0;
                    next.kb = Some(KbLive {
                        name: name.clone(),
                        layout: layout.clone(),
                        travel: *actuation_travel,
                        actuation: actuation.clone(),
                        perf: perf.clone(),
                        samples,
                    });
                }
                Err(_) => {
                    fails.0 += 1;
                    drop_side.0 = fails.0 >= FAIL_DROP;
                    next.status = format!("No response from {name}");
                }
            }
        }
        if let Some((d, name, names, fast)) = &targets.mouse {
            match d.live_buttons() {
                Ok(depths) => {
                    fails.1 = 0;
                    next.mouse = Some(MouseLive {
                        name: name.clone(),
                        names: names.clone(),
                        depths,
                        fast: fast.clone(),
                    });
                }
                Err(_) => {
                    fails.1 += 1;
                    drop_side.1 = fails.1 >= FAIL_DROP;
                    next.status = format!("No response from {name}");
                }
            }
        }
        // 連続失敗した側を破棄 → 次の周回で再探索に回る。
        if drop_side.0 {
            targets.kb = None;
            fails.0 = 0;
            retry = Instant::now() - Duration::from_secs(2);
        }
        if drop_side.1 {
            targets.mouse = None;
            fails.1 = 0;
            retry = Instant::now() - Duration::from_secs(2);
        }
        {
            let mut l = shared.lock().unwrap();
            next.seq = l.seq.wrapping_add(1);
            *l = next;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

pub struct Poller {
    pub shared: Arc<Mutex<Live>>,
    stop: Arc<AtomicBool>,
    join: Option<std::thread::JoinHandle<()>>,
}

impl Poller {
    pub fn start() -> Self {
        let shared = Arc::new(Mutex::new(Live {
            status: "Starting…".into(),
            ..Live::default()
        }));
        let stop = Arc::new(AtomicBool::new(false));
        let (s2, st2) = (shared.clone(), stop.clone());
        let join = std::thread::Builder::new()
            .name("devhub-obs-poll".into())
            .spawn(move || loop_body(&s2, &st2))
            .ok();
        Self { shared, stop, join }
    }
}

impl Drop for Poller {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

/// 全ソース共有のポーラー (3 ソース登録しても HID ポーリングは 1 本)。
/// 最後の保持者が落ちたらワーカーを止めて join する — ソースをすべて
/// 削除したり DLL が unload されても HID 読み取りが走り続けない。
struct Pool {
    live: Arc<Mutex<Live>>,
    stop: Arc<AtomicBool>,
    join: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl Pool {
    fn spawn() -> Arc<Self> {
        let live = Arc::new(Mutex::new(Live {
            status: "Starting…".into(),
            ..Live::default()
        }));
        let stop = Arc::new(AtomicBool::new(false));
        let (s2, st2) = (live.clone(), stop.clone());
        let join = std::thread::Builder::new()
            .name("devhub-obs-poll".into())
            .spawn(move || loop_body(&s2, &st2))
            .ok();
        Arc::new(Self {
            live,
            stop,
            join: Mutex::new(join),
        })
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(j) = self.join.lock().unwrap().take() {
            let _ = j.join();
        }
    }
}

static POOL: Mutex<std::sync::Weak<Pool>> = Mutex::new(std::sync::Weak::new());

/// 共有ポーラーの保持ハンドル。強参照がある限りワーカーは生きている。
pub struct LiveRef(Arc<Pool>);

impl LiveRef {
    /// 描画側が読むスナップショット。
    pub fn live(&self) -> &Mutex<Live> {
        &self.0.live
    }
}

/// 共有ポーラーを取得する — 既存プールがあれば参照を増やし、
/// なければ起動する。`LiveRef` をすべて落とすとワーカーは止まる。
pub fn shared() -> LiveRef {
    let mut slot = POOL.lock().unwrap();
    if let Some(p) = slot.upgrade() {
        return LiveRef(p);
    }
    let p = Pool::spawn();
    *slot = Arc::downgrade(&p);
    LiveRef(p)
}

/// DLL unload 時の保険 — 走っているプールがあれば停止フラグだけ立てる
/// (ソースをすべて畳んだ後なら通常は既に止まっている)。
pub fn shutdown() {
    if let Some(p) = POOL.lock().unwrap().upgrade() {
        p.stop.store(true, Ordering::Relaxed);
    }
}
