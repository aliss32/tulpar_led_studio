#![windows_subsystem = "windows"]
//! Tulpar LED Studio - Tulpar T5 V23.4 tek bolgeli klavye icin efekt programi.
//! InsydeDCHU.dll -> SetDCHU_Data(103, ...) yolunu kullanir (tulpar_led.ps1 ile ayni yontem).
//! Performans sekmesi: cc30wk.exe araciligiyla CPU guc modu, DCHU ile fan/sicaklik okuma.

use eframe::egui;
use egui::{Align2, Color32, FontId, Pos2, Rect, Sense, Stroke};
use std::collections::VecDeque;
use std::f64::consts::PI;
use std::ffi::c_void;
use std::os::windows::process::CommandExt;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

// ============================================================================
//  Win32 FFI (harici crate yok, surum uyumsuzlugu riski dusuk)
// ============================================================================

#[repr(C)]
struct MemoryStatusEx {
    length: u32,
    load: u32,
    total_phys: u64,
    avail_phys: u64,
    total_page: u64,
    avail_page: u64,
    total_virt: u64,
    avail_virt: u64,
    avail_ext: u64,
}
#[repr(C)]
struct PowerStatus {
    ac: u8,
    flag: u8,
    percent: u8,
    sysflag: u8,
    life: u32,
    full: u32,
}
#[repr(C)]
struct SystemTime {
    year: u16,
    month: u16,
    dow: u16,
    day: u16,
    hour: u16,
    min: u16,
    sec: u16,
    ms: u16,
}
#[repr(C)]
struct LastInputInfo {
    size: u32,
    time: u32,
}
#[repr(C)]
struct BitmapInfoHeader {
    size: u32,
    width: i32,
    height: i32,
    planes: u16,
    bitcount: u16,
    compression: u32,
    size_image: u32,
    xppm: i32,
    yppm: i32,
    clr_used: u32,
    clr_important: u32,
}
#[repr(C)]
struct BitmapInfo {
    header: BitmapInfoHeader,
    colors: [u32; 1],
}

#[link(name = "kernel32")]
extern "system" {
    fn LoadLibraryW(name: *const u16) -> *mut c_void;
    fn GetProcAddress(h: *mut c_void, name: *const u8) -> *mut c_void;
    fn GetLastError() -> u32;
    fn GetSystemTimes(idle: *mut u64, kernel: *mut u64, user: *mut u64) -> i32;
    fn GlobalMemoryStatusEx(buf: *mut MemoryStatusEx) -> i32;
    fn GetSystemPowerStatus(buf: *mut PowerStatus) -> i32;
    fn GetTickCount() -> u32;
    fn GetLocalTime(st: *mut SystemTime);
}
#[link(name = "user32")]
extern "system" {
    fn GetAsyncKeyState(vk: i32) -> i16;
    fn GetKeyState(vk: i32) -> i16;
    fn GetLastInputInfo(p: *mut LastInputInfo) -> i32;
    fn GetSystemMetrics(i: i32) -> i32;
    fn GetDC(hwnd: *mut c_void) -> *mut c_void;
    fn ReleaseDC(hwnd: *mut c_void, dc: *mut c_void) -> i32;
}
#[link(name = "gdi32")]
extern "system" {
    fn CreateCompatibleDC(dc: *mut c_void) -> *mut c_void;
    fn CreateCompatibleBitmap(dc: *mut c_void, w: i32, h: i32) -> *mut c_void;
    fn SelectObject(dc: *mut c_void, obj: *mut c_void) -> *mut c_void;
    fn StretchBlt(
        dst: *mut c_void,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        src: *mut c_void,
        sx: i32,
        sy: i32,
        sw: i32,
        sh: i32,
        rop: u32,
    ) -> i32;
    fn SetStretchBltMode(dc: *mut c_void, mode: i32) -> i32;
    fn GetDIBits(
        dc: *mut c_void,
        bmp: *mut c_void,
        start: u32,
        lines: u32,
        bits: *mut c_void,
        info: *mut BitmapInfo,
        usage: u32,
    ) -> i32;
    fn DeleteObject(o: *mut c_void) -> i32;
    fn DeleteDC(dc: *mut c_void) -> i32;
}
#[link(name = "shell32")]
extern "system" {
    fn IsUserAnAdmin() -> i32;
    fn ShellExecuteW(
        hwnd: *mut c_void,
        op: *const u16,
        file: *const u16,
        params: *const u16,
        dir: *const u16,
        show: i32,
    ) -> isize;
}

#[repr(C)]
struct PdhFmtValue {
    status: u32,
    _pad: u32,
    value: f64,
}
#[link(name = "pdh")]
extern "system" {
    fn PdhOpenQueryW(src: *const u16, user: usize, q: *mut isize) -> i32;
    fn PdhAddEnglishCounterW(q: isize, path: *const u16, user: usize, c: *mut isize) -> i32;
    fn PdhCollectQueryData(q: isize) -> i32;
    fn PdhGetFormattedCounterValue(c: isize, fmt: u32, ctype: *mut u32, val: *mut PdhFmtValue) -> i32;
}

/// Gercek CPU frekansi (Gorev Yoneticisi ile ayni yontem):
/// "Processor Frequency" x "% Processor Performance" / 100
struct PdhFreq {
    q: isize,
    perf: isize,
    frq: isize,
    ok: bool,
}
impl PdhFreq {
    fn new() -> PdhFreq {
        unsafe {
            let mut q: isize = 0;
            if PdhOpenQueryW(null(), 0, &mut q) != 0 {
                return PdhFreq { q: 0, perf: 0, frq: 0, ok: false };
            }
            let (mut c1, mut c2): (isize, isize) = (0, 0);
            let p1 = wide("\\Processor Information(_Total)\\% Processor Performance");
            let p2 = wide("\\Processor Information(_Total)\\Processor Frequency");
            let r1 = PdhAddEnglishCounterW(q, p1.as_ptr(), 0, &mut c1);
            let r2 = PdhAddEnglishCounterW(q, p2.as_ptr(), 0, &mut c2);
            let ok = r1 == 0 && r2 == 0;
            if ok {
                PdhCollectQueryData(q);
            }
            PdhFreq { q, perf: c1, frq: c2, ok }
        }
    }
    fn read(&mut self) -> Option<f64> {
        if !self.ok {
            return None;
        }
        unsafe {
            if PdhCollectQueryData(self.q) != 0 {
                return None;
            }
            let mut a = PdhFmtValue { status: 0, _pad: 0, value: 0.0 };
            let mut b = PdhFmtValue { status: 0, _pad: 0, value: 0.0 };
            let mut t = 0u32;
            if PdhGetFormattedCounterValue(self.perf, 0x200, &mut t, &mut a) != 0 {
                return None;
            }
            if PdhGetFormattedCounterValue(self.frq, 0x200, &mut t, &mut b) != 0 {
                return None;
            }
            let mhz = b.value * a.value / 100.0;
            if mhz > 100.0 && mhz < 8000.0 { Some(mhz) } else { None }
        }
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

// ============================================================================
//  Renk yardimcilari (tum degerler f64, RGB 0..255)
// ============================================================================

fn clamp01(x: f64) -> f64 {
    x.max(0.0).min(1.0)
}

fn hsv(h: f64, s: f64, v: f64) -> [f64; 3] {
    let h = h - h.floor();
    let hh = h * 6.0;
    let i = hh.floor() as i32;
    let f = hh - i as f64;
    let p = v * (1.0 - s);
    let q = v * (1.0 - f * s);
    let t = v * (1.0 - (1.0 - f) * s);
    let (r, g, b) = match i % 6 {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    };
    [r * 255.0, g * 255.0, b * 255.0]
}

fn rgb_to_hsv(c: [f64; 3]) -> (f64, f64, f64) {
    let r = c[0] / 255.0;
    let g = c[1] / 255.0;
    let b = c[2] / 255.0;
    let mx = r.max(g).max(b);
    let mn = r.min(g).min(b);
    let d = mx - mn;
    let h = if d == 0.0 {
        0.0
    } else if mx == r {
        (((g - b) / d) % 6.0) / 6.0
    } else if mx == g {
        (((b - r) / d) + 2.0) / 6.0
    } else {
        (((r - g) / d) + 4.0) / 6.0
    };
    let h = if h < 0.0 { h + 1.0 } else { h };
    let s = if mx == 0.0 { 0.0 } else { d / mx };
    (h, s, mx)
}

fn lerp3(a: [f64; 3], b: [f64; 3], t: f64) -> [f64; 3] {
    let t = clamp01(t);
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

fn smoothstep(e0: f64, e1: f64, x: f64) -> f64 {
    let t = clamp01((x - e0) / (e1 - e0));
    t * t * (3.0 - 2.0 * t)
}

fn to_c32(c: [f64; 3]) -> Color32 {
    Color32::from_rgb(
        c[0].max(0.0).min(255.0) as u8,
        c[1].max(0.0).min(255.0) as u8,
        c[2].max(0.0).min(255.0) as u8,
    )
}

fn hash01(k: i64) -> f64 {
    let mut x = (k as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    x ^= x >> 29;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 32;
    (x % 1000) as f64 / 1000.0
}

fn pulse(t: f64, hz: f64) -> f64 {
    0.5 + 0.5 * (2.0 * PI * t * hz).sin()
}

fn f2u(x: f64) -> u32 {
    (x as f32).to_bits()
}
fn u2f(u: u32) -> f64 {
    f32::from_bits(u) as f64
}

// ============================================================================
//  Modlar ve ayarlar
// ============================================================================

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    Static,
    Rainbow,
    Breathe,
    KeyFlash,
    TypingHeat,
    Glow,
    IdleActive,
    System,
    Battery,
    Ping,
    Music,
    Screen,
    Fire,
    Storm,
    Heart,
    Aurora,
    DayTone,
    Pomodoro,
    Caps,
    Combine,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SysSrc {
    Cpu,
    Ram,
    Gpu,
}

impl Mode {
    const ALL: [Mode; 20] = [
        Mode::Combine,
        Mode::Rainbow,
        Mode::Breathe,
        Mode::Static,
        Mode::KeyFlash,
        Mode::TypingHeat,
        Mode::Glow,
        Mode::IdleActive,
        Mode::Screen,
        Mode::Music,
        Mode::System,
        Mode::Battery,
        Mode::Ping,
        Mode::Fire,
        Mode::Storm,
        Mode::Heart,
        Mode::Aurora,
        Mode::DayTone,
        Mode::Pomodoro,
        Mode::Caps,
    ];

    /// Combine modunda arka plan katmani olarak secilebilen modlar.
    const BASES: [Mode; 14] = [
        Mode::Screen,
        Mode::Rainbow,
        Mode::Static,
        Mode::Breathe,
        Mode::Music,
        Mode::System,
        Mode::Battery,
        Mode::Ping,
        Mode::Fire,
        Mode::Storm,
        Mode::Heart,
        Mode::Aurora,
        Mode::DayTone,
        Mode::Pomodoro,
    ];

    fn name(self) -> &'static str {
        match self {
            Mode::Static => "Sabit renk",
            Mode::Rainbow => "Rainbow",
            Mode::Breathe => "Nefes alma",
            Mode::KeyFlash => "Tuş dalgası",
            Mode::TypingHeat => "Yazma ısı haritası",
            Mode::Glow => "Birikimli parıltı",
            Mode::IdleActive => "Boşta / Aktif",
            Mode::System => "Sistem yükü",
            Mode::Battery => "Pil durumu",
            Mode::Ping => "Ağ / Ping",
            Mode::Music => "Müzik",
            Mode::Screen => "Ekran alt şeridi",
            Mode::Fire => "Şömine",
            Mode::Storm => "Fırtına",
            Mode::Heart => "Kalp atışı",
            Mode::Aurora => "Aurora",
            Mode::DayTone => "Gün saati tonu",
            Mode::Pomodoro => "Pomodoro",
            Mode::Caps => "Caps Lock göstergesi",
            Mode::Combine => "Combine (karışık)",
        }
    }

    fn desc(self) -> &'static str {
        match self {
            Mode::Static => "Tek renk, istediğin ton",
            Mode::Rainbow => "Yumuşak sürekli renk geçişi",
            Mode::Breathe => "Yavaşça parlayıp sönen ışık",
            Mode::KeyFlash => "Boşta loş, tuşa basınca patlar",
            Mode::TypingHeat => "Hızlı yazdıkça maviden kırmızıya",
            Mode::Glow => "Yazdıkça birikir, durunca söner",
            Mode::IdleActive => "Boşta nefes, uzun boşta söner",
            Mode::System => "CPU / RAM / GPU sıcaklığı",
            Mode::Battery => "Şarj ve pil seviyesi",
            Mode::Ping => "İnternet kesilince kırmızı",
            Mode::Music => "Sistem sesine göre canlanır",
            Mode::Screen => "Ekranın altındaki renk (video)",
            Mode::Fire => "Titreyen sıcak ateş ışığı",
            Mode::Storm => "Koyu mavi, arada şimşek",
            Mode::Heart => "Çift vuruşlu nabız",
            Mode::Aurora => "Yeşil-mor yavaş akış",
            Mode::DayTone => "Saate göre sıcak/soğuk beyaz",
            Mode::Pomodoro => "Çalışma ve mola zamanlayıcı",
            Mode::Caps => "Caps açıkken kırmızı",
            Mode::Combine => "Arka plan + tuş + caps birlikte",
        }
    }

    fn accent(self) -> Color32 {
        match self {
            Mode::Static => Color32::from_rgb(200, 200, 210),
            Mode::Rainbow => Color32::from_rgb(255, 90, 160),
            Mode::Breathe => Color32::from_rgb(120, 190, 255),
            Mode::KeyFlash => Color32::from_rgb(255, 210, 80),
            Mode::TypingHeat => Color32::from_rgb(255, 120, 60),
            Mode::Glow => Color32::from_rgb(255, 235, 140),
            Mode::IdleActive => Color32::from_rgb(140, 160, 255),
            Mode::System => Color32::from_rgb(90, 220, 150),
            Mode::Battery => Color32::from_rgb(120, 230, 90),
            Mode::Ping => Color32::from_rgb(70, 210, 230),
            Mode::Music => Color32::from_rgb(190, 110, 255),
            Mode::Screen => Color32::from_rgb(255, 150, 200),
            Mode::Fire => Color32::from_rgb(255, 130, 40),
            Mode::Storm => Color32::from_rgb(110, 140, 255),
            Mode::Heart => Color32::from_rgb(255, 70, 90),
            Mode::Aurora => Color32::from_rgb(80, 240, 190),
            Mode::DayTone => Color32::from_rgb(255, 200, 130),
            Mode::Pomodoro => Color32::from_rgb(255, 110, 90),
            Mode::Caps => Color32::from_rgb(255, 80, 80),
            Mode::Combine => Color32::from_rgb(160, 255, 200),
        }
    }

    fn uses_hue(self) -> bool {
        matches!(
            self,
            Mode::Static
                | Mode::Breathe
                | Mode::Glow
                | Mode::IdleActive
                | Mode::Heart
                | Mode::Caps
                | Mode::Music
        )
    }

    fn uses_speed(self) -> bool {
        matches!(
            self,
            Mode::Rainbow
                | Mode::Breathe
                | Mode::KeyFlash
                | Mode::Music
                | Mode::Fire
                | Mode::Storm
                | Mode::Heart
                | Mode::Aurora
        )
    }
}

#[derive(Clone)]
struct Settings {
    mode: Mode,
    on: bool,
    brightness: f64,
    speed: f64,
    base_hue: f64,
    white: bool,
    sys_src: SysSrc,
    idle_breathe_after: f64,
    idle_off_after: f64,
    music_gain: f64,
    screen_strip: f64,
    screen_vivid: f64,
    pomo_work: f64,
    pomo_break: f64,
    pomo_start_nonce: u32,
    pomo_reset_nonce: u32,
    ping_host: String,
    dll_path: String,
    // tus katmani (KeyFlash ve Combine ortak)
    key_idle: f64,
    key_mono: bool,
    key_white: bool,
    key_hue: f64,
    key_hold: f64,
    // combine
    cmb_base: Mode,
    cmb_keys: bool,
    cmb_caps: bool,
    caps_hue: f64,
    caps_strength: f64,
    cmb_bg: bool,
    // ekran seridi: renge gore parlaklik
    screen_adapt: bool,
    screen_min: f64,
    screen_gamma: f64,
}

fn default_dll() -> String {
    let up = std::env::var("USERPROFILE").unwrap_or_else(|_| "C:\\Users\\aliha".to_string());
    let cands = [
        format!("{}\\Desktop\\tstudio\\InsydeDCHU.dll", up),
        format!("{}\\Desktop\\tstudio\\FnKey\\CC40\\InsydeDCHU.dll", up),
        format!("{}\\Desktop\\InsydeDCHU.dll", up),
    ];
    for c in &cands {
        if std::path::Path::new(c).exists() {
            return c.clone();
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let p = dir.join("InsydeDCHU.dll");
            if p.exists() {
                return p.to_string_lossy().to_string();
            }
        }
    }
    format!("{}\\Desktop\\InsydeDCHU.dll", up)
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            mode: Mode::Rainbow,
            on: true,
            brightness: 1.0,
            speed: 1.0,
            base_hue: 0.0,
            white: false,
            sys_src: SysSrc::Cpu,
            idle_breathe_after: 8.0,
            idle_off_after: 120.0,
            music_gain: 4.0,
            screen_strip: 12.0,
            screen_vivid: 1.4,
            pomo_work: 25.0,
            pomo_break: 5.0,
            pomo_start_nonce: 0,
            pomo_reset_nonce: 0,
            ping_host: "1.1.1.1".to_string(),
            dll_path: default_dll(),
            key_idle: 10.0,
            key_mono: false,
            key_white: true,
            key_hue: 0.0,
            key_hold: 1.5,
            cmb_base: Mode::Screen,
            cmb_keys: true,
            cmb_caps: true,
            caps_hue: 0.0,
            caps_strength: 60.0,
            cmb_bg: true,
            screen_adapt: true,
            screen_min: 5.0,
            screen_gamma: 1.6,
        }
    }
}

// ============================================================================
//  Ayarlari kaydetme / yukleme  (%APPDATA%\TulparLedStudio\ayarlar.txt)
// ============================================================================

fn settings_path() -> Option<std::path::PathBuf> {
    let appdata = std::env::var("APPDATA").ok()?;
    Some(
        std::path::PathBuf::from(appdata)
            .join("TulparLedStudio")
            .join("ayarlar.txt"),
    )
}

fn write_settings_text(text: &str) {
    if let Some(p) = settings_path() {
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(p, text);
    }
}

impl Settings {
    fn to_text(&self) -> String {
        let src = match self.sys_src {
            SysSrc::Cpu => 0,
            SysSrc::Ram => 1,
            SysSrc::Gpu => 2,
        };
        let mut t = String::new();
        t.push_str(&format!("mode={:?}\n", self.mode));
        t.push_str(&format!("on={}\n", if self.on { 1 } else { 0 }));
        t.push_str(&format!("brightness={}\n", self.brightness));
        t.push_str(&format!("speed={}\n", self.speed));
        t.push_str(&format!("base_hue={}\n", self.base_hue));
        t.push_str(&format!("white={}\n", if self.white { 1 } else { 0 }));
        t.push_str(&format!("sys_src={}\n", src));
        t.push_str(&format!("idle_breathe_after={}\n", self.idle_breathe_after));
        t.push_str(&format!("idle_off_after={}\n", self.idle_off_after));
        t.push_str(&format!("music_gain={}\n", self.music_gain));
        t.push_str(&format!("screen_strip={}\n", self.screen_strip));
        t.push_str(&format!("screen_vivid={}\n", self.screen_vivid));
        t.push_str(&format!("pomo_work={}\n", self.pomo_work));
        t.push_str(&format!("pomo_break={}\n", self.pomo_break));
        t.push_str(&format!("ping_host={}\n", self.ping_host));
        t.push_str(&format!("dll_path={}\n", self.dll_path));
        t.push_str(&format!("key_idle={}\n", self.key_idle));
        t.push_str(&format!("key_mono={}\n", if self.key_mono { 1 } else { 0 }));
        t.push_str(&format!("key_white={}\n", if self.key_white { 1 } else { 0 }));
        t.push_str(&format!("key_hue={}\n", self.key_hue));
        t.push_str(&format!("key_hold={}\n", self.key_hold));
        t.push_str(&format!("cmb_base={:?}\n", self.cmb_base));
        t.push_str(&format!("cmb_keys={}\n", if self.cmb_keys { 1 } else { 0 }));
        t.push_str(&format!("cmb_caps={}\n", if self.cmb_caps { 1 } else { 0 }));
        t.push_str(&format!("caps_hue={}\n", self.caps_hue));
        t.push_str(&format!("caps_strength={}\n", self.caps_strength));
        t.push_str(&format!("cmb_bg={}\n", if self.cmb_bg { 1 } else { 0 }));
        t.push_str(&format!("screen_adapt={}\n", if self.screen_adapt { 1 } else { 0 }));
        t.push_str(&format!("screen_min={}\n", self.screen_min));
        t.push_str(&format!("screen_gamma={}\n", self.screen_gamma));
        t
    }

    fn save(&self) {
        write_settings_text(&self.to_text());
    }

    fn load() -> Settings {
        let mut st = Settings::default();
        let text = match settings_path().and_then(|p| std::fs::read_to_string(p).ok()) {
            Some(t) => t,
            None => return st,
        };
        for line in text.lines() {
            let mut it = line.splitn(2, '=');
            let k = it.next().unwrap_or("").trim();
            let v = it.next().unwrap_or("").trim();
            let num = v.parse::<f64>().ok();
            match k {
                "mode" => {
                    if let Some(m) = Mode::ALL.iter().find(|m| format!("{:?}", m) == v) {
                        st.mode = *m;
                    }
                }
                "on" => st.on = v == "1",
                "brightness" => {
                    if let Some(x) = num {
                        st.brightness = x.max(0.05).min(1.0);
                    }
                }
                "speed" => {
                    if let Some(x) = num {
                        st.speed = x.max(0.25).min(4.0);
                    }
                }
                "base_hue" => {
                    if let Some(x) = num {
                        st.base_hue = x.max(0.0).min(1.0);
                    }
                }
                "white" => st.white = v == "1",
                "sys_src" => {
                    st.sys_src = match v {
                        "1" => SysSrc::Ram,
                        "2" => SysSrc::Gpu,
                        _ => SysSrc::Cpu,
                    }
                }
                "idle_breathe_after" => {
                    if let Some(x) = num {
                        st.idle_breathe_after = x.max(2.0).min(120.0);
                    }
                }
                "idle_off_after" => {
                    if let Some(x) = num {
                        st.idle_off_after = x.max(10.0).min(1800.0);
                    }
                }
                "music_gain" => {
                    if let Some(x) = num {
                        st.music_gain = x.max(1.0).min(12.0);
                    }
                }
                "screen_strip" => {
                    if let Some(x) = num {
                        st.screen_strip = x.max(4.0).min(40.0);
                    }
                }
                "screen_vivid" => {
                    if let Some(x) = num {
                        st.screen_vivid = x.max(1.0).min(3.0);
                    }
                }
                "pomo_work" => {
                    if let Some(x) = num {
                        st.pomo_work = x.max(1.0).min(90.0);
                    }
                }
                "pomo_break" => {
                    if let Some(x) = num {
                        st.pomo_break = x.max(1.0).min(30.0);
                    }
                }
                "ping_host" => {
                    if !v.is_empty() {
                        st.ping_host = v.to_string();
                    }
                }
                "cmb_bg" => st.cmb_bg = v == "1",
                "screen_adapt" => st.screen_adapt = v == "1",
                "screen_min" => {
                    if let Some(x) = num {
                        st.screen_min = x.max(0.0).min(40.0);
                    }
                }
                "screen_gamma" => {
                    if let Some(x) = num {
                        st.screen_gamma = x.max(1.0).min(3.0);
                    }
                }
                "key_idle" => {
                    if let Some(x) = num {
                        st.key_idle = x.max(0.0).min(50.0);
                    }
                }
                "key_mono" => st.key_mono = v == "1",
                "key_white" => st.key_white = v == "1",
                "key_hue" => {
                    if let Some(x) = num {
                        st.key_hue = x.max(0.0).min(1.0);
                    }
                }
                "key_hold" => {
                    if let Some(x) = num {
                        st.key_hold = x.max(0.3).min(6.0);
                    }
                }
                "cmb_base" => {
                    if let Some(m) = Mode::BASES.iter().find(|m| format!("{:?}", m) == v) {
                        st.cmb_base = *m;
                    }
                }
                "cmb_keys" => st.cmb_keys = v == "1",
                "cmb_caps" => st.cmb_caps = v == "1",
                "caps_hue" => {
                    if let Some(x) = num {
                        st.caps_hue = x.max(0.0).min(1.0);
                    }
                }
                "caps_strength" => {
                    if let Some(x) = num {
                        st.caps_strength = x.max(10.0).min(100.0);
                    }
                }
                "dll_path" => {
                    if std::path::Path::new(v).exists() {
                        st.dll_path = v.to_string();
                    }
                }
                _ => {}
            }
        }
        st
    }
}

// ============================================================================
//  Girdi anlik goruntusu ve saf render fonksiyonu
// ============================================================================

#[derive(Clone, Copy)]
struct Inputs {
    flash: f64,
    flash_vis: f64,
    act: f64,
    flash_hue: f64,
    typing: f64,
    glow: f64,
    idle_secs: f64,
    caps: bool,
    cpu: f64,
    ram: f64,
    gpu_temp: f64,
    battery: f64,
    charging: bool,
    battery_known: bool,
    ping_ms: f64,
    audio: f64,
    bass: f64,
    beat: f64,
    screen: [f64; 3],
    hour: f64,
    pomo_phase: u8,
    pomo_frac: f64,
    pomo_left: f64,
}

const DAY_KEYS: [(f64, [f64; 3], f64); 8] = [
    (0.0, [255.0, 110.0, 25.0], 0.20),
    (6.0, [255.0, 120.0, 35.0], 0.25),
    (9.0, [255.0, 205.0, 150.0], 0.80),
    (12.0, [215.0, 232.0, 255.0], 1.00),
    (16.0, [240.0, 236.0, 255.0], 1.00),
    (19.0, [255.0, 170.0, 90.0], 0.70),
    (22.0, [255.0, 120.0, 30.0], 0.35),
    (24.0, [255.0, 110.0, 25.0], 0.20),
];

fn day_tone(hour: f64) -> [f64; 3] {
    let h = hour.max(0.0).min(24.0);
    for k in 0..DAY_KEYS.len() - 1 {
        let a = DAY_KEYS[k];
        let b = DAY_KEYS[k + 1];
        if h >= a.0 && h <= b.0 {
            let t = if b.0 > a.0 { (h - a.0) / (b.0 - a.0) } else { 0.0 };
            let c = lerp3(a.1, b.1, t);
            let v = a.2 + (b.2 - a.2) * t;
            return [c[0] * v, c[1] * v, c[2] * v];
        }
    }
    [255.0, 120.0, 30.0]
}

/// Tus katmani rengi: bosta dusuk parlaklik, tusa basinca yumusak ama patlayarak yukselir.
fn key_color(s: &Settings, i: &Inputs) -> [f64; 3] {
    let idle = clamp01(s.key_idle / 100.0);
    let v = clamp01(idle + (1.0 - idle) * clamp01(i.flash_vis));
    if s.key_mono {
        hsv(s.key_hue, if s.key_white { 0.0 } else { 1.0 }, v)
    } else {
        hsv(i.flash_hue, 1.0, v)
    }
}

/// Parlaklik haric saf renk uretimi (parlaklik motorda uygulanir). t = saniye.
fn render(s: &Settings, i: &Inputs, t: f64) -> [f64; 3] {
    if s.mode != Mode::Combine {
        return render_mode(s, s.mode, i, t);
    }
    // Combine: arka plan + tus katmani + caps katmani ust uste
    let bg_on = s.cmb_bg;
    let base = if bg_on { render_mode(s, s.cmb_base, i, t) } else { [0.0, 0.0, 0.0] };
    let mut out = base;
    if s.cmb_keys {
        // Arka plan yoksa tus katmani surekli aktif (bosta dusuk, basinca patlar)
        let a = if bg_on { clamp01(i.act) } else { 1.0 };
        out = lerp3(base, key_color(s, i), a);
    }
    if s.cmb_caps && i.caps {
        // Parlakligi koru, sadece rengi caps rengine cevir (tus patlamasi kaybolmasin)
        let (_, _, v) = rgb_to_hsv(out);
        let v = v.max(0.35 * (1.0 - clamp01(i.act)));
        let tint = hsv(s.caps_hue, 1.0, v);
        out = lerp3(out, tint, clamp01(s.caps_strength / 100.0));
    }
    out
}

fn render_mode(s: &Settings, mode: Mode, i: &Inputs, t: f64) -> [f64; 3] {
    let sat = if s.white { 0.0 } else { 1.0 };
    let sp = s.speed;
    match mode {
        Mode::Static => hsv(s.base_hue, sat, 1.0),
        Mode::Rainbow => hsv(t * sp / 10.0, 1.0, 1.0),
        Mode::Breathe => {
            let x = (1.0 - (2.0 * PI * t * sp / 4.0).cos()) / 2.0;
            hsv(s.base_hue, sat, 0.03 + 0.97 * x.powf(1.6))
        }
        Mode::KeyFlash => key_color(s, i),
        Mode::TypingHeat => {
            let ty = clamp01(i.typing);
            hsv(0.62 * (1.0 - ty), 1.0, 0.30 + 0.70 * ty)
        }
        Mode::Glow => hsv(s.base_hue, sat, 0.08 + 0.92 * clamp01(i.glow)),
        Mode::IdleActive => {
            if i.idle_secs < s.idle_breathe_after {
                lerp3(hsv(s.base_hue, sat, 0.5), hsv(i.flash_hue, 1.0, 1.0), i.flash)
            } else if i.idle_secs < s.idle_off_after {
                let x = (1.0 - (2.0 * PI * t / 4.0).cos()) / 2.0;
                hsv(s.base_hue, sat, 0.03 + 0.55 * x.powf(1.6))
            } else {
                [0.0, 0.0, 0.0]
            }
        }
        Mode::System => {
            let v = match s.sys_src {
                SysSrc::Cpu => i.cpu,
                SysSrc::Ram => i.ram,
                SysSrc::Gpu => {
                    if i.gpu_temp < 0.0 {
                        -1.0
                    } else {
                        clamp01((i.gpu_temp - 40.0) / 50.0)
                    }
                }
            };
            if v < 0.0 {
                hsv(0.75, 0.6, 0.25 + 0.2 * pulse(t, 0.4))
            } else {
                let v = clamp01(v);
                hsv(0.62 * (1.0 - v), 1.0, 0.5 + 0.5 * v)
            }
        }
        Mode::Battery => {
            if !i.battery_known {
                hsv(0.0, 0.0, 0.4)
            } else if i.charging {
                let x = (1.0 - (2.0 * PI * t / 3.0).cos()) / 2.0;
                hsv(0.33, 1.0, 0.15 + 0.85 * x)
            } else if i.battery < 0.15 {
                hsv(0.0, 1.0, 0.1 + 0.9 * pulse(t, 1.2))
            } else {
                hsv(0.33 * clamp01((i.battery - 0.15) / 0.85), 1.0, 1.0)
            }
        }
        Mode::Ping => {
            if i.ping_ms < -1.5 {
                hsv(0.0, 0.0, 0.35)
            } else if i.ping_ms < 0.0 {
                hsv(0.0, 1.0, 0.15 + 0.85 * pulse(t, 1.5))
            } else {
                let q = clamp01((i.ping_ms - 30.0) / 220.0);
                hsv(0.33 * (1.0 - q), 1.0, 1.0)
            }
        }
        Mode::Music => {
            let lvl = clamp01(i.audio);
            let v = (0.06 + 0.94 * lvl.powf(0.8)).max(0.1 + 0.9 * i.beat);
            let hue = s.base_hue + t * 0.02 * sp + i.bass * 0.25;
            hsv(hue, 1.0 - 0.35 * i.beat, clamp01(v))
        }
        Mode::Screen => i.screen,
        Mode::Fire => {
            let f = clamp01(
                0.55 + 0.25 * (t * sp * 7.1).sin()
                    + 0.12 * (t * sp * 13.7 + 1.0).sin()
                    + 0.08 * (t * sp * 23.3 + 2.0).sin(),
            );
            hsv(0.02 + 0.06 * f, 1.0, 0.25 + 0.75 * f)
        }
        Mode::Storm => {
            let tau = t * sp / 3.0;
            let k = tau.floor();
            let fr = tau - k;
            let calm = hsv(0.62, 0.9, 0.12 + 0.05 * (t * 0.8).sin());
            if hash01(k as i64) < 0.5 && fr < 0.16 {
                let env = if fr < 0.04 {
                    1.0
                } else if fr < 0.07 {
                    0.15
                } else {
                    0.9 * (1.0 - (fr - 0.07) / 0.09)
                };
                lerp3(calm, hsv(0.6, 0.12, 1.0), env)
            } else {
                calm
            }
        }
        Mode::Heart => {
            let p = (t * sp / 1.1).fract();
            let g = |c: f64, w: f64| (-((p - c) / w).powi(2)).exp();
            let env = g(0.0, 0.06).max(0.65 * g(0.22, 0.06));
            hsv(s.base_hue, sat, 0.06 + 0.94 * env)
        }
        Mode::Aurora => {
            let tt = t * sp;
            let h = 0.45 + 0.16 * (tt * 0.25).sin() + 0.05 * (tt * 0.71 + 1.0).sin();
            let v = (0.55 + 0.30 * (tt * 0.5 + 2.0).sin() + 0.15 * (tt * 1.3).sin())
                .max(0.25)
                .min(1.0);
            hsv(h, 0.85, v)
        }
        Mode::DayTone => day_tone(i.hour),
        Mode::Pomodoro => match i.pomo_phase {
            1 => {
                let f = clamp01(i.pomo_frac);
                hsv(0.33 - 0.27 * smoothstep(0.55, 1.0, f), 1.0, 1.0)
            }
            2 => hsv(0.0, 1.0, 0.3 + 0.7 * pulse(t, 1.5)),
            3 => {
                let x = (1.0 - (2.0 * PI * t / 5.0).cos()) / 2.0;
                hsv(0.58, 0.9, 0.2 + 0.8 * x)
            }
            _ => hsv(0.0, 0.0, 0.12 + 0.10 * pulse(t, 0.25)),
        },
        Mode::Caps => {
            if i.caps {
                hsv(0.0, 1.0, 1.0)
            } else {
                hsv(s.base_hue, sat, 0.35)
            }
        }
        Mode::Combine => [0.0, 0.0, 0.0],
    }
}

/// Onizleme kartlari icin sentetik girdiler.
fn demo_inputs(t: f64, s: &Settings) -> Inputs {
    let cyc = t / 1.4;
    let flash = (-(cyc.fract()) * 5.0).exp();
    let since = cyc.fract() * 1.4;
    let flash_vis = flash * (1.0 - (-since / 0.03).exp());
    let c8 = t % 8.0;
    let act = if c8 < 4.0 { clamp01(c8 / 0.2) } else { clamp01(1.0 - (c8 - 4.0) / 0.7) };
    let osc = |f: f64, ph: f64| 0.5 + 0.5 * (t * f + ph).sin();
    let c = t % 40.0;
    let (ph, fr) = if c < 20.0 {
        (1u8, c / 20.0)
    } else if c < 24.0 {
        (2u8, (c - 20.0) / 4.0)
    } else if c < 34.0 {
        (3u8, (c - 24.0) / 10.0)
    } else {
        (0u8, 0.0)
    };
    let span = (s.idle_off_after + 25.0).max(30.0);
    let audio = ((t * 6.0).sin().abs() * (t * 1.3).sin().abs()).min(1.0);
    Inputs {
        flash,
        flash_vis,
        act,
        flash_hue: (cyc.floor() * 0.06).fract(),
        typing: osc(0.6, 0.0),
        glow: osc(0.4, 1.0),
        idle_secs: (t * 8.0) % span,
        caps: (t % 6.0) < 3.0,
        cpu: osc(0.5, 0.0),
        ram: osc(0.3, 2.0),
        gpu_temp: 40.0 + 50.0 * osc(0.5, 1.0),
        battery: (t * 0.08) % 1.0,
        charging: (t % 24.0) < 6.0,
        battery_known: true,
        ping_ms: if (t % 25.0) > 22.0 { -1.0 } else { 20.0 + 200.0 * osc(0.4, 0.0) },
        audio,
        bass: audio,
        beat: (-((t / 0.5).fract()) * 6.0).exp(),
        screen: hsv((t * 0.05) % 1.0, 0.7, 0.85),
        hour: (t * 1.2) % 24.0,
        pomo_phase: ph,
        pomo_frac: fr,
        pomo_left: 0.0,
    }
}

// ============================================================================
//  Sensorler (paylasilan atomikler)
// ============================================================================

struct Sensors {
    epoch: Instant,
    gpu_temp: AtomicI32,
    gpu_needed: AtomicBool,
    ping_ms: AtomicI32,
    ping_needed: AtomicBool,
    ping_host: Mutex<String>,
    audio_level: AtomicU32,
    audio_bass: AtomicU32,
    audio_last_ms: AtomicU64,
    screen_rgb: AtomicU32,
    screen_on: AtomicBool,
    screen_strip: AtomicU32,
    screen_vivid: AtomicU32,
    screen_adapt: AtomicBool,
    screen_min: AtomicU32,
    screen_gamma: AtomicU32,
}

impl Sensors {
    fn new() -> Sensors {
        Sensors {
            epoch: Instant::now(),
            gpu_temp: AtomicI32::new(-1),
            gpu_needed: AtomicBool::new(false),
            ping_ms: AtomicI32::new(-2),
            ping_needed: AtomicBool::new(false),
            ping_host: Mutex::new("1.1.1.1".to_string()),
            audio_level: AtomicU32::new(0),
            audio_bass: AtomicU32::new(0),
            audio_last_ms: AtomicU64::new(0),
            screen_rgb: AtomicU32::new(0),
            screen_on: AtomicBool::new(false),
            screen_strip: AtomicU32::new(f2u(12.0)),
            screen_vivid: AtomicU32::new(f2u(1.4)),
            screen_adapt: AtomicBool::new(true),
            screen_min: AtomicU32::new(f2u(5.0)),
            screen_gamma: AtomicU32::new(f2u(1.6)),
        }
    }
}

fn run_hidden(cmd: &str, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new(cmd)
        .args(args)
        .creation_flags(0x0800_0000)
        .output()
        .ok()?;
    Some(String::from_utf8_lossy(&out.stdout).to_string())
}

fn parse_ping_ms(out: &str) -> Option<i32> {
    for line in out.lines() {
        if line.contains("TTL=") {
            if let Some(pos) = line.find("ms") {
                let bytes: Vec<char> = line[..pos].chars().collect();
                let mut digits = String::new();
                let mut k = bytes.len();
                while k > 0 && bytes[k - 1].is_ascii_digit() {
                    digits.insert(0, bytes[k - 1]);
                    k -= 1;
                }
                if let Ok(v) = digits.parse::<i32>() {
                    return Some(v);
                }
                return Some(1);
            }
            return Some(1);
        }
    }
    None
}

fn spawn_sensor_threads(sens: Arc<Sensors>) {
    // GPU sicakligi (nvidia-smi)
    let s1 = sens.clone();
    std::thread::spawn(move || loop {
        if s1.gpu_needed.load(Ordering::Relaxed) {
            let out = run_hidden(
                "nvidia-smi",
                &["--query-gpu=temperature.gpu", "--format=csv,noheader,nounits"],
            );
            let v = out
                .and_then(|o| o.lines().next().map(|l| l.trim().to_string()))
                .and_then(|l| l.parse::<i32>().ok())
                .unwrap_or(-1);
            s1.gpu_temp.store(v, Ordering::Relaxed);
            std::thread::sleep(Duration::from_millis(2000));
        } else {
            std::thread::sleep(Duration::from_millis(500));
        }
    });

    // Ping
    let s2 = sens.clone();
    std::thread::spawn(move || loop {
        if s2.ping_needed.load(Ordering::Relaxed) {
            let host = s2.ping_host.lock().unwrap().clone();
            let ok_host = !host.is_empty()
                && host
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == ':');
            if ok_host {
                let out = run_hidden("ping", &["-n", "1", "-w", "1000", &host]);
                let v = out.and_then(|o| parse_ping_ms(&o)).unwrap_or(-1);
                s2.ping_ms.store(v, Ordering::Relaxed);
            } else {
                s2.ping_ms.store(-1, Ordering::Relaxed);
            }
            std::thread::sleep(Duration::from_millis(2500));
        } else {
            std::thread::sleep(Duration::from_millis(500));
        }
    });

    // Ekran alt seridi
    let s3 = sens;
    std::thread::spawn(move || loop {
        if !s3.screen_on.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(250));
            continue;
        }
        let strip = u2f(s3.screen_strip.load(Ordering::Relaxed));
        let vivid = u2f(s3.screen_vivid.load(Ordering::Relaxed));
        let adapt = s3.screen_adapt.load(Ordering::Relaxed);
        let vmin = u2f(s3.screen_min.load(Ordering::Relaxed)) / 100.0;
        let gamma = u2f(s3.screen_gamma.load(Ordering::Relaxed));
        if let Some(c) = capture_strip(strip, vivid, adapt, vmin, gamma) {
            let r = c[0].max(0.0).min(255.0) as u32;
            let g = c[1].max(0.0).min(255.0) as u32;
            let b = c[2].max(0.0).min(255.0) as u32;
            s3.screen_rgb.store((r << 16) | (g << 8) | b, Ordering::Relaxed);
        }
        std::thread::sleep(Duration::from_millis(40));
    });
}

/// Ekranin alt %strip_pct kismini kucultup renk ortalamasi alir.
fn capture_strip(strip_pct: f64, vivid: f64, adapt: bool, vmin: f64, gamma: f64) -> Option<[f64; 3]> {
    const W: i32 = 64;
    const H: i32 = 8;
    unsafe {
        let sw = GetSystemMetrics(0);
        let sh = GetSystemMetrics(1);
        if sw <= 0 || sh <= 0 {
            return None;
        }
        let strip_h = (((sh as f64) * strip_pct / 100.0).max(8.0) as i32).min(sh);
        let sy = sh - strip_h;
        let screen = GetDC(null_mut());
        if screen.is_null() {
            return None;
        }
        let mem = CreateCompatibleDC(screen);
        let bmp = CreateCompatibleBitmap(screen, W, H);
        if mem.is_null() || bmp.is_null() {
            if !bmp.is_null() {
                DeleteObject(bmp);
            }
            if !mem.is_null() {
                DeleteDC(mem);
            }
            ReleaseDC(null_mut(), screen);
            return None;
        }
        let old = SelectObject(mem, bmp);
        SetStretchBltMode(mem, 4); // HALFTONE
        let ok = StretchBlt(mem, 0, 0, W, H, screen, 0, sy, sw, strip_h, 0x40CC_0020);
        SelectObject(mem, old);
        let mut bi = BitmapInfo {
            header: BitmapInfoHeader {
                size: std::mem::size_of::<BitmapInfoHeader>() as u32,
                width: W,
                height: -H,
                planes: 1,
                bitcount: 32,
                compression: 0,
                size_image: 0,
                xppm: 0,
                yppm: 0,
                clr_used: 0,
                clr_important: 0,
            },
            colors: [0],
        };
        let mut buf = vec![0u8; (W * H * 4) as usize];
        let got = GetDIBits(
            mem,
            bmp,
            0,
            H as u32,
            buf.as_mut_ptr() as *mut c_void,
            &mut bi,
            0,
        );
        DeleteObject(bmp);
        DeleteDC(mem);
        ReleaseDC(null_mut(), screen);
        if ok == 0 || got == 0 {
            return None;
        }
        let (mut sr, mut sg, mut sb, mut sw_) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
        let mut lvl = 0.0f64;
        let mut npx = 0.0f64;
        for px in buf.chunks(4) {
            let b = px[0] as f64;
            let g = px[1] as f64;
            let r = px[2] as f64;
            let mx = r.max(g).max(b);
            let mn = r.min(g).min(b);
            let chroma = (mx - mn) / 255.0;
            let w = 0.02 + chroma * (mx / 255.0);
            sr += r * w;
            sg += g * w;
            sb += b * w;
            sw_ += w;
            lvl += (mx / 255.0).powf(gamma);
            npx += 1.0;
        }
        if sw_ <= 0.0 {
            return Some([0.0, 0.0, 0.0]);
        }
        let avg = [sr / sw_, sg / sw_, sb / sw_];
        let (h, s, v) = rgb_to_hsv(avg);
        if adapt {
            // Parlaklik: seridin ortalama aydinligina gore. Siyah -> vmin, koyu bordo -> dusuk,
            // acik/cirtlak renk -> tam parlaklik. Ton ve doygunluk renkli piksellerden gelir.
            let level = clamp01(lvl / npx.max(1.0) * 1.6);
            let vmin = clamp01(vmin);
            Some(hsv(h, (s * vivid).min(1.0), vmin + (1.0 - vmin) * level))
        } else {
            Some(hsv(h, (s * vivid).min(1.0), (v * 1.25).min(1.0)))
        }
    }
}

/// Sistem sesini (loopback) dinleyip seviye ve bas enerjisi uretir.
fn start_audio(sens: Arc<Sensors>) {
    std::thread::spawn(move || {
        use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
        let host = cpal::default_host();
        let dev = match host.default_output_device() {
            Some(d) => d,
            None => return,
        };
        let cfg = match dev.default_output_config() {
            Ok(c) => c,
            Err(_) => return,
        };
        let channels = (cfg.channels() as usize).max(1);
        let sr = cfg.sample_rate().0 as f32;
        let fmt = cfg.sample_format();
        let config: cpal::StreamConfig = cfg.into();
        let alpha = 1.0 - (-2.0 * std::f32::consts::PI * 150.0 / sr).exp();
        let s2 = sens.clone();
        let mut lp = 0.0f32;
        let err_fn = |_e: cpal::StreamError| {};
        let stream = match fmt {
            cpal::SampleFormat::F32 => dev.build_input_stream(
                &config,
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    let mut sum = 0.0f32;
                    let mut bsum = 0.0f32;
                    let mut n = 0usize;
                    for frame in data.chunks(channels) {
                        let m: f32 = frame.iter().sum::<f32>() / channels as f32;
                        sum += m * m;
                        lp += alpha * (m - lp);
                        bsum += lp * lp;
                        n += 1;
                    }
                    if n > 0 {
                        let rms = (sum / n as f32).sqrt();
                        let bass = (bsum / n as f32).sqrt();
                        s2.audio_level.store(rms.to_bits(), Ordering::Relaxed);
                        s2.audio_bass.store(bass.to_bits(), Ordering::Relaxed);
                        s2.audio_last_ms
                            .store(s2.epoch.elapsed().as_millis() as u64, Ordering::Relaxed);
                    }
                },
                err_fn,
                None,
            ),
            _ => return,
        };
        if let Ok(st) = stream {
            let _ = st.play();
            loop {
                std::thread::sleep(Duration::from_secs(1));
            }
        }
    });
}

// ============================================================================
//  Performans altyapisi & ThrottleStop Tarzi Kontrol
// ============================================================================

/// DCHU salt-okuma: GetDCHU_Data_Integer
type GetDataIntFn = unsafe extern "C" fn(i32, *mut i32) -> i32;
/// DCHU yazma: SetDCHU_Data
type SetDataIntFn = unsafe extern "C" fn(i32, *const u8, i32) -> i32;
/// AppSettings yazma: WriteAppSettings (FnKey ve CC40 ayar senkronizasyonu)
type WriteAppFn = unsafe extern "C" fn(i32, i32, i32, *const u8) -> i32;

fn load_dchu_get(dll_path: &str) -> Option<GetDataIntFn> {
    unsafe {
        let w = wide(dll_path);
        let h = LoadLibraryW(w.as_ptr());
        if h.is_null() { return None; }
        let p = GetProcAddress(h, b"GetDCHU_Data_Integer\0".as_ptr());
        if p.is_null() { return None; }
        Some(std::mem::transmute::<*mut c_void, GetDataIntFn>(p))
    }
}

fn load_dchu_set(dll_path: &str) -> Option<SetDataIntFn> {
    unsafe {
        let w = wide(dll_path);
        let h = LoadLibraryW(w.as_ptr());
        if h.is_null() { return None; }
        let p = GetProcAddress(h, b"SetDCHU_Data\0".as_ptr());
        if p.is_null() { return None; }
        Some(std::mem::transmute::<*mut c_void, SetDataIntFn>(p))
    }
}

fn load_dchu_app_write(dll_path: &str) -> Option<WriteAppFn> {
    unsafe {
        let w = wide(dll_path);
        let h = LoadLibraryW(w.as_ptr());
        if h.is_null() { return None; }
        let p = GetProcAddress(h, b"WriteAppSettings\0".as_ptr());
        if p.is_null() { return None; }
        Some(std::mem::transmute::<*mut c_void, WriteAppFn>(p))
    }
}

/// DCHU oku: InsydeDCHU.dll başarıda r'ye doğrudan okunan değeri döner (0 değil).
/// Hata kodu: -2147483646 (0x80000002).
fn dchu_read(f: GetDataIntFn, cmd: i32) -> Option<i32> {
    let mut v: i32 = 0;
    let r = unsafe { f(cmd, &mut v) };
    let val = if v != 0 { v } else { r };
    if val != -2147483646i32 && val != 0 {
        Some(val)
    } else {
        None
    }
}

#[allow(dead_code)]
fn dchu_send_u32(f: SetDataIntFn, cmd: i32, val: u32) -> bool {
    let bytes = val.to_le_bytes();
    let r = unsafe { f(cmd, bytes.as_ptr(), 4) };
    r >= 0
}

/// Kullanıcının masaüstündeki tstudio klasörünü bul
fn find_tstudio_dir() -> Option<std::path::PathBuf> {
    let up = std::env::var("USERPROFILE").unwrap_or_else(|_| "C:\\Users\\aliha".to_string());
    let p = std::path::PathBuf::from(up).join("Desktop").join("tstudio");
    if p.exists() { Some(p) } else { None }
}

/// ControlGPU.exe'nin yolunu bul (öncelik Masaüstü tstudio klasöründe)
fn find_control_gpu_exe() -> Option<String> {
    if let Some(dir) = find_tstudio_dir() {
        let p1 = dir.join("ControlGPU.exe");
        if p1.exists() { return Some(p1.to_string_lossy().to_string()); }
        let p2 = dir.join("FnKey").join("CC40").join("ControlGPU.exe");
        if p2.exists() { return Some(p2.to_string_lossy().to_string()); }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let p = dir.join("ControlGPU.exe");
            if p.exists() { return Some(p.to_string_lossy().to_string()); }
        }
    }
    find_cc40_fnkey_path().map(|p| format!("{}\\ControlGPU.exe", p))
}

/// cc30wk.exe'nin yolunu bul
fn find_cc30wk_exe() -> Option<String> {
    if let Some(dir) = find_tstudio_dir() {
        let p = dir.join("FnKey").join("cc30wk.exe");
        if p.exists() { return Some(p.to_string_lossy().to_string()); }
    }
    find_cc40_fnkey_path().map(|p| format!("{}\\cc30wk.exe", p))
}

/// Windows Güç Yönetimi üzerinden CPU Throttle & Turbo Boost kontrolü
fn apply_cpu_throttle(max_percent: u32) {
    let val_str = format!("{}", max_percent.min(100).max(50));
    let _ = std::process::Command::new("powercfg")
        .args(["/setacvalueindex", "SCHEME_CURRENT", "SUB_PROCESSOR", "PROCTHROTTLEMAX", &val_str])
        .creation_flags(0x0800_0000)
        .status();
    let _ = std::process::Command::new("powercfg")
        .args(["/setdcvalueindex", "SCHEME_CURRENT", "SUB_PROCESSOR", "PROCTHROTTLEMAX", &val_str])
        .creation_flags(0x0800_0000)
        .status();
    let _ = std::process::Command::new("powercfg")
        .args(["/setactive", "SCHEME_CURRENT"])
        .creation_flags(0x0800_0000)
        .status();
}

/// cc30wk.exe ile CC40 güç modunu çağır
fn trigger_cc30wk_mode(mode_val: u32) {
    if let Some(exe) = find_cc30wk_exe() {
        let _ = std::process::Command::new(&exe)
            .args(["0x0013", &format!("{}", mode_val)])
            .creation_flags(0x0800_0000)
            .status();
    }
}

/// GPU frekans offseti (Core & Memory OC/UV) uygula (ControlGPU.exe)
fn apply_gpu_offset(core_mhz: i16, mem_mhz: i16) {
    if let Some(exe) = find_control_gpu_exe() {
        let work_dir = std::path::Path::new(&exe).parent().map(|p| p.to_path_buf());
        let mut cmd = std::process::Command::new(&exe);
        // ControlGPU.exe argümanları iki nokta (:) ile ayrıştırır (Program.cs: args[0].Split(':'))
        cmd.arg(format!("set_increment:0:{}:{}", core_mhz, mem_mhz))
           .creation_flags(0x0800_0000);
        if let Some(w) = work_dir {
            cmd.current_dir(w);
        }
        let _ = cmd.status();
    }
}

/// GPU Thermal Limit ayarla (65C - 87C)
fn apply_gpu_thermal_limit(limit_c: i16) {
    if let Some(exe) = find_control_gpu_exe() {
        let work_dir = std::path::Path::new(&exe).parent().map(|p| p.to_path_buf());
        let mut cmd = std::process::Command::new(&exe);
        // ControlGPU.exe argümanları iki nokta (:) ile ayrıştırır (Program.cs: args[0].Split(':'))
        cmd.arg(format!("STL:{}", limit_c))
           .creation_flags(0x0800_0000);
        if let Some(w) = work_dir {
            cmd.current_dir(w);
        }
        let _ = cmd.status();
    }
}

/// Performans sekmesi için paylaşılan durum
struct PerfState {
    // Sensörler (DCHU EC)
    cpu_temp:     AtomicI32,
    gpu_temp_hw:  AtomicI32,
    cpu_fan_duty: AtomicI32,
    gpu_fan_duty: AtomicI32,
    fan_cpu_rpm:  AtomicI32,
    fan_gpu_rpm:  AtomicI32,
    charge_raw:   AtomicI32,
    sensor_ok:    AtomicBool,
    last_read_ms: AtomicU64,

    // Güncel Aktif Ayarlar
    active_preset:  AtomicU8, // 0=Perf, 1=Ent, 2=Quiet, 3=Save, 255=Custom
    cpu_max_pct:    AtomicU32, // 50 .. 100
    turbo_enabled:  AtomicBool,
    gpu_core_oc:    AtomicI32, // MHz (-200 .. +200)
    gpu_mem_oc:     AtomicI32, // MHz (-500 .. +500)
    gpu_temp_limit: AtomicI32, // 65 .. 87 C

    // Fan Ayarları
    fan_mode:       AtomicU8,  // 0 = Otomatik, 1 = Turbo %100, 2 = Manuel %
    fan_manual_pct: AtomicU32, // %30 .. %100

    // Fan basina kontrol (fan_mode == 2 iken kullanilir)
    fan1_pct: AtomicU32, // CPU fani %20..100
    fan2_pct: AtomicU32, // GPU fani %20..100
    fan_last_rc: AtomicI32,

    // Gercek CPU frekansi (MHz) ve onbellekli bulunma bayraklari
    cpu_mhz: AtomicU32,
    tstudio_found: AtomicBool,
    gpu_found: AtomicBool,

    // Emir kuyruğu
    apply_req: AtomicBool,
    fan_req: AtomicBool,
    apply_at_ms: AtomicU64,
}

impl PerfState {
    fn new() -> PerfState {
        PerfState {
            cpu_temp:     AtomicI32::new(-1),
            gpu_temp_hw:  AtomicI32::new(-1),
            cpu_fan_duty: AtomicI32::new(-1),
            gpu_fan_duty: AtomicI32::new(-1),
            fan_cpu_rpm:  AtomicI32::new(-1),
            fan_gpu_rpm:  AtomicI32::new(-1),
            charge_raw:   AtomicI32::new(-1),
            sensor_ok:    AtomicBool::new(false),
            last_read_ms: AtomicU64::new(0),

            active_preset:  AtomicU8::new(0),
            cpu_max_pct:    AtomicU32::new(100),
            turbo_enabled:  AtomicBool::new(true),
            gpu_core_oc:    AtomicI32::new(0),
            gpu_mem_oc:     AtomicI32::new(0),
            gpu_temp_limit: AtomicI32::new(86),

            fan_mode:       AtomicU8::new(0),
            fan_manual_pct: AtomicU32::new(70),

            fan1_pct: AtomicU32::new(60),
            fan2_pct: AtomicU32::new(60),
            fan_last_rc: AtomicI32::new(i32::MIN),

            cpu_mhz: AtomicU32::new(0),
            tstudio_found: AtomicBool::new(false),
            gpu_found: AtomicBool::new(false),

            apply_req: AtomicBool::new(false),
            fan_req: AtomicBool::new(false),
            apply_at_ms: AtomicU64::new(0),
        }
    }
}

/// CC40 kurulum yolunu bul
fn find_cc40_fnkey_path() -> Option<String> {
    static CACHE: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    CACHE.get_or_init(find_cc40_fnkey_path_uncached).clone()
}

fn find_cc40_fnkey_path_uncached() -> Option<String> {
    let known = "C:\\Program Files\\WindowsApps\\CLEVOCO.FnhotkeysandOSD_7.101.0.0_x64__6h6z29zh29qx0\\FnKey";
    if std::path::Path::new(known).join("ControlGPU.exe").exists() {
        return Some(known.to_string());
    }
    if let Ok(entries) = std::fs::read_dir("C:\\Program Files\\WindowsApps") {
        for e in entries.flatten() {
            let name = e.file_name();
            let n = name.to_string_lossy();
            if n.starts_with("CLEVOCO.") && n.contains("Fnhot") {
                let p = e.path().join("FnKey");
                if p.join("ControlGPU.exe").exists() {
                    return Some(p.to_string_lossy().to_string());
                }
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
//  Fan yazma (EC): CC40 ve FnKey.exe ile tam senkronize protokol
//  - SetDCHU_Data(121, 4 bayt: [data0, data1, data2, sub])
//  - WriteAppSettings(4, offset, len, &val): FnKey'in ayarı ezmesini engeller,
//    FnKey ile çatışmadan ve FnKey'i kapatmadan (hotkey'ler korunarak) çalışır!
// ---------------------------------------------------------------------------

fn setwmi4(sd: SetDataIntFn, cmd: i32, sub: u8, data: u32) -> i32 {
    let mut b = [0u8; 4];
    let d = data.to_le_bytes();
    b[0] = d[0];
    b[1] = d[1];
    b[2] = d[2];
    b[3] = sub;
    unsafe { sd(cmd, b.as_ptr(), 4) }
}

fn fan_apply(sf: SetDataIntFn, wf: Option<WriteAppFn>, perf: &PerfState) -> i32 {
    let mode = perf.fan_mode.load(Ordering::Relaxed);
    let v0 = 0u8;
    let v1 = 1u8;
    match mode {
        0 => {
            // Otomatik mod: FnKey ve CC40 uyumlu
            if let Some(wa) = wf {
                unsafe {
                    wa(4, 9, 1, &v0); // 1033 = 0 (Turbo fan kapalı)
                    wa(4, 5, 1, &v0); // 1029 = 0 (Otomatik mod)
                }
            }
            let r1 = setwmi4(sf, 121, 1, 0);
            let r2 = setwmi4(sf, 121, 34, 1);
            if r2 >= 0 { r2 } else { r1 }
        }
        1 => {
            // Turbo %100 fan (Maksimum Hız): FnKey korumalı (FnKey geri ezmez!)
            if let Some(wa) = wf {
                unsafe {
                    wa(4, 9, 1, &v1); // 1033 = 1 (Turbo fan açık)
                    wa(4, 5, 1, &v0); // 1029 = 0
                }
            }
            setwmi4(sf, 121, 1, 8) // Mode 8 = Turbo Fan
        }
        _ => {
            // Manuel % slider modu (20..100):
            let pct = perf.fan_manual_pct.load(Ordering::Relaxed).clamp(20, 100);
            let duty = ((pct as f64 / 100.0) * 255.0).round() as u32;
            let pct_byte = pct as u8;
            let mode6 = 6u8; // custom mode
            if let Some(wa) = wf {
                unsafe {
                    wa(4, 5, 1, &mode6);    // 1029 = 6 (Custom)
                    wa(4, 7, 1, &pct_byte); // 1031 = %
                    wa(4, 9, 1, &v0);       // 1033 = 0
                }
            }
            setwmi4(sf, 121, 14, duty) // sub 14 = Set Fan Duty
        }
    }
}

/// Agir isler (powercfg, ControlGPU, cc30wk) AYRI thread'de: sensor/fan dongusunu bloklamaz,
/// slider surukleme bitene kadar (400 ms) beklenir ve sadece DEGISEN ayar uygulanir.
fn spawn_apply_thread(perf: Arc<PerfState>, epoch: Instant) {
    std::thread::spawn(move || {
        let mut last_preset: Option<u8> = None;
        let mut last_cpu: Option<u32> = None;
        let mut last_gpu: Option<(i16, i16)> = None;
        let mut last_tlim: Option<i16> = None;
        loop {
            std::thread::sleep(Duration::from_millis(100));
            if !perf.apply_req.load(Ordering::Relaxed) {
                continue;
            }
            let now = epoch.elapsed().as_millis() as u64;
            if now < perf.apply_at_ms.load(Ordering::Relaxed) {
                continue;
            }
            perf.apply_req.store(false, Ordering::Relaxed);

            let preset = perf.active_preset.load(Ordering::Relaxed);
            if last_preset != Some(preset) {
                if preset == 0 {
                    trigger_cc30wk_mode(0);
                } else if preset == 2 || preset == 3 {
                    trigger_cc30wk_mode(1);
                }
                last_preset = Some(preset);
            }

            let turbo = perf.turbo_enabled.load(Ordering::Relaxed);
            let pct = if turbo { perf.cpu_max_pct.load(Ordering::Relaxed) } else { 99 };
            if last_cpu != Some(pct) {
                apply_cpu_throttle(pct);
                last_cpu = Some(pct);
            }

            let core = perf.gpu_core_oc.load(Ordering::Relaxed) as i16;
            let mem = perf.gpu_mem_oc.load(Ordering::Relaxed) as i16;
            if last_gpu != Some((core, mem)) {
                apply_gpu_offset(core, mem);
                last_gpu = Some((core, mem));
            }
            let t_lim = perf.gpu_temp_limit.load(Ordering::Relaxed) as i16;
            if last_tlim != Some(t_lim) {
                apply_gpu_thermal_limit(t_lim);
                last_tlim = Some(t_lim);
            }
        }
    });
}

/// Arkaplan thread'i: gercek EC sensorleri + fan yazma + CPU frekansi (500 ms'de bir)
fn spawn_perf_thread(perf: Arc<PerfState>, dll_path: Arc<Mutex<String>>, epoch: Instant) {
    std::thread::spawn(move || {
        let mut last_dll = String::new();
        let mut get_fn: Option<GetDataIntFn> = None;
        let mut set_fn: Option<SetDataIntFn> = None;
        let mut app_write_fn: Option<WriteAppFn> = None;
        let mut pdh = PdhFreq::new();
        let mut next_probe = 0u64;

        loop {
            let now = epoch.elapsed().as_millis() as u64;

            // Dosya var mi kontrolu: 10 snde bir (eskiden her karede yapiliyordu)
            if now >= next_probe {
                perf.tstudio_found.store(find_tstudio_dir().is_some(), Ordering::Relaxed);
                perf.gpu_found.store(find_control_gpu_exe().is_some(), Ordering::Relaxed);
                next_probe = now + 10_000;
            }

            // DLL guncelleme
            let dll = dll_path.lock().unwrap().clone();
            if dll != last_dll {
                last_dll = dll.clone();
                get_fn = load_dchu_get(&dll);
                set_fn = load_dchu_set(&dll);
                app_write_fn = load_dchu_app_write(&dll);
            }

            // Guvenlik: manuel/ozel fan modunda asiri sicaklikta otomatige don
            let ct = perf.cpu_temp.load(Ordering::Relaxed);
            let gt = perf.gpu_temp_hw.load(Ordering::Relaxed);
            if perf.fan_mode.load(Ordering::Relaxed) == 2 && (ct >= 88 || gt >= 85) {
                perf.fan_mode.store(0, Ordering::Relaxed);
                perf.fan_req.store(true, Ordering::Relaxed);
            }

            // Fan yazma istegi
            if perf.fan_req.swap(false, Ordering::Relaxed) {
                if let Some(sf) = set_fn {
                    let rc = fan_apply(sf, app_write_fn, &perf);
                    perf.fan_last_rc.store(rc, Ordering::Relaxed);
                }
            }

            // SENSOR OKUMA
            if let Some(f) = get_fn {
                if let Some(v99) = dchu_read(f, 99) {
                    let cpu_t = ((v99 >> 16) & 0xFF) as i32;
                    let cpu_d = ((v99 >> 8) & 0xFF) as i32;
                    if cpu_t > 0 && cpu_t < 115 {
                        perf.cpu_temp.store(cpu_t, Ordering::Relaxed);
                    }
                    if cpu_d >= 0 && cpu_d <= 100 {
                        perf.cpu_fan_duty.store(cpu_d, Ordering::Relaxed);
                    }
                }
                if let Some(v100) = dchu_read(f, 100) {
                    let gpu_t = ((v100 >> 16) & 0xFF) as i32;
                    let gpu_d = ((v100 >> 8) & 0xFF) as i32;
                    if gpu_t > 0 && gpu_t < 115 {
                        perf.gpu_temp_hw.store(gpu_t, Ordering::Relaxed);
                    }
                    if gpu_d >= 0 && gpu_d <= 100 {
                        perf.gpu_fan_duty.store(gpu_d, Ordering::Relaxed);
                    }
                }
                if let Some(v112) = dchu_read(f, 112) {
                    let f1 = (v112 & 0xFFFF) as i32;
                    let f2 = ((v112 >> 16) & 0xFFFF) as i32;
                    if f1 > 0 && f1 < 10000 {
                        perf.fan_cpu_rpm.store(f1, Ordering::Relaxed);
                    }
                    if f2 > 0 && f2 < 10000 {
                        perf.fan_gpu_rpm.store(f2, Ordering::Relaxed);
                    }
                }
                if let Some(cs) = dchu_read(f, 119) {
                    perf.charge_raw.store(cs, Ordering::Relaxed);
                }
                perf.sensor_ok.store(true, Ordering::Relaxed);
                perf.last_read_ms.store(epoch.elapsed().as_millis() as u64, Ordering::Relaxed);
            } else {
                perf.sensor_ok.store(false, Ordering::Relaxed);
            }

            // Gercek CPU frekansi
            if let Some(mhz) = pdh.read() {
                perf.cpu_mhz.store(mhz.round() as u32, Ordering::Relaxed);
            }

            std::thread::sleep(Duration::from_millis(500));
        }
    });
}

// ============================================================================
//  LED surucusu (InsydeDCHU.dll)
// ============================================================================

type SetDataFn = unsafe extern "C" fn(i32, *const u8, i32) -> i32;

struct Led {
    f: SetDataFn,
    last: u32,
}

fn c255(v: f64) -> u32 {
    v.max(0.0).min(255.0).round() as u32
}

impl Led {
    fn load(path: &str) -> Result<Led, String> {
        unsafe {
            let w = wide(path);
            let h = LoadLibraryW(w.as_ptr());
            if h.is_null() {
                return Err(format!("DLL yüklenemedi (Win32 hata {}): {}", GetLastError(), path));
            }
            let p = GetProcAddress(h, b"SetDCHU_Data\0".as_ptr());
            if p.is_null() {
                return Err("SetDCHU_Data bulunamadı".to_string());
            }
            let f: SetDataFn = std::mem::transmute::<*mut c_void, SetDataFn>(p);
            Ok(Led { f, last: 0 })
        }
    }

    /// true donerse gercekten gonderildi.
    fn send(&mut self, r: f64, g: f64, b: f64) -> bool {
        let ri = c255(r);
        let gi = c255(g);
        let bs = (0.7843137254901961 * c255(b) as f64).round() as u32;
        let v: u32 = (0xF0u32 << 24) | (bs << 16) | (ri << 8) | gi;
        if v == self.last {
            return false;
        }
        self.last = v;
        let bytes = v.to_le_bytes();
        unsafe {
            (self.f)(103, bytes.as_ptr(), 4);
        }
        true
    }
}

// ============================================================================
//  Motor: tum efektleri hesaplayip klavyeye gonderir
// ============================================================================

#[derive(Clone, Default)]
struct Status {
    color: [u8; 3],
    info: String,
    error: String,
    send_ms: f64,
}

fn poll_keys(
    prev: &mut [bool; 256],
    keys: &mut VecDeque<f64>,
    flash: &mut f64,
    flash_hue: &mut f64,
    glow: &mut f64,
    last_press: &mut f64,
    now: f64,
) {
    for vk in 8..255usize {
        if vk == 0x10 || vk == 0x11 || vk == 0x12 {
            continue;
        }
        let down = unsafe { ((GetAsyncKeyState(vk as i32) as u16) & 0x8000) != 0 };
        if down && !prev[vk] {
            keys.push_back(now);
            *last_press = now;
            *flash = 1.0;
            *flash_hue = (*flash_hue + 0.06).fract();
            *glow = (*glow + 0.07).min(1.0);
        }
        prev[vk] = down;
    }
}

fn info_text(s: &Settings, i: &Inputs) -> String {
    match s.mode {
        Mode::System => match s.sys_src {
            SysSrc::Cpu => format!("CPU yükü %{:.0}", i.cpu * 100.0),
            SysSrc::Ram => format!("RAM kullanımı %{:.0}", i.ram * 100.0),
            SysSrc::Gpu => {
                if i.gpu_temp < 0.0 {
                    "GPU sıcaklığı okunamadı (nvidia-smi bulunamadı mı?)".to_string()
                } else {
                    format!("GPU {:.0} °C", i.gpu_temp)
                }
            }
        },
        Mode::Battery => {
            if !i.battery_known {
                "Pil bilgisi yok".to_string()
            } else if i.charging {
                format!("Şarjda, pil %{:.0}", i.battery * 100.0)
            } else {
                format!("Pil %{:.0}", i.battery * 100.0)
            }
        }
        Mode::Ping => {
            if i.ping_ms < -1.5 {
                "Ping bekleniyor".to_string()
            } else if i.ping_ms < 0.0 {
                "Bağlantı YOK".to_string()
            } else {
                format!("Ping {:.0} ms", i.ping_ms)
            }
        }
        Mode::Music => format!("Ses seviyesi %{:.0}", i.audio * 100.0),
        Mode::Screen => format!(
            "Ekran rengi #{:02X}{:02X}{:02X}",
            c255(i.screen[0]),
            c255(i.screen[1]),
            c255(i.screen[2])
        ),
        Mode::TypingHeat => format!("Yazma hızı %{:.0}", i.typing * 100.0),
        Mode::Combine => format!(
            "Arka plan: {}{}",
            if s.cmb_bg { s.cmb_base.name() } else { "yok" },
            if s.cmb_keys && i.act > 0.5 { " • yazıyor" } else { "" }
        ),
        Mode::IdleActive => format!("Boşta {:.0} sn", i.idle_secs),
        Mode::DayTone => format!("Saat {:02}:{:02}", i.hour as i32, ((i.hour.fract()) * 60.0) as i32),
        Mode::Caps => {
            if i.caps {
                "Caps Lock AÇIK".to_string()
            } else {
                "Caps Lock kapalı".to_string()
            }
        }
        Mode::Pomodoro => match i.pomo_phase {
            1 => format!("Çalışma: {:02}:{:02} kaldı", (i.pomo_left / 60.0) as i32, (i.pomo_left % 60.0) as i32),
            2 => "Süre doldu! Mola zamanı".to_string(),
            3 => "Mola".to_string(),
            _ => "Durdu. Sağdan Başlat'a bas".to_string(),
        },
        _ => String::new(),
    }
}

fn engine(
    shared: Arc<Mutex<Settings>>,
    status: Arc<Mutex<Status>>,
    sens: Arc<Sensors>,
    quit: Arc<AtomicBool>,
) {
    let mut led: Option<Led> = None;
    let mut led_path = String::new();
    let mut err = String::new();
    let mut next_retry = 0.0f64;
    let mut next_force = 0.0f64;

    let mut prev_down = [false; 256];
    let mut keys: VecDeque<f64> = VecDeque::new();
    let mut flash = 0.0f64;
    let mut flash_hue = 0.0f64;
    let mut glow = 0.0f64;
    let mut flash_vis = 0.0f64;
    let mut act = 0.0f64;
    let mut last_press = -1000.0f64;
    let mut typing = 0.0f64;

    let mut last = 0.0f64;
    let mut cpu = 0.0f64;
    let mut ram = 0.0f64;
    let mut next_sys = 0.0f64;
    let mut prev_cpu: (u64, u64, u64) = (0, 0, 0);

    let mut battery = 0.0f64;
    let mut charging = false;
    let mut battery_known = false;
    let mut next_batt = 0.0f64;

    let mut audio_lvl = 0.0f64;
    let mut bass_avg = 0.0f64;
    let mut beat = 0.0f64;
    let mut audio_started = false;

    let mut screen_s = [0.0f64; 3];

    let mut pomo_phase = 0u8;
    let mut pomo_start = 0.0f64;
    let mut seen: Option<(u32, u32)> = None;

    let mut send_ms = 0.0f64;
    let mut last_host = String::new();

    while !quit.load(Ordering::Relaxed) {
        let s = shared.lock().unwrap().clone();
        let now = sens.epoch.elapsed().as_secs_f64();
        let mut dt = now - last;
        last = now;
        if dt > 0.5 {
            dt = 0.5;
        }
        if dt < 0.0001 {
            dt = 0.0001;
        }

        // sensor bayraklari
        // Combine modunda sensorler arka plan katmanina gore acilir
        let eff = if s.mode == Mode::Combine {
            if s.cmb_bg { s.cmb_base } else { Mode::Combine }
        } else {
            s.mode
        };
        sens.gpu_needed
            .store(eff == Mode::System && s.sys_src == SysSrc::Gpu, Ordering::Relaxed);
        sens.ping_needed.store(eff == Mode::Ping, Ordering::Relaxed);
        sens.screen_on.store(eff == Mode::Screen, Ordering::Relaxed);
        sens.screen_strip.store(f2u(s.screen_strip), Ordering::Relaxed);
        sens.screen_vivid.store(f2u(s.screen_vivid), Ordering::Relaxed);
        sens.screen_adapt.store(s.screen_adapt, Ordering::Relaxed);
        sens.screen_min.store(f2u(s.screen_min), Ordering::Relaxed);
        sens.screen_gamma.store(f2u(s.screen_gamma), Ordering::Relaxed);
        if s.ping_host != last_host {
            last_host = s.ping_host.clone();
            *sens.ping_host.lock().unwrap() = last_host.clone();
            sens.ping_ms.store(-2, Ordering::Relaxed);
        }
        if eff == Mode::Music && !audio_started {
            start_audio(sens.clone());
            audio_started = true;
        }

        // LED surucusu
        if s.dll_path != led_path || (led.is_none() && now >= next_retry) {
            led_path = s.dll_path.clone();
            match Led::load(&led_path) {
                Ok(l) => {
                    led = Some(l);
                    err.clear();
                }
                Err(e) => {
                    led = None;
                    err = e;
                    next_retry = now + 2.0;
                }
            }
        }
        if now >= next_force {
            if let Some(l) = led.as_mut() {
                l.last = 0;
            }
            next_force = now + 2.0;
        }

        // CPU / RAM
        if now >= next_sys {
            next_sys = now + 0.5;
            let (mut idle, mut ker, mut usr) = (0u64, 0u64, 0u64);
            unsafe {
                GetSystemTimes(&mut idle, &mut ker, &mut usr);
            }
            let total = (ker + usr).saturating_sub(prev_cpu.1 + prev_cpu.2);
            let idl = idle.saturating_sub(prev_cpu.0);
            if prev_cpu.1 != 0 && total > 0 {
                cpu = clamp01(1.0 - idl as f64 / total as f64);
            }
            prev_cpu = (idle, ker, usr);
            let mut ms: MemoryStatusEx = unsafe { std::mem::zeroed() };
            ms.length = std::mem::size_of::<MemoryStatusEx>() as u32;
            unsafe {
                if GlobalMemoryStatusEx(&mut ms) != 0 {
                    ram = ms.load as f64 / 100.0;
                }
            }
        }
        // Pil
        if now >= next_batt {
            next_batt = now + 2.0;
            let mut ps: PowerStatus = unsafe { std::mem::zeroed() };
            unsafe {
                if GetSystemPowerStatus(&mut ps) != 0 {
                    battery_known = ps.percent <= 100 && ps.flag != 128 && ps.flag != 255;
                    battery = (ps.percent.min(100)) as f64 / 100.0;
                    charging = ps.ac == 1;
                }
            }
        }
        // Saat
        let mut st: SystemTime = unsafe { std::mem::zeroed() };
        unsafe {
            GetLocalTime(&mut st);
        }
        let hour = st.hour as f64 + st.min as f64 / 60.0;

        // Bosta, caps
        let mut idle_secs = 0.0f64;
        let mut li = LastInputInfo { size: 8, time: 0 };
        unsafe {
            if GetLastInputInfo(&mut li) != 0 {
                idle_secs = GetTickCount().wrapping_sub(li.time) as f64 / 1000.0;
            }
        }
        let caps = unsafe { (GetKeyState(0x14) & 1) != 0 };

        // Yazma
        while let Some(&f) = keys.front() {
            if now - f > 3.0 {
                keys.pop_front();
            } else {
                break;
            }
        }
        let kps = keys.len() as f64 / 3.0;
        let target = clamp01(kps / 7.0);
        typing += (target - typing) * (1.0 - (-dt / 0.3).exp());
        flash *= (-dt / (0.4 / s.speed.max(0.25))).exp();
        glow *= (-dt / 2.5).exp();
        // Gorsel parlama: yumusak yukselis (~0.1 sn), sonra dogal sonum
        if flash > flash_vis {
            flash_vis += (flash - flash_vis) * (1.0 - (-dt / 0.03).exp());
        } else {
            flash_vis = flash;
        }
        // Yazma etkinligi: tusa basinca hizla 1, birakinca key_hold sn sonra yavasca 0
        let act_target = if now - last_press < s.key_hold { 1.0 } else { 0.0 };
        let act_tau = if act_target > act { 0.05 } else { 0.7 };
        act += (act_target - act) * (1.0 - (-dt / act_tau).exp());

        // Ses
        let age_ms = (sens.epoch.elapsed().as_millis() as u64)
            .saturating_sub(sens.audio_last_ms.load(Ordering::Relaxed));
        let (raw, braw) = if audio_started && age_ms < 250 {
            (
                f32::from_bits(sens.audio_level.load(Ordering::Relaxed)) as f64,
                f32::from_bits(sens.audio_bass.load(Ordering::Relaxed)) as f64,
            )
        } else {
            (0.0, 0.0)
        };
        if raw > audio_lvl {
            audio_lvl = raw;
        } else {
            audio_lvl += (raw - audio_lvl) * (1.0 - (-dt / 0.15).exp());
        }
        bass_avg += (braw - bass_avg) * (1.0 - (-dt / 1.0).exp());
        if braw > bass_avg * 1.35 && braw > 0.01 {
            beat = 1.0;
        }
        beat *= (-dt / 0.18).exp();

        // Ekran rengi (yumusatilmis)
        let packed = sens.screen_rgb.load(Ordering::Relaxed);
        let target_screen = [
            ((packed >> 16) & 0xFF) as f64,
            ((packed >> 8) & 0xFF) as f64,
            (packed & 0xFF) as f64,
        ];
        let k = 1.0 - (-dt / 0.25).exp();
        for c in 0..3 {
            screen_s[c] += (target_screen[c] - screen_s[c]) * k;
        }

        // Pomodoro
        let (sn, rn) = (s.pomo_start_nonce, s.pomo_reset_nonce);
        match seen {
            None => seen = Some((sn, rn)),
            Some((a, b)) => {
                if sn != a {
                    pomo_phase = 1;
                    pomo_start = now;
                }
                if rn != b {
                    pomo_phase = 0;
                }
                seen = Some((sn, rn));
            }
        }
        let mut pomo_frac = 0.0f64;
        let mut pomo_left = 0.0f64;
        let ph = pomo_phase;
        match ph {
            1 => {
                let dur = (s.pomo_work * 60.0).max(1.0);
                let el = now - pomo_start;
                pomo_frac = el / dur;
                pomo_left = (dur - el).max(0.0);
                if el >= dur {
                    pomo_phase = 2;
                    pomo_start = now;
                }
            }
            2 => {
                let el = now - pomo_start;
                pomo_frac = el / 8.0;
                if el >= 8.0 {
                    pomo_phase = 3;
                    pomo_start = now;
                }
            }
            3 => {
                let dur = (s.pomo_break * 60.0).max(1.0);
                let el = now - pomo_start;
                pomo_frac = el / dur;
                pomo_left = (dur - el).max(0.0);
                if el >= dur {
                    pomo_phase = 0;
                }
            }
            _ => {}
        }

        let inp = Inputs {
            flash,
            flash_vis,
            act,
            flash_hue,
            typing,
            glow,
            idle_secs,
            caps,
            cpu,
            ram,
            gpu_temp: sens.gpu_temp.load(Ordering::Relaxed) as f64,
            battery,
            charging,
            battery_known,
            ping_ms: sens.ping_ms.load(Ordering::Relaxed) as f64,
            audio: (audio_lvl * s.music_gain).min(1.0),
            bass: (braw * s.music_gain).min(1.0),
            beat,
            screen: screen_s,
            hour,
            pomo_phase,
            pomo_frac,
            pomo_left,
        };

        let mut c = render(&s, &inp, now);
        let kb = if s.on { s.brightness } else { 0.0 };
        c = [c[0] * kb, c[1] * kb, c[2] * kb];

        if let Some(l) = led.as_mut() {
            let t0 = Instant::now();
            if l.send(c[0], c[1], c[2]) {
                send_ms = t0.elapsed().as_secs_f64() * 1000.0;
            }
        }

        {
            let mut stt = status.lock().unwrap();
            stt.color = [c255(c[0]) as u8, c255(c[1]) as u8, c255(c[2]) as u8];
            stt.info = info_text(&s, &inp);
            stt.error = err.clone();
            stt.send_ms = send_ms;
        }

        // Bir sonraki kareye kadar bekle, bu sirada tus vuruslarini yakala
        let next_frame = now + 0.025;
        while !quit.load(Ordering::Relaxed) {
            let tn = sens.epoch.elapsed().as_secs_f64();
            if tn >= next_frame {
                break;
            }
            poll_keys(
                &mut prev_down,
                &mut keys,
                &mut flash,
                &mut flash_hue,
                &mut glow,
                &mut last_press,
                tn,
            );
            std::thread::sleep(Duration::from_millis(3));
        }
    }

    // Cikis: klavyeyi beyaza dondur
    if let Some(l) = led.as_mut() {
        l.last = 0;
        l.send(255.0, 255.0, 255.0);
    }
}

// ============================================================================
//  GUI
// ============================================================================

fn draw_keyboard(ui: &mut egui::Ui, rgb: [u8; 3]) {
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(w, 132.0), Sense::hover());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 14.0, Color32::from_rgb(9, 9, 13));
    p.rect_filled(
        rect,
        14.0,
        Color32::from_rgba_unmultiplied(rgb[0], rgb[1], rgb[2], 30),
    );
    let rows: [usize; 4] = [14, 14, 13, 12];
    let pad = 12.0f32;
    let gap = 4.0f32;
    let kw = ((rect.width() - pad * 2.0) / 14.0 - gap).max(6.0);
    let kh = ((rect.height() - pad * 2.0) / 5.0 - gap).max(6.0);
    let fill = Color32::from_rgba_unmultiplied(rgb[0], rgb[1], rgb[2], 60);
    let line = Color32::from_rgb(rgb[0], rgb[1], rgb[2]);
    for (ri, n) in rows.iter().enumerate() {
        let row_w = *n as f32 * (kw + gap) - gap;
        let x0 = rect.center().x - row_w / 2.0;
        let y = rect.top() + pad + ri as f32 * (kh + gap);
        for k in 0..*n {
            let r = Rect::from_min_size(
                Pos2::new(x0 + k as f32 * (kw + gap), y),
                egui::vec2(kw, kh),
            );
            p.rect_filled(r, 4.0, fill);
            p.rect_stroke(r, 4.0, Stroke::new(1.3_f32, line));
        }
    }
    let sw = kw * 6.0 + gap * 5.0;
    let sy = rect.top() + pad + 4.0 * (kh + gap);
    let r = Rect::from_min_size(
        Pos2::new(rect.center().x - sw / 2.0, sy),
        egui::vec2(sw, kh),
    );
    p.rect_filled(r, 4.0, fill);
    p.rect_stroke(r, 4.0, Stroke::new(1.3_f32, line));
}

fn draw_card(ui: &mut egui::Ui, m: Mode, selected: bool, base: &Settings, t: f64) -> bool {
    let size = egui::vec2(206.0, 108.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let p = ui.painter_at(rect);
    let bg = if resp.hovered() {
        Color32::from_rgb(36, 36, 50)
    } else {
        Color32::from_rgb(26, 26, 36)
    };
    p.rect_filled(rect, 12.0, bg);

    let strip = Rect::from_min_size(rect.min + egui::vec2(8.0, 8.0), egui::vec2(size.x - 16.0, 44.0));
    let mut ps = base.clone();
    ps.mode = m;
    let n = 40usize;
    let w = strip.width() / n as f32;
    for k in 0..n {
        let tt = t - (n - 1 - k) as f64 * 0.07;
        let inp = demo_inputs(tt, &ps);
        let c = render(&ps, &inp, tt);
        let x0 = strip.left() + w * k as f32;
        let r = Rect::from_min_max(
            Pos2::new(x0, strip.top()),
            Pos2::new(x0 + w + 0.6, strip.bottom()),
        );
        p.rect_filled(r, 0.0, to_c32(c));
    }
    p.rect_filled(
        Rect::from_min_size(rect.min + egui::vec2(8.0, 57.0), egui::vec2(26.0, 3.0)),
        2.0,
        m.accent(),
    );
    p.text(
        rect.min + egui::vec2(8.0, 64.0),
        Align2::LEFT_TOP,
        m.name(),
        FontId::proportional(15.0),
        Color32::WHITE,
    );
    p.text(
        rect.min + egui::vec2(8.0, 85.0),
        Align2::LEFT_TOP,
        m.desc(),
        FontId::proportional(11.5),
        Color32::from_gray(150),
    );
    if selected {
        p.rect_stroke(rect, 12.0, Stroke::new(2.5_f32, m.accent()));
    }
    resp.clicked()
}

fn hue_slider(ui: &mut egui::Ui, hue: &mut f64) {
    let w = ui.available_width().min(280.0);
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, 24.0), Sense::click_and_drag());
    let p = ui.painter_at(rect);
    let n = 72usize;
    let sw = rect.width() / n as f32;
    for k in 0..n {
        let c = to_c32(hsv(k as f64 / n as f64, 1.0, 1.0));
        let x0 = rect.left() + sw * k as f32;
        p.rect_filled(
            Rect::from_min_max(Pos2::new(x0, rect.top()), Pos2::new(x0 + sw + 0.6, rect.bottom())),
            0.0,
            c,
        );
    }
    if resp.dragged() || resp.clicked() {
        if let Some(pos) = resp.interact_pointer_pos() {
            *hue = ((pos.x - rect.left()) / rect.width()).max(0.0).min(1.0) as f64;
        }
    }
    let mx = rect.left() + rect.width() * (*hue as f32);
    p.line_segment(
        [Pos2::new(mx, rect.top() - 2.0), Pos2::new(mx, rect.bottom() + 2.0)],
        Stroke::new(3.0_f32, Color32::WHITE),
    );
    p.rect_stroke(rect, 3.0, Stroke::new(1.0_f32, Color32::from_gray(90)));
}

/// Tus katmani ayarlari (Tus dalgasi modu ve Combine ortak kullanir).
fn key_panel(ui: &mut egui::Ui, s: &mut Settings, show_hold: bool) {
    ui.checkbox(&mut s.key_mono, "Tek renk (renk değişmesin)");
    if s.key_mono {
        ui.checkbox(&mut s.key_white, "Beyaz");
        if !s.key_white {
            ui.label("Tuş rengi");
            hue_slider(ui, &mut s.key_hue);
        }
    } else {
        ui.label(
            egui::RichText::new("Her tuşta renk kayar (çok renkli)")
                .small()
                .color(Color32::from_gray(130)),
        );
    }
    ui.add(
        egui::Slider::new(&mut s.key_idle, 0.0..=50.0)
            .text("Boşta parlaklık %")
            .fixed_decimals(0),
    );
    if show_hold {
        ui.add(
            egui::Slider::new(&mut s.key_hold, 0.3..=6.0)
                .text("Yazma bitince arka plana dön (sn)")
                .fixed_decimals(1),
        );
    }
}

/// Hangi ana sekme secili?
#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab { Led, Perf }

impl App {
    /// LED sekmesi: sag panel + merkez kartlar (orijinal yerlesim)
    fn draw_led_tab(&mut self, ctx: &egui::Context, t: f64) {
        egui::SidePanel::right("opts").min_width(300.0).show(ctx, |ui| {
            ui.add_space(8.0);
            let m = self.settings.mode;
            let eff = if m == Mode::Combine {
                if self.settings.cmb_bg { self.settings.cmb_base } else { Mode::Combine }
            } else {
                m
            };
            ui.heading(egui::RichText::new(m.name()).color(m.accent()));
            ui.label(egui::RichText::new(m.desc()).color(Color32::from_gray(150)));
            ui.separator();

            if m == Mode::Combine {
                ui.label(egui::RichText::new("Birlikte çalışacak katmanları tikle:").color(Color32::from_gray(170)));
                ui.add_space(4.0);
                ui.checkbox(&mut self.settings.cmb_bg, "Arka plan katmanı (tuşa basılmazken)");
                if self.settings.cmb_bg {
                    egui::ComboBox::from_label("Arka plan modu")
                        .selected_text(self.settings.cmb_base.name())
                        .show_ui(ui, |ui| {
                            for b in Mode::BASES.iter() {
                                ui.selectable_value(&mut self.settings.cmb_base, *b, b.name());
                            }
                        });
                }
                ui.add_space(6.0);
                ui.checkbox(&mut self.settings.cmb_keys, "Tuş katmanı (yazarken devreye girer)");
                if self.settings.cmb_keys { key_panel(ui, &mut self.settings, true); }
                ui.add_space(6.0);
                ui.checkbox(&mut self.settings.cmb_caps, "Caps Lock katmanı");
                if self.settings.cmb_caps {
                    ui.label("Caps rengi");
                    hue_slider(ui, &mut self.settings.caps_hue);
                    ui.add(egui::Slider::new(&mut self.settings.caps_strength, 10.0..=100.0).text("Renk gücü %").fixed_decimals(0));
                }
                if self.settings.cmb_bg {
                    ui.add_space(6.0); ui.separator();
                    ui.label(egui::RichText::new(format!("Arka plan ayarları: {}", eff.name())).strong());
                    ui.add_space(4.0);
                }
            } else if m == Mode::KeyFlash {
                key_panel(ui, &mut self.settings, false);
                ui.add_space(6.0);
            }

            if eff.uses_hue() {
                ui.label("Renk tonu");
                hue_slider(ui, &mut self.settings.base_hue);
                ui.checkbox(&mut self.settings.white, "Beyaz (renksiz)");
                ui.add_space(6.0);
            }
            if eff.uses_speed() || (m == Mode::Combine && self.settings.cmb_keys) {
                ui.add(egui::Slider::new(&mut self.settings.speed, 0.25..=4.0).text("Hız"));
                ui.add_space(6.0);
            }

            match eff {
                Mode::System => {
                    let label = match self.settings.sys_src { SysSrc::Cpu => "CPU yükü", SysSrc::Ram => "RAM kullanımı", SysSrc::Gpu => "GPU sıcaklığı" };
                    egui::ComboBox::from_label("Kaynak").selected_text(label).show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.settings.sys_src, SysSrc::Cpu, "CPU yükü");
                        ui.selectable_value(&mut self.settings.sys_src, SysSrc::Ram, "RAM kullanımı");
                        ui.selectable_value(&mut self.settings.sys_src, SysSrc::Gpu, "GPU sıcaklığı (nvidia-smi)");
                    });
                    ui.label("Mavi = düşük, kırmızı = yüksek.");
                }
                Mode::IdleActive => {
                    ui.add(egui::Slider::new(&mut self.settings.idle_breathe_after, 2.0..=120.0).text("Nefese geç (sn)"));
                    ui.add(egui::Slider::new(&mut self.settings.idle_off_after, 10.0..=1800.0).logarithmic(true).text("Sön (sn)"));
                }
                Mode::Music => {
                    ui.add(egui::Slider::new(&mut self.settings.music_gain, 1.0..=12.0).text("Hassasiyet"));
                    ui.label("Sistemden çalan sesi dinler (YouTube, Spotify...).");
                }
                Mode::Screen => {
                    ui.add(egui::Slider::new(&mut self.settings.screen_strip, 4.0..=40.0).text("Şerit yüksekliği %"));
                    ui.add(egui::Slider::new(&mut self.settings.screen_vivid, 1.0..=3.0).text("Canlılık"));
                    ui.label("Birincil monitörün alt şeridi okunur.");
                    ui.checkbox(&mut self.settings.screen_adapt, "Ekran parlaklığına göre ışığı ayarla");
                    if self.settings.screen_adapt {
                        ui.add(egui::Slider::new(&mut self.settings.screen_min, 0.0..=40.0).text("En düşük parlaklık % (siyahta)").fixed_decimals(0));
                        ui.add(egui::Slider::new(&mut self.settings.screen_gamma, 1.0..=3.0).text("Koyu renk bastırma").fixed_decimals(1));
                        ui.label(egui::RichText::new("Siyah ve koyu bordo gibi renklerde ışık kısılır, açık ve cırtlak renklerde parlar.").small().color(Color32::from_gray(130)));
                    }
                }
                Mode::Pomodoro => {
                    ui.add(egui::Slider::new(&mut self.settings.pomo_work, 1.0..=90.0).text("Çalışma dk"));
                    ui.add(egui::Slider::new(&mut self.settings.pomo_break, 1.0..=30.0).text("Mola dk"));
                    ui.horizontal(|ui| {
                        if ui.button("▶ Başlat").clicked() { self.settings.pomo_start_nonce += 1; }
                        if ui.button("■ Sıfırla").clicked() { self.settings.pomo_reset_nonce += 1; }
                    });
                }
                Mode::Ping => {
                    ui.label("Hedef adres");
                    ui.text_edit_singleline(&mut self.settings.ping_host);
                }
                _ => {}
            }

            ui.add_space(14.0);
            ui.separator();
            ui.label(egui::RichText::new("InsydeDCHU.dll yolu").color(Color32::from_gray(150)));
            ui.text_edit_singleline(&mut self.settings.dll_path);
            // DLL yolu degisince perf thread'ine bildir
            {
                let mut shared_dll = self.perf_dll_path.lock().unwrap();
                if *shared_dll != self.settings.dll_path { *shared_dll = self.settings.dll_path.clone(); }
            }
            ui.label(egui::RichText::new("Control Center / CC40 kapalı olmalı.").small().color(Color32::from_gray(120)));
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.spacing_mut().item_spacing = egui::vec2(10.0, 10.0);
                ui.horizontal_wrapped(|ui| {
                    for m in Mode::ALL.iter() {
                        let selected = self.settings.mode == *m;
                        if draw_card(ui, *m, selected, &self.settings, t) { self.settings.mode = *m; }
                    }
                });
            });
        });
    }

    /// Performans sekmesi: 4 Hazir Mod + ThrottleStop Tarzi Advanced Ayarlar + Dogrulanmis Canli Sensorler + Fan Kontrolu
    fn draw_perf_tab(&mut self, ctx: &egui::Context) {
        let cpu_temp     = self.perf.cpu_temp.load(Ordering::Relaxed);
        let gpu_temp_hw  = self.perf.gpu_temp_hw.load(Ordering::Relaxed);
        let cpu_duty     = self.perf.cpu_fan_duty.load(Ordering::Relaxed);
        let gpu_duty     = self.perf.gpu_fan_duty.load(Ordering::Relaxed);
        let fan_cpu      = self.perf.fan_cpu_rpm.load(Ordering::Relaxed);
        let fan_gpu      = self.perf.fan_gpu_rpm.load(Ordering::Relaxed);
        let charge_raw   = self.perf.charge_raw.load(Ordering::Relaxed);
        let sensor_ok    = self.perf.sensor_ok.load(Ordering::Relaxed);
        let age_ms       = self.epoch.elapsed().as_millis() as u64;
        let last_read    = self.perf.last_read_ms.load(Ordering::Relaxed);
        let age_secs     = if last_read == 0 { 999u64 } else { (age_ms.saturating_sub(last_read)) / 1000 };

        let mut preset     = self.perf.active_preset.load(Ordering::Relaxed);
        let mut turbo      = self.perf.turbo_enabled.load(Ordering::Relaxed);
        let mut cpu_pct    = self.perf.cpu_max_pct.load(Ordering::Relaxed);
        let mut core_oc    = self.perf.gpu_core_oc.load(Ordering::Relaxed);
        let mut mem_oc     = self.perf.gpu_mem_oc.load(Ordering::Relaxed);
        let mut t_limit    = self.perf.gpu_temp_limit.load(Ordering::Relaxed);
        let mut fan_m      = self.perf.fan_mode.load(Ordering::Relaxed);
        let mut fan_pct    = self.perf.fan_manual_pct.load(Ordering::Relaxed);
        let mut changed    = false;

        let tstudio_found  = self.perf.tstudio_found.load(Ordering::Relaxed);
        let gpu_found      = self.perf.gpu_found.load(Ordering::Relaxed);
        let cpu_mhz        = self.perf.cpu_mhz.load(Ordering::Relaxed);
        let fan_rc         = self.perf.fan_last_rc.load(Ordering::Relaxed);
        let mut fan1       = self.perf.fan1_pct.load(Ordering::Relaxed);
        let mut fan2       = self.perf.fan2_pct.load(Ordering::Relaxed);

        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.add_space(8.0);

                // Durum cubugu
                ui.horizontal(|ui| {
                    if sensor_ok {
                        ui.colored_label(Color32::from_rgb(80, 220, 120), format!("● DCHU EC Bağlı ({} sn önce güncellendi)", age_secs));
                    } else {
                        ui.colored_label(Color32::from_rgb(255, 100, 80), "● DCHU Bekleniyor / Bağlanamadı");
                    }
                    ui.separator();
                    if tstudio_found {
                        ui.colored_label(Color32::from_rgb(80, 200, 120), "● Masaüstü tstudio Bağlı");
                    } else {
                        ui.colored_label(Color32::from_rgb(200, 150, 50), "⚠ Masaüstü tstudio Bulunamadı");
                    }
                    ui.separator();
                    if gpu_found {
                        ui.colored_label(Color32::from_rgb(80, 200, 120), "● GPU API Hazır");
                    } else {
                        ui.colored_label(Color32::from_gray(140), "○ GPU API Yok");
                    }
                });
                ui.add_space(10.0);

                // ============================================================
                //  1. BÖLÜM: 4 HAZIR GÜÇ MODU (CC40 Standartları)
                // ============================================================
                ui.heading(egui::RichText::new("⚡ Hızlı Güç Modları (CC40)").color(Color32::from_rgb(255, 180, 60)));
                ui.add_space(4.0);

                ui.horizontal(|ui| {
                    // 1. Performans
                    let is_p = preset == 0;
                    let p_btn = egui::Button::new(
                        egui::RichText::new("⚡ Performans\n(Maksimum Hız)").color(if is_p { Color32::WHITE } else { Color32::from_rgb(255, 160, 40) })
                    ).min_size(egui::vec2(140.0, 55.0)).fill(if is_p { Color32::from_rgb(180, 60, 20) } else { Color32::from_rgb(45, 30, 10) });

                    if ui.add(p_btn).clicked() {
                        preset = 0;
                        turbo = true;
                        cpu_pct = 100;
                        fan_m = 0; // Auto fan
                        changed = true;
                    }

                    // 2. Eğlence
                    let is_e = preset == 1;
                    let e_btn = egui::Button::new(
                        egui::RichText::new("🎬 Eğlence\n(Dengeli)").color(if is_e { Color32::WHITE } else { Color32::from_rgb(255, 210, 60) })
                    ).min_size(egui::vec2(140.0, 55.0)).fill(if is_e { Color32::from_rgb(160, 130, 20) } else { Color32::from_rgb(40, 35, 15) });

                    if ui.add(e_btn).clicked() {
                        preset = 1;
                        turbo = true;
                        cpu_pct = 100;
                        fan_m = 0;
                        changed = true;
                    }

                    // 3. Sessiz
                    let is_q = preset == 2;
                    let q_btn = egui::Button::new(
                        egui::RichText::new("🔇 Sessiz\n(Sessiz Fan)").color(if is_q { Color32::WHITE } else { Color32::from_rgb(100, 200, 255) })
                    ).min_size(egui::vec2(140.0, 55.0)).fill(if is_q { Color32::from_rgb(20, 80, 140) } else { Color32::from_rgb(15, 30, 45) });

                    if ui.add(q_btn).clicked() {
                        preset = 2;
                        turbo = false; // Sessiz modda Turbo kapalı (soğuk & sessiz)
                        cpu_pct = 85;
                        fan_m = 0;
                        changed = true;
                    }

                    // 4. Güç Tasarrufu
                    let is_s = preset == 3;
                    let s_btn = egui::Button::new(
                        egui::RichText::new("🍃 Güç Tasarrufu\n(Maksimum Pil)").color(if is_s { Color32::WHITE } else { Color32::from_rgb(90, 230, 120) })
                    ).min_size(egui::vec2(140.0, 55.0)).fill(if is_s { Color32::from_rgb(20, 120, 50) } else { Color32::from_rgb(15, 40, 20) });

                    if ui.add(s_btn).clicked() {
                        preset = 3;
                        turbo = false;
                        cpu_pct = 70;
                        fan_m = 0;
                        changed = true;
                    }
                });

                ui.add_space(14.0);
                ui.separator();

                // ============================================================
                //  2. BÖLÜM: FAN KONTROLÜ (RPM & % Slider & Turbo)
                // ============================================================
                ui.heading(egui::RichText::new("🌀 Fan Kontrolü & Ayarları").color(Color32::from_rgb(120, 220, 255)));
                ui.add_space(4.0);

                ui.group(|ui| {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("CPU Frekansı:").strong());
                        if cpu_mhz > 0 {
                            ui.colored_label(Color32::from_rgb(255, 190, 70), format!("{} MHz", cpu_mhz));
                        } else {
                            ui.label("— MHz");
                        }
                        ui.separator();
                        ui.label(egui::RichText::new("Anlık Devir:").strong());
                        if fan_cpu > 0 {
                            ui.colored_label(Color32::from_rgb(100, 220, 255), format!("CPU fan: {} RPM", fan_cpu));
                        } else {
                            ui.label("CPU fan: — RPM");
                        }
                        ui.separator();
                        if fan_gpu > 0 {
                            ui.colored_label(Color32::from_rgb(130, 255, 160), format!("GPU fan: {} RPM", fan_gpu));
                        } else {
                            ui.label("GPU fan: — RPM");
                        }
                        ui.separator();
                        ui.label(format!("Yük: CPU %{} / GPU %{}", cpu_duty.max(0), gpu_duty.max(0)));
                    });

                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        ui.label("Çalışma Rejimi:");
                        if ui.selectable_value(&mut fan_m, 0, "🌀 Otomatik").clicked() {
                            changed = true;
                        }
                        if ui.selectable_value(&mut fan_m, 1, "🚀 En fazla (iki fan %100)").clicked() {
                            changed = true;
                        }
                        if ui.selectable_value(&mut fan_m, 2, "🎚️ Özel (fan başına)").clicked() {
                            changed = true;
                        }
                    });

                    if fan_m == 2 {
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            ui.label("Fan Hız Limiti:");
                            if ui.add(egui::Slider::new(&mut fan_pct, 20..=100).text("%").fixed_decimals(0)).changed() {
                                fan1 = fan_pct;
                                fan2 = fan_pct;
                                changed = true;
                            }
                        });
                        let rc_txt = if fan_rc == i32::MIN {
                            "bekleniyor".to_string()
                        } else if fan_rc >= 0 {
                            format!("EC/FnKey senkronize (kod {})", fan_rc)
                        } else {
                            format!("EC Hata kodu {}", fan_rc)
                        };
                        ui.label(egui::RichText::new(rc_txt).small().color(Color32::from_gray(130)));
                        ui.label(egui::RichText::new("Güvenlik: CPU ≥88°C veya GPU ≥85°C olursa otomatik moda döner.").small().color(Color32::from_gray(110)));
                    }
                });

                ui.add_space(14.0);
                ui.separator();

                // ============================================================
                //  3. BÖLÜM: THROTTLESTOP TARZI GELİŞMİŞ AYARLAR (ADVANCED)
                // ============================================================
                ui.heading(egui::RichText::new("🛠️ Gelişmiş Ayarlar (ThrottleStop & GPU OC/UV)").color(Color32::from_rgb(130, 200, 255)));
                ui.add_space(4.0);

                ui.group(|ui| {
                    ui.horizontal(|ui| {
                        // CPU Turbo Switch
                        if ui.checkbox(&mut turbo, "⚡ CPU Turbo Boost").changed() {
                            preset = 255;
                            changed = true;
                        }
                        if turbo {
                            ui.colored_label(Color32::from_rgb(255, 150, 50), "(Aktif - Yüksek frekansa fırlar)");
                        } else {
                            ui.colored_label(Color32::from_rgb(100, 200, 255), "(Kapalı - Taban saat hızına kilitli, soğuk ve serin)");
                        }
                    });

                    ui.add_space(6.0);
                    // CPU Throttle Slider
                    ui.horizontal(|ui| {
                        ui.label("İşlemci Güç Limiti:");
                        if ui.add(egui::Slider::new(&mut cpu_pct, 50..=100).text("%").fixed_decimals(0)).changed() {
                            preset = 255;
                            changed = true;
                        }
                    });
                });

                ui.add_space(8.0);
                // GPU Overclock / Undervolt Grubu
                ui.group(|ui| {
                    ui.label(egui::RichText::new("🎮 NVIDIA GPU Frekans & Sıcaklık Limiti").strong());
                    ui.add_space(4.0);

                    ui.horizontal(|ui| {
                        ui.label("GPU Çekirdek Frekansı (Core Offset):");
                        if ui.add(egui::Slider::new(&mut core_oc, -200..=200).text("MHz")).changed() {
                            preset = 255;
                            changed = true;
                        }
                    });

                    ui.horizontal(|ui| {
                        ui.label("GPU Bellek Frekansı (Mem Offset):");
                        if ui.add(egui::Slider::new(&mut mem_oc, -500..=500).text("MHz")).changed() {
                            preset = 255;
                            changed = true;
                        }
                    });

                    ui.horizontal(|ui| {
                        ui.label("GPU Sıcaklık Limiti (Thermal Limit):");
                        if ui.add(egui::Slider::new(&mut t_limit, 65..=87).text("°C")).changed() {
                            preset = 255;
                            changed = true;
                        }
                    });

                    if ui.button("↺ GPU Ayarlarını Sıfırla (0 MHz)").clicked() {
                        core_oc = 0;
                        mem_oc = 0;
                        t_limit = 86;
                        preset = 255;
                        changed = true;
                    }
                });

                // Değişiklik olduysa arka plana uygula emri ver
                if changed {
                    self.perf.active_preset.store(preset, Ordering::Relaxed);
                    self.perf.turbo_enabled.store(turbo, Ordering::Relaxed);
                    self.perf.cpu_max_pct.store(cpu_pct, Ordering::Relaxed);
                    self.perf.gpu_core_oc.store(core_oc, Ordering::Relaxed);
                    self.perf.gpu_mem_oc.store(mem_oc, Ordering::Relaxed);
                    self.perf.gpu_temp_limit.store(t_limit, Ordering::Relaxed);
                    self.perf.fan_mode.store(fan_m, Ordering::Relaxed);
                    self.perf.fan_manual_pct.store(fan_pct, Ordering::Relaxed);
                    self.perf.fan1_pct.store(fan1, Ordering::Relaxed);
                    self.perf.fan2_pct.store(fan2, Ordering::Relaxed);
                    let now_ms = self.epoch.elapsed().as_millis() as u64;
                    self.perf.apply_at_ms.store(now_ms + 400, Ordering::Relaxed); // slider birakilana kadar bekle
                    self.perf.apply_req.store(true, Ordering::Relaxed);
                    self.perf.fan_req.store(true, Ordering::Relaxed);
                }

                ui.add_space(14.0);
                ui.separator();

                // ============================================================
                //  4. BÖLÜM: GERÇEK ZAMANLI DONANIM SENSÖRLERİ (DOĞRU EC VERİSİ)
                // ============================================================
                ui.heading(egui::RichText::new("🌡️ Canlı Donanım Sensörleri (DCHU EC)").color(Color32::from_rgb(90, 220, 150)));
                ui.add_space(6.0);

                egui::Grid::new("hw_sensors_grid").num_columns(3).spacing([24.0, 8.0]).show(ui, |ui| {
                    // CPU Frekansı (PDH)
                    ui.label(egui::RichText::new("CPU Frekansı").color(Color32::from_gray(210)));
                    if cpu_mhz > 0 {
                        ui.label(format!("{} MHz", cpu_mhz));
                    } else {
                        ui.colored_label(Color32::from_gray(100), "Okunuyor...");
                    }
                    ui.label(egui::RichText::new("Windows PDH").small().color(Color32::from_gray(120)));
                    ui.end_row();

                    // CPU Sıcaklığı
                    ui.label(egui::RichText::new("CPU Sıcaklığı").color(Color32::from_gray(210)));
                    if cpu_temp > 0 {
                        ui.colored_label(temp_color(cpu_temp), format!("{} °C", cpu_temp));
                    } else {
                        ui.colored_label(Color32::from_gray(100), "Okunuyor...");
                    }
                    ui.label(egui::RichText::new(format!("Fan: %{}", cpu_duty.max(0))).small().color(Color32::from_gray(120)));
                    ui.end_row();

                    // GPU Sıcaklığı
                    ui.label(egui::RichText::new("GPU Sıcaklığı").color(Color32::from_gray(210)));
                    if gpu_temp_hw > 0 {
                        ui.colored_label(temp_color(gpu_temp_hw), format!("{} °C", gpu_temp_hw));
                    } else {
                        ui.colored_label(Color32::from_gray(100), "Okunuyor...");
                    }
                    ui.label(egui::RichText::new(format!("Fan: %{}", gpu_duty.max(0))).small().color(Color32::from_gray(120)));
                    ui.end_row();

                    // CPU Fan Hızı
                    ui.label(egui::RichText::new("CPU Fan Devri").color(Color32::from_gray(210)));
                    if fan_cpu > 0 {
                        ui.label(format!("{} RPM", fan_cpu));
                    } else {
                        ui.colored_label(Color32::from_gray(100), "—");
                    }
                    ui.label(egui::RichText::new("Fan #1 (CPU)").small().color(Color32::from_gray(120)));
                    ui.end_row();

                    // GPU Fan Hızı
                    ui.label(egui::RichText::new("GPU Fan Devri").color(Color32::from_gray(210)));
                    if fan_gpu > 0 {
                        ui.label(format!("{} RPM", fan_gpu));
                    } else {
                        ui.colored_label(Color32::from_gray(100), "—");
                    }
                    ui.label(egui::RichText::new("Fan #2 (GPU)").small().color(Color32::from_gray(120)));
                    ui.end_row();

                    // Şarj Durumu
                    ui.label(egui::RichText::new("Güç Durumu").color(Color32::from_gray(210)));
                    if charge_raw >= 0 {
                        let state = match (charge_raw >> 16) & 0xFF {
                            1 => "⚡ Şarj Ediliyor",
                            2 => "🔋 Pil Dolu",
                            _ => "🔌 Fişe Takılı",
                        };
                        ui.label(state);
                    } else {
                        ui.colored_label(Color32::from_gray(100), "—");
                    }
                    ui.label(egui::RichText::new(format!("0x{:06X}", charge_raw)).small().color(Color32::from_gray(100)));
                    ui.end_row();
                });

                ui.add_space(14.0);
                ui.separator();
                ui.label(egui::RichText::new(
                    "💡 tstudio Entegrasyonu: DLL ve EXE dosyaları doğrudan masaüstündeki tstudio klasöründen dinamik olarak okunur. Repoya harici ikili dosya eklenmesine gerek yoktur."
                ).small().color(Color32::from_gray(140)));
            });
        });
    }
}

/// Sıcaklığa göre renk: soğuk=mavi, orta=sarı, sıcak=kırmızı
fn temp_color(t: i32) -> Color32 {
    if t < 50 { Color32::from_rgb(80, 180, 255) }
    else if t < 70 { Color32::from_rgb(255, 220, 60) }
    else if t < 85 { Color32::from_rgb(255, 140, 40) }
    else { Color32::from_rgb(255, 70, 70) }
}

struct App {
    settings: Settings,
    shared: Arc<Mutex<Settings>>,
    status: Arc<Mutex<Status>>,
    t0: Instant,
    saved_text: String,
    // Aktif sekme
    tab: Tab,
    // Performans altyapisi
    perf: Arc<PerfState>,
    perf_dll_path: Arc<Mutex<String>>,
    epoch: Instant,
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let t = self.t0.elapsed().as_secs_f64();
        let st = self.status.lock().unwrap().clone();
        let dirty = self.tab == Tab::Led && self.settings.to_text() != self.saved_text;

        // ----------------------------------------------------------------
        //  UST PANEL: baslik + klavye + sekme secimi
        // ----------------------------------------------------------------
        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.heading(
                    egui::RichText::new("TULPAR LED STUDIO")
                        .color(Color32::from_rgb(130, 200, 255))
                        .strong(),
                );
                ui.label(
                    egui::RichText::new("tek bölgeli klavye + performans merkezi")
                        .color(Color32::from_gray(140)),
                );
            });
            ui.add_space(4.0);
            draw_keyboard(ui, st.color);
            ui.add_space(6.0);

            // Sekme butonlari
            ui.horizontal(|ui| {
                let led_btn = egui::Button::new(
                    egui::RichText::new("💡  LED Efektleri").color(
                        if self.tab == Tab::Led { Color32::from_rgb(130, 200, 255) }
                        else { Color32::from_gray(180) }
                    )
                ).fill(
                    if self.tab == Tab::Led { Color32::from_rgb(28, 40, 60) }
                    else { Color32::from_rgb(20, 20, 28) }
                );
                if ui.add(led_btn).clicked() { self.tab = Tab::Led; }

                let perf_btn = egui::Button::new(
                    egui::RichText::new("⚡  Performans").color(
                        if self.tab == Tab::Perf { Color32::from_rgb(255, 180, 60) }
                        else { Color32::from_gray(180) }
                    )
                ).fill(
                    if self.tab == Tab::Perf { Color32::from_rgb(50, 36, 10) }
                    else { Color32::from_rgb(20, 20, 28) }
                );
                if ui.add(perf_btn).clicked() { self.tab = Tab::Perf; }

                ui.separator();
                if self.tab == Tab::Led {
                    ui.checkbox(&mut self.settings.on, "Işık açık");
                    ui.add(
                        egui::Slider::new(&mut self.settings.brightness, 0.05..=1.0)
                            .text("Parlaklık")
                            .fixed_decimals(2),
                    );
                    ui.separator();
                    if ui.button("Kaydet").clicked() {
                        self.settings.save();
                        self.saved_text = self.settings.to_text();
                    }
                    if dirty {
                        ui.colored_label(
                            Color32::from_rgb(255, 200, 90),
                            "● kaydedilmemiş değişiklik var",
                        );
                    } else {
                        ui.colored_label(Color32::from_gray(130), "✓ kaydedildi (program bu ayarla açılır)");
                    }
                }
            });
            ui.add_space(6.0);
        });

        // ----------------------------------------------------------------
        //  ALT PANEL: durum cubugu
        // ----------------------------------------------------------------
        egui::TopBottomPanel::bottom("bottom").show(ctx, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if st.error.is_empty() {
                    ui.colored_label(Color32::from_rgb(110, 220, 130), "● LED bağlı");
                    ui.label(format!("gönderim {:.1} ms", st.send_ms));
                } else {
                    ui.colored_label(Color32::from_rgb(255, 100, 100), format!("● {}", st.error));
                }
                ui.separator();
                ui.label(&st.info);
                ui.separator();
                ui.label(format!(
                    "#{:02X}{:02X}{:02X}",
                    st.color[0], st.color[1], st.color[2]
                ));
            });
            ui.add_space(4.0);
        });

        // ----------------------------------------------------------------
        //  SEKMELERİN İÇERİĞİ
        // ----------------------------------------------------------------
        match self.tab {
            Tab::Led => self.draw_led_tab(ctx, t),
            Tab::Perf => self.draw_perf_tab(ctx),
        }

        *self.shared.lock().unwrap() = self.settings.clone();
        ctx.request_repaint_after(Duration::from_millis(if self.tab == Tab::Led { 33 } else { 80 }));
    }
}

fn setup_style(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();
    v.panel_fill = Color32::from_rgb(16, 16, 22);
    v.window_fill = Color32::from_rgb(20, 20, 28);
    v.extreme_bg_color = Color32::from_rgb(10, 10, 14);
    ctx.set_visuals(v);
}

fn main() -> Result<(), eframe::Error> {
    // Yonetici degilse kendini yonetici olarak yeniden baslat.
    unsafe {
        if IsUserAnAdmin() == 0 {
            if let Ok(exe) = std::env::current_exe() {
                let wexe = wide(&exe.to_string_lossy());
                let verb = wide("runas");
                ShellExecuteW(null_mut(), verb.as_ptr(), wexe.as_ptr(), null(), null(), 1);
            }
            return Ok(());
        }
    }

    let settings = Settings::load();
    let first_text = settings.to_text();
    let shared = Arc::new(Mutex::new(settings.clone()));
    let status = Arc::new(Mutex::new(Status::default()));
    let sens = Arc::new(Sensors::new());
    let quit = Arc::new(AtomicBool::new(false));

    // Performans altyapisi
    let perf = Arc::new(PerfState::new());
    let perf_dll_path = Arc::new(Mutex::new(settings.dll_path.clone()));
    let epoch = sens.epoch;
    spawn_perf_thread(perf.clone(), perf_dll_path.clone(), epoch);
    spawn_apply_thread(perf.clone(), epoch);

    spawn_sensor_threads(sens.clone());

    let eng = {
        let (a, b, c, d) = (shared.clone(), status.clone(), sens.clone(), quit.clone());
        std::thread::spawn(move || engine(a, b, c, d))
    };

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1150.0, 760.0])
            .with_min_inner_size([900.0, 620.0]),
        ..Default::default()
    };

    let app = App {
        settings,
        shared,
        status,
        t0: Instant::now(),
        saved_text: first_text,
        tab: Tab::Led,
        perf,
        perf_dll_path,
        epoch,
    };

    let res = eframe::run_native(
        "Tulpar LED Studio",
        options,
        Box::new(move |cc| {
            setup_style(&cc.egui_ctx);
            Ok(Box::new(app))
        }),
    );

    quit.store(true, Ordering::Relaxed);
    let _ = eng.join();
    res
}
