#![windows_subsystem = "windows"]
//! Tulpar LED Studio - Tulpar T5 V23.4 tek bolgeli klavye icin efekt programi.
//! InsydeDCHU.dll -> SetDCHU_Data(103, ...) yolunu kullanir (tulpar_led.ps1 ile ayni yontem).

use eframe::egui;
use egui::{Align2, Color32, FontId, Pos2, Rect, Sense, Stroke};
use std::collections::VecDeque;
use std::f64::consts::PI;
use std::ffi::c_void;
use std::os::windows::process::CommandExt;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64, Ordering};
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
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SysSrc {
    Cpu,
    Ram,
    Gpu,
}

impl Mode {
    const ALL: [Mode; 19] = [
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
        }
    }

    fn desc(self) -> &'static str {
        match self {
            Mode::Static => "Tek renk, istediğin ton",
            Mode::Rainbow => "Yumuşak sürekli renk geçişi",
            Mode::Breathe => "Yavaşça parlayıp sönen ışık",
            Mode::KeyFlash => "Tuşa basınca ışık parlar",
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
        }
    }

    fn uses_hue(self) -> bool {
        matches!(
            self,
            Mode::Static
                | Mode::Breathe
                | Mode::KeyFlash
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
}

fn default_dll() -> String {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let p = dir.join("InsydeDCHU.dll");
            if p.exists() {
                return p.to_string_lossy().to_string();
            }
        }
    }
    let up = std::env::var("USERPROFILE").unwrap_or_else(|_| "C:\\Users\\aliha".to_string());
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

/// Parlaklik haric saf renk uretimi (parlaklik motorda uygulanir). t = saniye.
fn render(s: &Settings, i: &Inputs, t: f64) -> [f64; 3] {
    let sat = if s.white { 0.0 } else { 1.0 };
    let sp = s.speed;
    match s.mode {
        Mode::Static => hsv(s.base_hue, sat, 1.0),
        Mode::Rainbow => hsv(t * sp / 10.0, 1.0, 1.0),
        Mode::Breathe => {
            let x = (1.0 - (2.0 * PI * t * sp / 4.0).cos()) / 2.0;
            hsv(s.base_hue, sat, 0.03 + 0.97 * x.powf(1.6))
        }
        Mode::KeyFlash => lerp3(hsv(s.base_hue, sat, 0.18), hsv(i.flash_hue, 1.0, 1.0), i.flash),
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
    }
}

/// Onizleme kartlari icin sentetik girdiler.
fn demo_inputs(t: f64, s: &Settings) -> Inputs {
    let cyc = t / 1.4;
    let flash = (-(cyc.fract()) * 5.0).exp();
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
        if let Some(c) = capture_strip(strip, vivid) {
            let r = c[0].max(0.0).min(255.0) as u32;
            let g = c[1].max(0.0).min(255.0) as u32;
            let b = c[2].max(0.0).min(255.0) as u32;
            s3.screen_rgb.store((r << 16) | (g << 8) | b, Ordering::Relaxed);
        }
        std::thread::sleep(Duration::from_millis(40));
    });
}

/// Ekranin alt %strip_pct kismini kucultup renk ortalamasi alir.
fn capture_strip(strip_pct: f64, vivid: f64) -> Option<[f64; 3]> {
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
        }
        if sw_ <= 0.0 {
            return Some([0.0, 0.0, 0.0]);
        }
        let avg = [sr / sw_, sg / sw_, sb / sw_];
        let (h, s, v) = rgb_to_hsv(avg);
        Some(hsv(h, (s * vivid).min(1.0), (v * 1.25).min(1.0)))
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
    now: f64,
) {
    for vk in 8..255usize {
        if vk == 0x10 || vk == 0x11 || vk == 0x12 {
            continue;
        }
        let down = unsafe { ((GetAsyncKeyState(vk as i32) as u16) & 0x8000) != 0 };
        if down && !prev[vk] {
            keys.push_back(now);
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
        sens.gpu_needed
            .store(s.mode == Mode::System && s.sys_src == SysSrc::Gpu, Ordering::Relaxed);
        sens.ping_needed.store(s.mode == Mode::Ping, Ordering::Relaxed);
        sens.screen_on.store(s.mode == Mode::Screen, Ordering::Relaxed);
        sens.screen_strip.store(f2u(s.screen_strip), Ordering::Relaxed);
        sens.screen_vivid.store(f2u(s.screen_vivid), Ordering::Relaxed);
        if s.ping_host != last_host {
            last_host = s.ping_host.clone();
            *sens.ping_host.lock().unwrap() = last_host.clone();
            sens.ping_ms.store(-2, Ordering::Relaxed);
        }
        if s.mode == Mode::Music && !audio_started {
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
            poll_keys(&mut prev_down, &mut keys, &mut flash, &mut flash_hue, &mut glow, tn);
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
            p.rect_stroke(r, 4.0, Stroke::new(1.3, line));
        }
    }
    let sw = kw * 6.0 + gap * 5.0;
    let sy = rect.top() + pad + 4.0 * (kh + gap);
    let r = Rect::from_min_size(
        Pos2::new(rect.center().x - sw / 2.0, sy),
        egui::vec2(sw, kh),
    );
    p.rect_filled(r, 4.0, fill);
    p.rect_stroke(r, 4.0, Stroke::new(1.3, line));
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
        p.rect_stroke(rect, 12.0, Stroke::new(2.5, m.accent()));
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
        Stroke::new(3.0, Color32::WHITE),
    );
    p.rect_stroke(rect, 3.0, Stroke::new(1.0, Color32::from_gray(90)));
}

struct App {
    settings: Settings,
    shared: Arc<Mutex<Settings>>,
    status: Arc<Mutex<Status>>,
    t0: Instant,
    saved_text: String,
    pending_text: String,
    pending_since: Instant,
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let t = self.t0.elapsed().as_secs_f64();
        let st = self.status.lock().unwrap().clone();

        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.heading(
                    egui::RichText::new("TULPAR LED STUDIO")
                        .color(Color32::from_rgb(130, 200, 255))
                        .strong(),
                );
                ui.label(
                    egui::RichText::new("tek bölgeli klavye efekt merkezi")
                        .color(Color32::from_gray(140)),
                );
            });
            ui.add_space(4.0);
            draw_keyboard(ui, st.color);
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.checkbox(&mut self.settings.on, "Işık açık");
                ui.add(
                    egui::Slider::new(&mut self.settings.brightness, 0.05..=1.0)
                        .text("Parlaklık")
                        .fixed_decimals(2),
                );
            });
            ui.add_space(6.0);
        });

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

        egui::SidePanel::right("opts").min_width(300.0).show(ctx, |ui| {
            ui.add_space(8.0);
            let m = self.settings.mode;
            ui.heading(egui::RichText::new(m.name()).color(m.accent()));
            ui.label(egui::RichText::new(m.desc()).color(Color32::from_gray(150)));
            ui.separator();

            if m.uses_hue() {
                ui.label("Renk tonu");
                hue_slider(ui, &mut self.settings.base_hue);
                ui.checkbox(&mut self.settings.white, "Beyaz (renksiz)");
                ui.add_space(6.0);
            }
            if m.uses_speed() {
                ui.add(egui::Slider::new(&mut self.settings.speed, 0.25..=4.0).text("Hız"));
                ui.add_space(6.0);
            }

            match m {
                Mode::System => {
                    let label = match self.settings.sys_src {
                        SysSrc::Cpu => "CPU yükü",
                        SysSrc::Ram => "RAM kullanımı",
                        SysSrc::Gpu => "GPU sıcaklığı",
                    };
                    egui::ComboBox::from_label("Kaynak")
                        .selected_text(label)
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut self.settings.sys_src, SysSrc::Cpu, "CPU yükü");
                            ui.selectable_value(&mut self.settings.sys_src, SysSrc::Ram, "RAM kullanımı");
                            ui.selectable_value(&mut self.settings.sys_src, SysSrc::Gpu, "GPU sıcaklığı (nvidia-smi)");
                        });
                    ui.label("Mavi = düşük, kırmızı = yüksek.");
                }
                Mode::IdleActive => {
                    ui.add(
                        egui::Slider::new(&mut self.settings.idle_breathe_after, 2.0..=120.0)
                            .text("Nefese geç (sn)"),
                    );
                    ui.add(
                        egui::Slider::new(&mut self.settings.idle_off_after, 10.0..=1800.0)
                            .logarithmic(true)
                            .text("Sön (sn)"),
                    );
                }
                Mode::Music => {
                    ui.add(
                        egui::Slider::new(&mut self.settings.music_gain, 1.0..=12.0).text("Hassasiyet"),
                    );
                    ui.label("Sistemden çalan sesi dinler (YouTube, Spotify...).");
                }
                Mode::Screen => {
                    ui.add(
                        egui::Slider::new(&mut self.settings.screen_strip, 4.0..=40.0)
                            .text("Şerit yüksekliği %"),
                    );
                    ui.add(
                        egui::Slider::new(&mut self.settings.screen_vivid, 1.0..=3.0)
                            .text("Canlılık"),
                    );
                    ui.label("Birincil monitörün alt şeridi okunur.");
                }
                Mode::Pomodoro => {
                    ui.add(egui::Slider::new(&mut self.settings.pomo_work, 1.0..=90.0).text("Çalışma dk"));
                    ui.add(egui::Slider::new(&mut self.settings.pomo_break, 1.0..=30.0).text("Mola dk"));
                    ui.horizontal(|ui| {
                        if ui.button("▶ Başlat").clicked() {
                            self.settings.pomo_start_nonce += 1;
                        }
                        if ui.button("■ Sıfırla").clicked() {
                            self.settings.pomo_reset_nonce += 1;
                        }
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
            ui.label(
                egui::RichText::new("Control Center / CC40 kapalı olmalı.")
                    .small()
                    .color(Color32::from_gray(120)),
            );
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.spacing_mut().item_spacing = egui::vec2(10.0, 10.0);
                ui.horizontal_wrapped(|ui| {
                    for m in Mode::ALL.iter() {
                        let selected = self.settings.mode == *m;
                        if draw_card(ui, *m, selected, &self.settings, t) {
                            self.settings.mode = *m;
                        }
                    }
                });
            });
        });

        // Ayar degisince 0.8 sn sonra otomatik kaydet
        let cur = self.settings.to_text();
        if cur != self.pending_text {
            self.pending_text = cur;
            self.pending_since = Instant::now();
        } else if self.pending_text != self.saved_text
            && self.pending_since.elapsed() > Duration::from_millis(800)
        {
            write_settings_text(&self.pending_text);
            self.saved_text = self.pending_text.clone();
        }

        *self.shared.lock().unwrap() = self.settings.clone();
        ctx.request_repaint_after(Duration::from_millis(33));
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
    let shared_exit = shared.clone();
    let status = Arc::new(Mutex::new(Status::default()));
    let sens = Arc::new(Sensors::new());
    let quit = Arc::new(AtomicBool::new(false));

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
        saved_text: first_text.clone(),
        pending_text: first_text,
        pending_since: Instant::now(),
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
    shared_exit.lock().unwrap().save();
    res
}
