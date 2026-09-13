//! egui front-end: pick a device/display, tune quality, toggle audio, and watch
//! the live mirror with a draggable/zoomable crop (ideal for the Quest's small
//! floating panel — drag to pan, scroll to zoom, or snap to an eye preset).

use crate::adb::{self, Device, DisplayInfo};
use crate::config::Config;
use crate::decoder::Frame;
use crate::stream::{Status, StreamConfig, StreamHandle, ViewParams};
use eframe::egui;
use egui::{Color32, Pos2, Rect, Sense, pos2, vec2};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The "solid" Quest 3 view profile applied by the one-click preset button.
const QUEST3_CROP: [f32; 4] = [0.0, 0.0, 0.5, 1.0];
const QUEST3_K1: f32 = 0.02;
const QUEST3_K2: f32 = -0.20;
const QUEST3_TILT: f32 = -1.0;

/// Quest 2 — wider FOV fresnel lens; stronger barrel distortion.
const QUEST2_CROP: [f32; 4] = [0.0, 0.0, 0.5, 1.0];
const QUEST2_K1: f32 = 0.10;
const QUEST2_K2: f32 = -0.25;
const QUEST2_TILT: f32 = -1.0;

/// Quest 3S — fresnel lens (same type as Quest 2), slightly smaller distortion.
const QUEST3S_CROP: [f32; 4] = [0.0, 0.0, 0.5, 1.0];
const QUEST3S_K1: f32 = 0.07;
const QUEST3S_K2: f32 = -0.22;
const QUEST3S_TILT: f32 = -1.0;

/// Quest Pro — pancake lens; much flatter, barely any barrel distortion.
const QUESTPRO_CROP: [f32; 4] = [0.0, 0.0, 0.5, 1.0];
const QUESTPRO_K1: f32 = 0.01;
const QUESTPRO_K2: f32 = -0.04;
const QUESTPRO_TILT: f32 = 0.0;

/// One-click quality presets
const PRESET_LOWLAT: (u32, u32, u32) = (1440, 60, 12);
const PRESET_HIQUAL: (u32, u32, u32) = (1920, 60, 20);

#[derive(Clone, PartialEq)]
struct Settings {
    display_id: u32,
    max_size: u32,
    bitrate_mbps: u32,
    max_fps: u32,
    audio: bool,
    /// cpal output device name for the Quest's audio; empty = system default.
    audio_output_device: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            display_id: 0,
            max_size: 1920,
            bitrate_mbps: 16,
            max_fps: 60,
            audio: false,
            audio_output_device: String::new(),
        }
    }
}

type DisplayFetch = Arc<Mutex<Option<anyhow::Result<Vec<DisplayInfo>>>>>;

#[derive(Default)]
pub struct StartupConfig {
    pub serial: Option<String>,
    pub display_id: Option<u32>,
    pub max_size: Option<u32>,
    pub bitrate_mbps: Option<u32>,
    pub max_fps: Option<u32>,
    pub audio: Option<bool>,
    pub autostart: bool,
}

pub struct App {
    devices: Vec<Device>,
    selected_serial: Option<String>,
    displays: Vec<DisplayInfo>,
    display_fetch: DisplayFetch,
    settings: Settings,

    stream: Option<StreamHandle>,
    flat: Option<crate::flat::FlatHandle>,
    texture: Option<egui::TextureHandle>,
    last_generation: u64,
    tex_size: [usize; 2],
    last_frame: Option<Frame>,
    capture_msg: Option<String>,

    spout_enabled: bool,
    spout: Option<crate::spout::SpoutOutput>,
    spout_error: Option<String>,

    vcam_enabled: bool,
    vcam: Option<crate::vcam::VirtualCamOutput>,
    vcam_error: Option<String>,

    uv: Rect,
    lens_correct: bool,
    lens_k1: f32,
    lens_k2: f32,
    rotation_deg: f32,

    want_autostart: bool,
    auto_reconnect: bool,
    need_display_fetch: bool,
    fetch_inflight: bool,

    remote_input: String,
    remotes: Vec<String>,
    connect_result: Arc<Mutex<Option<(String, Result<String, String>)>>>,
    connecting: bool,

    streamer_forced: Option<bool>,
    obs_detected: bool,
    last_obs_check: Instant,

    persisted: Config,
    dirty_since: Option<Instant>,

    settings_open: bool,
}

/// Custom UI Widget: A modern Toggle Switch (replaces standard checkboxes)
fn toggle_switch(ui: &mut egui::Ui, on: &mut bool) -> egui::Response {
    let desired_size = vec2(36.0, 20.0);
    let (rect, mut response) = ui.allocate_exact_size(desired_size, egui::Sense::click());
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, ui.is_enabled(), *on, ""));

    if ui.is_rect_visible(rect) {
        let visuals = ui.style().interact_selectable(&response, *on);
        let rect = rect.expand(visuals.expansion);
        let radius = 0.5 * rect.height();
        
        // Colors matching our Purple Flat theme
        let bg_color = if *on { Color32::from_rgb(0x8b, 0x5c, 0xf6) } else { Color32::from_rgb(0x24, 0x22, 0x2e) };
        let border_color = if *on { Color32::from_rgb(0x8b, 0x5c, 0xf6) } else { Color32::from_rgb(0x31, 0x2f, 0x40) };
        
        ui.painter().rect(
            rect,
            egui::CornerRadius::same(radius as u8),
            bg_color,
            egui::Stroke::new(1.0f32, border_color),
            egui::StrokeKind::Middle,
        );
        
        let how_on = ui.ctx().animate_bool(response.id, *on);
        let circle_x = egui::lerp((rect.left() + radius + 2.0)..=(rect.right() - radius - 2.0), how_on);
        let center = pos2(circle_x, rect.center().y);
        
        ui.painter().circle(
            center,
            0.75 * radius,
            Color32::WHITE,
            egui::Stroke::NONE,
        );
    }
    response
}

fn apply_purple_theme(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    let bg_main = Color32::from_rgb(0x14, 0x13, 0x1a);
    let bg_sidebar = Color32::from_rgb(0x1a, 0x19, 0x22);
    let bg_input = Color32::from_rgb(0x24, 0x22, 0x2e);
    let border = Color32::from_rgb(0x31, 0x2f, 0x40);
    let accent = Color32::from_rgb(0x8b, 0x5c, 0xf6);
    let accent_hover = Color32::from_rgb(0x7c, 0x3a, 0xed);
    let text_main = Color32::from_rgb(0xe2, 0xe8, 0xf0);
    let text_muted = Color32::from_rgb(0x8b, 0x8a, 0x96);

    visuals.panel_fill = bg_main;
    visuals.window_fill = bg_sidebar;
    visuals.window_stroke = egui::Stroke::new(1.0f32, border);
    
    // Flat aesthetic: disable default egui shadows
    visuals.window_shadow = egui::epaint::Shadow::NONE;
    visuals.popup_shadow = egui::epaint::Shadow::NONE;
    visuals.extreme_bg_color = bg_input;

    let rounding = egui::CornerRadius::same(6);

    visuals.widgets.noninteractive.bg_fill = bg_input;
    visuals.widgets.noninteractive.weak_bg_fill = bg_input;
    visuals.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0f32, border);
    visuals.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0f32, text_muted);
    visuals.widgets.noninteractive.corner_radius = rounding;

    visuals.widgets.inactive.bg_fill = bg_input;
    visuals.widgets.inactive.weak_bg_fill = bg_input;
    visuals.widgets.inactive.bg_stroke = egui::Stroke::new(1.0f32, border);
    visuals.widgets.inactive.fg_stroke = egui::Stroke::new(1.0f32, text_main);
    visuals.widgets.inactive.corner_radius = rounding;

    visuals.widgets.hovered.bg_fill = Color32::from_rgb(0x2d, 0x2b, 0x3a);
    visuals.widgets.hovered.weak_bg_fill = Color32::from_rgb(0x2d, 0x2b, 0x3a);
    visuals.widgets.hovered.bg_stroke = egui::Stroke::new(1.0f32, accent);
    visuals.widgets.hovered.fg_stroke = egui::Stroke::new(1.0f32, Color32::WHITE);
    visuals.widgets.hovered.corner_radius = rounding;

    visuals.widgets.active.bg_fill = accent;
    visuals.widgets.active.weak_bg_fill = accent;
    visuals.widgets.active.bg_stroke = egui::Stroke::new(1.0f32, accent_hover);
    visuals.widgets.active.fg_stroke = egui::Stroke::new(1.0f32, Color32::WHITE);
    visuals.widgets.active.corner_radius = rounding;

    visuals.widgets.open.bg_fill = bg_input;
    visuals.widgets.open.weak_bg_fill = bg_input;
    visuals.widgets.open.bg_stroke = egui::Stroke::new(1.0f32, accent);
    visuals.widgets.open.fg_stroke = egui::Stroke::new(1.0f32, text_main);
    visuals.widgets.open.corner_radius = rounding;

    visuals.selection.bg_fill = accent;
    visuals.selection.stroke = egui::Stroke::new(1.0f32, Color32::WHITE);

    ctx.set_visuals(visuals);

    ctx.global_style_mut(|style| {
        style.spacing.item_spacing = vec2(8.0, 10.0);
        style.spacing.button_padding = vec2(8.0, 6.0);
    });
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, startup: StartupConfig) -> Self {
        apply_purple_theme(&cc.egui_ctx);
        install_custom_fonts(&cc.egui_ctx);
        
        let cfg = Config::load().unwrap_or_default();
        let mut settings = Settings {
            display_id: cfg.display_id,
            max_size: cfg.max_size,
            bitrate_mbps: cfg.bitrate_mbps,
            max_fps: cfg.max_fps,
            audio: cfg.audio,
            audio_output_device: cfg.audio_output_device.clone(),
        };
        if let Some(v) = startup.display_id { settings.display_id = v; }
        if let Some(v) = startup.max_size { settings.max_size = v; }
        if let Some(v) = startup.bitrate_mbps { settings.bitrate_mbps = v; }
        if let Some(v) = startup.max_fps { settings.max_fps = v; }
        if let Some(v) = startup.audio { settings.audio = v; }

        let mut app = Self {
            devices: Vec::new(),
            selected_serial: None,
            displays: Vec::new(),
            display_fetch: Arc::new(Mutex::new(None)),
            settings,
            stream: None,
            flat: None,
            texture: None,
            last_generation: 0,
            tex_size: [0, 0],
            last_frame: None,
            capture_msg: None,
            spout_enabled: false,
            spout: None,
            spout_error: None,
            vcam_enabled: false,
            vcam: None,
            vcam_error: None,
            uv: Rect::from_min_max(pos2(cfg.uv[0], cfg.uv[1]), pos2(cfg.uv[2], cfg.uv[3])),
            lens_correct: cfg.lens_correct,
            lens_k1: cfg.lens_k1,
            lens_k2: cfg.lens_k2,
            rotation_deg: cfg.rotation_deg,
            want_autostart: startup.autostart,
            auto_reconnect: true,
            need_display_fetch: false,
            fetch_inflight: false,
            remote_input: String::new(),
            remotes: cfg.remotes.clone(),
            connect_result: Arc::new(Mutex::new(None)),
            connecting: false,
            streamer_forced: None,
            obs_detected: crate::obsdetect::is_running(),
            last_obs_check: Instant::now(),
            persisted: cfg,
            dirty_since: None,
            settings_open: false,
        };
        
        app.persisted = app.current_config();
        app.refresh_devices();
        if let Some(s) = startup.serial {
            if app.devices.iter().any(|d| d.serial == s) {
                app.selected_serial = Some(s);
                app.request_display_fetch();
            }
        }
        app
    }

    fn refresh_devices(&mut self) {
        let prev = self.selected_serial.clone();
        self.devices = adb::list_devices().unwrap_or_default();
        let online: Vec<&Device> = self.devices.iter().filter(|d| d.state == "device").collect();

        let still_present = prev
            .as_ref()
            .map(|s| online.iter().any(|d| &d.serial == s))
            .unwrap_or(false);
            
        if !still_present {
            self.selected_serial = online
                .iter()
                .find(|d| d.usb)
                .or_else(|| online.first())
                .map(|d| d.serial.clone());
            self.request_display_fetch();
        }
    }

    fn request_display_fetch(&mut self) {
        self.displays.clear();
        self.need_display_fetch = true;
    }

    fn fetch_safe(&self) -> bool {
        match self.stream.as_ref().map(|s| s.status()) {
            None | Some(Status::Streaming { .. }) | Some(Status::Error(_)) | Some(Status::Stopped) => true,
            Some(Status::Connecting) => false,
        }
    }

    fn drive_display_fetch(&mut self) {
        if !self.need_display_fetch || self.fetch_inflight || !self.fetch_safe() {
            return;
        }
        let Some(serial) = self.selected_serial.clone() else {
            self.need_display_fetch = false;
            return;
        };
        self.need_display_fetch = false;
        self.fetch_inflight = true;
        let slot = self.display_fetch.clone();
        *slot.lock().unwrap() = None;
        std::thread::spawn(move || {
            let res = adb::list_displays(&serial);
            *slot.lock().unwrap() = Some(res);
        });
    }

    fn poll_displays(&mut self) {
        let taken = self.display_fetch.lock().unwrap().take();
        if let Some(res) = taken {
            self.fetch_inflight = false;
            match res {
                Ok(mut list) => {
                    list.retain(|d| d.width >= 2 && d.height >= 2);
                    list.sort_by(|a, b| (b.width * b.height).cmp(&(a.width * a.height)));
                    self.displays = list;
                    if !self.displays.iter().any(|d| d.id == self.settings.display_id) {
                        if self.displays.iter().any(|d| d.id == 0) {
                            self.settings.display_id = 0;
                        } else if let Some(first) = self.displays.first() {
                            self.settings.display_id = first.id;
                        }
                    }
                }
                Err(_) => self.displays.clear(),
            }
        }
    }

    fn connect(&mut self, ctx: &egui::Context) {
        let Some(serial) = self.selected_serial.clone() else { return };
        self.stream = None; 
        self.texture = None;
        self.last_frame = None;
        self.last_generation = 0;
        let cfg = StreamConfig {
            serial,
            display_id: self.settings.display_id,
            max_size: self.settings.max_size,
            video_bit_rate: self.settings.bitrate_mbps * 1_000_000,
            max_fps: self.settings.max_fps,
            audio: self.settings.audio,
            audio_bit_rate: 128_000,
            audio_output_device: self.settings.audio_output_device.clone(),
        };
        let ar = Arc::new(std::sync::atomic::AtomicBool::new(self.auto_reconnect));
        self.stream = Some(StreamHandle::start(cfg, ctx.clone(), ar));
    }

    fn disconnect(&mut self) {
        self.stream = None;
        self.flat = None;
        self.texture = None;
        self.last_frame = None;
    }

    fn connect_flat(&mut self, ctx: &egui::Context) {
        let Some(serial) = self.selected_serial.clone() else { return };
        self.stream = None;
        self.flat = None;
        self.texture = None;
        self.last_frame = None;
        self.last_generation = 0;
        self.lens_correct = false;
        self.rotation_deg = 0.0;
        self.uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
        let cfg = self.flat_config();
        self.flat = Some(crate::flat::FlatHandle::start(
            serial,
            cfg.max_size,
            cfg.max_size,
            cfg.bitrate,
            cfg.fps,
            self.settings.audio,
            self.settings.audio_output_device.clone(),
            ctx.clone(),
        ));
    }

    fn apply_quest3_preset(&mut self) {
        self.uv = Rect::from_min_max(pos2(QUEST3_CROP[0], QUEST3_CROP[1]), pos2(QUEST3_CROP[2], QUEST3_CROP[3]));
        self.lens_correct = true;
        self.lens_k1 = QUEST3_K1;
        self.lens_k2 = QUEST3_K2;
        self.rotation_deg = QUEST3_TILT;
    }

    fn apply_quest2_preset(&mut self) {
        self.uv = Rect::from_min_max(pos2(QUEST2_CROP[0], QUEST2_CROP[1]), pos2(QUEST2_CROP[2], QUEST2_CROP[3]));
        self.lens_correct = true;
        self.lens_k1 = QUEST2_K1;
        self.lens_k2 = QUEST2_K2;
        self.rotation_deg = QUEST2_TILT;
    }

    fn apply_questpro_preset(&mut self) {
        self.uv = Rect::from_min_max(pos2(QUESTPRO_CROP[0], QUESTPRO_CROP[1]), pos2(QUESTPRO_CROP[2], QUESTPRO_CROP[3]));
        self.lens_correct = true;
        self.lens_k1 = QUESTPRO_K1;
        self.lens_k2 = QUESTPRO_K2;
        self.rotation_deg = QUESTPRO_TILT;
    }

    fn apply_quest3s_preset(&mut self) {
        self.uv = Rect::from_min_max(pos2(QUEST3S_CROP[0], QUEST3S_CROP[1]), pos2(QUEST3S_CROP[2], QUEST3S_CROP[3]));
        self.lens_correct = true;
        self.lens_k1 = QUEST3S_K1;
        self.lens_k2 = QUEST3S_K2;
        self.rotation_deg = QUEST3S_TILT;
    }

    fn apply_quality_preset(&mut self, max_size: u32, fps: u32, mbps: u32) {
        self.settings.max_size = max_size;
        self.settings.max_fps = fps;
        self.settings.bitrate_mbps = mbps;
    }

    fn sync_auto_reconnect(&self) {
        if let Some(s) = &self.stream {
            s.auto_reconnect.store(self.auto_reconnect, std::sync::atomic::Ordering::Relaxed);
        }
    }

    fn current_config(&self) -> Config {
        Config {
            lens_correct: self.lens_correct,
            lens_k1: self.lens_k1,
            lens_k2: self.lens_k2,
            rotation_deg: self.rotation_deg,
            uv: [self.uv.min.x, self.uv.min.y, self.uv.max.x, self.uv.max.y],
            display_id: self.settings.display_id,
            max_size: self.settings.max_size,
            bitrate_mbps: self.settings.bitrate_mbps,
            max_fps: self.settings.max_fps,
            audio: self.settings.audio,
            audio_output_device: self.settings.audio_output_device.clone(),
            remotes: self.remotes.clone(),
        }
    }

    fn start_remote_connect(&mut self, addr: String, ctx: egui::Context) {
        let addr = adb::normalize_addr(&addr);
        if addr.is_empty() || self.connecting {
            return;
        }
        self.connecting = true;
        self.capture_msg = Some(format!("🔗 Connecting to {addr}…"));
        let slot = self.connect_result.clone();
        std::thread::spawn(move || {
            let res = adb::connect(&addr).map_err(|e| format!("{e:#}"));
            *slot.lock().unwrap() = Some((addr, res));
            ctx.request_repaint();
        });
    }

    fn start_connect_all(&mut self, ctx: egui::Context) {
        if self.connecting || self.remotes.is_empty() { return; }
        self.connecting = true;
        self.capture_msg = Some("🔗 Connecting saved devices…".into());
        let slot = self.connect_result.clone();
        let remotes = self.remotes.clone();
        std::thread::spawn(move || {
            let mut ok = 0usize;
            for r in &remotes {
                if adb::connect(r).is_ok() { ok += 1; }
            }
            *slot.lock().unwrap() = Some(("*".to_string(), Ok(format!("connected {ok}/{}", remotes.len()))));
            ctx.request_repaint();
        });
    }

    fn poll_connect(&mut self) {
        let taken = self.connect_result.lock().unwrap().take();
        let Some((addr, res)) = taken else { return };
        self.connecting = false;
        match res {
            Ok(msg) => {
                self.capture_msg = Some(format!("🔗 {msg}"));
                if !self.remotes.contains(&addr) {
                    self.remotes.push(addr.clone());
                }
                self.refresh_devices();
                if self.devices.iter().any(|d| d.serial == addr && d.state == "device") {
                    self.selected_serial = Some(addr);
                    self.request_display_fetch();
                }
            }
            Err(e) => self.capture_msg = Some(format!("Connect failed: {e}")),
        }
    }

    fn autosave(&mut self) {
        let cur = self.current_config();
        if cur != self.persisted {
            if self.dirty_since.is_none() { self.dirty_since = Some(Instant::now()); }
        }
        if let Some(t) = self.dirty_since {
            if t.elapsed().as_millis() >= 500 {
                let cur = self.current_config();
                cur.save();
                self.persisted = cur;
                self.dirty_since = None;
            }
        }
    }

    fn take_screenshot(&mut self) {
        let Some(frame) = &self.last_frame else { return; };
        match save_crop_png(frame, self.uv) {
            Ok(path) => self.capture_msg = Some(format!("📷 Saved {}", path.display())),
            Err(e) => self.capture_msg = Some(format!("Screenshot failed: {e}")),
        }
    }

    fn toggle_recording(&mut self) {
        let recording = self.stream.as_ref().map(|s| s.is_recording()).unwrap_or(false)
            || self.flat.as_ref().map(|f| f.is_recording()).unwrap_or(false);
        if self.stream.is_none() && self.flat.is_none() { return; }
        
        if recording {
            if let Some(s) = &self.stream { s.stop_recording(); }
            if let Some(f) = &self.flat { f.stop_recording(); }
            self.capture_msg = Some("⏹ Recording saved.".into());
        } else {
            match capture_path("clip", "mp4") {
                Ok(path) => {
                    if let Some(f) = &self.flat {
                        f.start_recording(path.clone());
                    } else if let Some(s) = &self.stream {
                        let view = ViewParams {
                            uv: [self.uv.min.x, self.uv.min.y, self.uv.max.x, self.uv.max.y],
                            lens_correct: self.lens_correct,
                            k1: self.lens_k1,
                            k2: self.lens_k2,
                            rotation_deg: self.rotation_deg,
                        };
                        s.start_recording(path.clone(), view);
                    }
                    self.capture_msg = Some(format!("⏺ Recording to {}", path.display()));
                }
                Err(e) => self.capture_msg = Some(format!("Record failed: {e}")),
            }
        }
    }

    fn selected_is_quest(&self) -> bool {
        self.selected_serial
            .as_ref()
            .and_then(|s| self.devices.iter().find(|d| &d.serial == s))
            .map(|d| {
                let m = d.model.to_lowercase();
                m.contains("quest") || m.contains("oculus") || m.contains("eureka") || m.contains("meta")
            })
            .unwrap_or(false)
    }

    fn active_config_matches(&self) -> bool {
        if let Some(f) = &self.flat {
            let want = self.flat_config();
            return f.config == want;
        }
        let Some(s) = &self.stream else { return false };
        s.config.display_id == self.settings.display_id
            && s.config.max_size == self.settings.max_size
            && s.config.video_bit_rate == self.settings.bitrate_mbps * 1_000_000
            && s.config.max_fps == self.settings.max_fps
            && s.config.audio == self.settings.audio
            && s.config.audio_output_device == self.settings.audio_output_device
    }

    fn flat_config(&self) -> crate::flat::FlatConfig {
        crate::flat::FlatConfig {
            max_size: if self.settings.max_size >= 640 { self.settings.max_size } else { 1920 },
            bitrate: self.settings.bitrate_mbps.max(2) * 1_000_000,
            fps: if self.settings.max_fps > 0 { self.settings.max_fps } else { 72 },
        }
    }

    fn sync_texture(&mut self, ctx: &egui::Context) {
        let slot_arc = if let Some(f) = &self.flat {
            f.slot.clone()
        } else if let Some(s) = &self.stream {
            s.slot.clone()
        } else {
            return;
        };
        let mut slot = slot_arc.lock().unwrap();
        if slot.generation == self.last_generation { return; }
        if let Some(frame) = slot.frame.take() {
            self.last_generation = slot.generation;
            drop(slot);
            let size = [frame.width as usize, frame.height as usize];
            let image = egui::ColorImage::from_rgba_unmultiplied(size, &frame.rgba);
            self.tex_size = size;
            match &mut self.texture {
                Some(t) => t.set(image, egui::TextureOptions::LINEAR),
                None => self.texture = Some(ctx.load_texture("mirror", image, egui::TextureOptions::LINEAR))
            }
            self.last_frame = Some(frame);
            self.push_live_outputs();
        }
    }

    fn live_view(&self, frame: &Frame) -> (ViewParams, u32, u32) {
        if self.flat.is_some() {
            (ViewParams::default(), frame.width, frame.height)
        } else {
            let view = ViewParams {
                uv: [self.uv.min.x, self.uv.min.y, self.uv.max.x, self.uv.max.y],
                lens_correct: self.lens_correct,
                k1: self.lens_k1,
                k2: self.lens_k2,
                rotation_deg: self.rotation_deg,
            };
            let (ow, oh) = crate::stream::output_dims(&view, frame.width, frame.height);
            (view, ow, oh)
        }
    }

    fn push_live_outputs(&mut self) {
        if !self.spout_enabled && !self.vcam_enabled { return; }
        let Some(frame) = &self.last_frame else { return };
        let (view, ow, oh) = self.live_view(frame);

        if self.spout_enabled {
            let bgra = crate::stream::warp_to_bgra(frame, &view, ow, oh);
            let sender = self.spout.get_or_insert_with(|| crate::spout::SpoutOutput::new("Quest scrcpy"));
            match sender.send(&bgra, ow, oh) {
                Ok(()) => self.spout_error = None,
                Err(e) => {
                    self.spout_error = Some(format!("{e:#}"));
                    self.spout_enabled = false;
                    self.spout = None;
                }
            }
        }

        if self.vcam_enabled {
            let rgba = crate::stream::warp_to_rgba(frame, &view, ow, oh);
            let cam = self.vcam.get_or_insert_with(crate::vcam::VirtualCamOutput::new);
            match cam.send(&rgba, ow, oh) {
                Ok(()) => self.vcam_error = None,
                Err(e) => {
                    self.vcam_error = Some(format!("{e:#}"));
                    self.vcam_enabled = false;
                    self.vcam = None;
                }
            }
        }
    }

    fn poll_obs_detection(&mut self) {
        if self.last_obs_check.elapsed() < Duration::from_secs(2) { return; }
        self.last_obs_check = Instant::now();
        self.obs_detected = crate::obsdetect::is_running();
    }

    fn streamer_active(&self) -> bool {
        self.streamer_forced.unwrap_or(self.obs_detected)
    }
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.want_autostart && self.selected_serial.is_some() && self.stream.is_none() {
            self.want_autostart = false;
            self.request_display_fetch();
            self.connect(ctx);
        }
        self.poll_connect();
        self.poll_displays();
        self.drive_display_fetch();
        if let Some(f) = &self.flat {
            f.set_audio_enabled(self.settings.audio);
        }
        self.sync_texture(ctx);
        self.autosave();
        self.poll_obs_detection();

        let is_fullscreen = ctx.input(|i| i.viewport().fullscreen).unwrap_or(false);
        if ctx.input(|i| i.key_pressed(egui::Key::F11)) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(!is_fullscreen));
        } else if is_fullscreen && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            // The toggle button is hidden in fullscreen (no chrome), so Esc is
            // the only other way out.
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        ui.painter().rect_filled(ui.max_rect(), 0.0, Color32::from_rgb(0x14, 0x13, 0x1a));
        let fullscreen = ui.ctx().input(|i| i.viewport().fullscreen).unwrap_or(false);
        if !fullscreen {
            self.top_bar(ui);
            self.sidebar(ui);
        }
        self.central_video(ui);
        self.settings_modal(ui.ctx());
    }
}

mod icon {
    pub const VR_HEADSET: &str = "\u{f729}";   // fa-vr-cardboard
    pub const DISPLAY: &str = "\u{f26c}";      // fa-tv
    pub const EXPAND: &str = "\u{f065}";       // fa-expand
    pub const STREAMER: &str = "\u{f144}";     // fa-circle-play
    pub const WIFI: &str = "\u{f1eb}";         // fa-wifi
    pub const BITRATE: &str = "\u{f080}";      // fa-chart-bar
    pub const FPS: &str = "\u{f625}";          // fa-gauge-high
    pub const AUDIO: &str = "\u{f028}";        // fa-volume-high
    pub const BOLT: &str = "\u{e0b7}";         // fa-bolt-lightning
    pub const GEM: &str = "\u{f3a5}";          // fa-gem
    pub const PLAY: &str = "\u{f04b}";         // fa-play
    pub const REFRESH: &str = "\u{f2f9}";      // fa-rotate-right
    pub const LINK: &str = "\u{f0c1}";         // fa-link
    pub const GEAR: &str = "\u{f013}";         // fa-gear
    pub const CAMERA: &str = "\u{f030}";       // fa-camera
    pub const VIDEO: &str = "\u{f03d}";        // fa-video
    pub const STOP: &str = "\u{f04d}";         // fa-stop
    pub const XMARK: &str = "\u{f00d}";        // fa-xmark
    pub const COMPRESS: &str = "\u{f066}";     // fa-compress
}

fn status_dot(ui: &mut egui::Ui, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(vec2(8.0, 8.0), Sense::hover());
    ui.painter().circle_filled(rect.center(), 3.5, color);
}

fn combo_chevron_icon(ui: &egui::Ui, rect: Rect, visuals: &egui::style::WidgetVisuals, is_open: bool) {
    let painter = ui.painter();
    let center = rect.center();
    let color = visuals.fg_stroke.color;
    let stroke = egui::Stroke::new(1.5_f32, color);
    let h = 2.5;
    let w = 4.0;
    if is_open {
        painter.line_segment([center + vec2(-w, h * 0.5), center + vec2(0.0, -h * 0.5)], stroke);
        painter.line_segment([center + vec2(0.0, -h * 0.5), center + vec2(w, h * 0.5)], stroke);
    } else {
        painter.line_segment([center + vec2(-w, -h * 0.5), center + vec2(0.0, h * 0.5)], stroke);
        painter.line_segment([center + vec2(0.0, h * 0.5), center + vec2(w, -h * 0.5)], stroke);
    }
}

fn custom_combobox(id_salt: impl std::hash::Hash) -> egui::ComboBox {
    egui::ComboBox::from_id_salt(id_salt).icon(combo_chevron_icon)
}

fn col_header(ui: &mut egui::Ui, icon_char: &str, text: &str) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        ui.label(
            egui::RichText::new(icon_char)
                .size(10.5)
                .color(Color32::from_rgb(0x8b, 0x8a, 0x96)),
        );
        ui.label(
            egui::RichText::new(text)
                .size(10.5)
                .strong()
                .color(Color32::from_rgb(0x8b, 0x8a, 0x96)),
        );
    });
}

fn preset_button(ui: &mut egui::Ui, size: [f32; 2], is_active: bool, icon_char: &str, text: &str) -> egui::Response {
    let (bg, border, text_color) = if is_active {
        (
            Color32::from_rgb(0x1c, 0x5e, 0x89), // Ocean Blue from reference
            Color32::from_rgb(0x38, 0xbd, 0xf8), // Cyan border
            Color32::WHITE,
        )
    } else {
        (
            Color32::from_rgb(0x1e, 0x1d, 0x29),
            Color32::from_rgb(0x31, 0x2f, 0x40),
            Color32::from_rgb(0xe2, 0xe8, 0xf0),
        )
    };

    let rich = egui::RichText::new(format!("{icon_char}  {text}"))
        .size(12.5)
        .strong()
        .color(text_color);

    let btn = egui::Button::new(rich)
        .fill(bg)
        .stroke(egui::Stroke::new(1.0f32, border))
        .corner_radius(egui::CornerRadius::same(6))
        .truncate();

    ui.add_sized(size, btn)
}

impl App {
    fn top_bar(&mut self, ui: &mut egui::Ui) {
        let frame = egui::Frame::default()
            .fill(Color32::from_rgb(0x0f, 0x0e, 0x13)) // bg-header
            .inner_margin(egui::Margin::symmetric(20, 14))
            .stroke(egui::Stroke::new(1.0f32, Color32::from_rgb(0x31, 0x2f, 0x40)));

        egui::Panel::top("header")
            .frame(frame)
            .exact_size(60.0)
            .show_inside(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    // Left: Logo & Title
                    ui.label(
                        egui::RichText::new("🥽")
                            .size(22.0)
                            .color(Color32::from_rgb(0xe2, 0xe8, 0xf0)),
                    );
                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new("QUEST STREAMER")
                            .strong()
                            .size(13.0)
                            .color(Color32::from_rgb(0xe2, 0xe8, 0xf0)),
                    );

                    // Right side: Status & Settings Gear
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let gear_color = if self.settings_open {
                            Color32::from_rgb(0xe2, 0xe8, 0xf0)
                        } else {
                            Color32::from_rgb(0x8b, 0x8a, 0x96)
                        };
                        let gear_btn = egui::Button::new(
                            egui::RichText::new(icon::GEAR).size(16.0).color(gear_color)
                        ).fill(Color32::TRANSPARENT).stroke(egui::Stroke::NONE);

                        if ui.add(gear_btn).on_hover_text("Configurações Avançadas").clicked() {
                            self.settings_open = !self.settings_open;
                        }

                        ui.add_space(8.0);

                        let is_fullscreen =
                            ui.ctx().input(|i| i.viewport().fullscreen).unwrap_or(false);
                        let (fs_icon, fs_hover) = if is_fullscreen {
                            (icon::COMPRESS, "Sair da tela cheia (F11)")
                        } else {
                            (icon::EXPAND, "Tela cheia (F11)")
                        };
                        let fs_btn = egui::Button::new(
                            egui::RichText::new(fs_icon)
                                .size(16.0)
                                .color(Color32::from_rgb(0x8b, 0x8a, 0x96)),
                        )
                        .fill(Color32::TRANSPARENT)
                        .stroke(egui::Stroke::NONE);
                        if ui.add(fs_btn).on_hover_text(fs_hover).clicked() {
                            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Fullscreen(!is_fullscreen));
                        }

                        ui.add_space(20.0);
                        self.status_header_badge(ui);
                    });
                });
            });
    }

    fn status_header_badge(&self, ui: &mut egui::Ui) {
        let status = if let Some(f) = &self.flat {
            f.status()
        } else if let Some(s) = &self.stream {
            s.status()
        } else {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                status_dot(ui, Color32::from_rgb(0xef, 0x44, 0x44));
                ui.label(egui::RichText::new("Desconectado").size(13.0).color(Color32::from_rgb(0x8b, 0x8a, 0x96)));
            });
            return;
        };

        match status {
            Status::Connecting => {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    ui.spinner();
                    ui.label(egui::RichText::new("Conectando…").size(13.0).color(Color32::from_rgb(0x8b, 0x5c, 0xf6)));
                });
            }
            Status::Streaming { width, height, fps, decode_ms } => {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    status_dot(ui, Color32::from_rgb(0x22, 0xc5, 0x5e));
                    let label = if decode_ms > 0.1 {
                        format!("{width}×{height}  ·  {fps:.0} fps  ·  {decode_ms:.1} ms")
                    } else {
                        format!("{width}×{height}  ·  {fps:.0} fps")
                    };
                    ui.label(egui::RichText::new(label).size(13.0).color(Color32::from_rgb(0xe2, 0xe8, 0xf0)));
                });
            }
            Status::Error(e) => {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    ui.colored_label(Color32::from_rgb(0xef, 0x44, 0x44), "⚠️");
                    ui.label(egui::RichText::new(e).size(13.0).color(Color32::from_rgb(0xef, 0x44, 0x44)));
                });
            }
            Status::Stopped => {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    status_dot(ui, Color32::from_rgb(0x8b, 0x8a, 0x96));
                    ui.label(egui::RichText::new("Parado").size(13.0).color(Color32::from_rgb(0x8b, 0x8a, 0x96)));
                });
            }
        }
    }

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        let stroke_w = 1.0f32;
        let margin_x = 20.0f32;
        let sidebar_w = 300.0f32;
        let inner_w = sidebar_w - margin_x * 2.0 - stroke_w * 2.0; // 258.0

        let frame = egui::Frame::default()
            .fill(Color32::from_rgb(0x1a, 0x19, 0x22))
            .inner_margin(egui::Margin::symmetric(margin_x as i8, 24))
            .stroke(egui::Stroke::new(stroke_w, Color32::from_rgb(0x31, 0x2f, 0x40)));

        egui::Panel::left("sidebar")
            .frame(frame)
            .resizable(false)
            .exact_size(sidebar_w)
            .show_inside(ui, |ui| {
                ui.set_width(inner_w);
                ui.set_max_width(inner_w);
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.set_width(inner_w);
                        ui.set_max_width(inner_w);
                        self.sidebar_content(ui, inner_w);
                    });
            });
    }

    fn section_header(&self, ui: &mut egui::Ui, icon_char: &str, text: &str) {
        ui.add_space(14.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 7.0;
            if !icon_char.is_empty() {
                ui.label(
                    egui::RichText::new(icon_char)
                        .size(11.0)
                        .color(Color32::from_rgb(0x8b, 0x8a, 0x96)),
                );
            }
            ui.label(
                egui::RichText::new(text)
                    .size(11.0)
                    .strong()
                    .color(Color32::from_rgb(0x8b, 0x8a, 0x96)),
            );
        });
        ui.add_space(4.0);
    }

    fn sidebar_content(&mut self, ui: &mut egui::Ui, inner_w: f32) {
        let ctx = ui.ctx().clone();
        let spacing_x = ui.spacing().item_spacing.x;
        let half_w = ((inner_w - spacing_x) * 0.5).max(40.0);

        // 1. Dispositivo
        self.section_header(ui, icon::VR_HEADSET, "DISPOSITIVO");
        let dev_label = self
            .selected_serial
            .as_ref()
            .and_then(|s| self.devices.iter().find(|d| &d.serial == s))
            .map(|d| d.label_masked(self.streamer_active()))
            .unwrap_or_else(|| "No device".into());

        let mut changed_device = false;
        let btn_size = 32.0;
        let combo_w = (inner_w - btn_size - spacing_x).max(60.0);
        ui.horizontal(|ui| {
            ui.allocate_ui_with_layout(vec2(combo_w, btn_size), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                ui.set_max_width(combo_w);
                let _r = custom_combobox("device")
                    .selected_text(egui::RichText::new(&dev_label).size(13.0))
                    .truncate()
                    .width(combo_w)
                    .show_ui(ui, |ui| {
                        let mask = self.streamer_active();
                        for d in self.devices.iter().filter(|d| d.state == "device") {
                            if ui.selectable_label(
                                    self.selected_serial.as_deref() == Some(&d.serial),
                                    d.label_masked(mask),
                                ).clicked()
                            {
                                self.selected_serial = Some(d.serial.clone());
                                changed_device = true;
                            }
                        }
                    });
            });

            let b = ui.add_sized([btn_size, btn_size], egui::Button::new(icon::REFRESH)).on_hover_text("Atualizar Lista");
            if b.clicked() {
                self.refresh_devices();
            }
        });
        if changed_device { self.request_display_fetch(); }

        // 2. Display & Max Size
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.allocate_ui_with_layout(vec2(half_w, 48.0), egui::Layout::top_down(egui::Align::Min), |ui| {
                ui.set_max_width(half_w);
                col_header(ui, icon::DISPLAY, "DISPLAY");
                let disp_label = self.displays.iter().find(|d| d.id == self.settings.display_id)
                    .map(|d| format!("{} × {}", d.width, d.height))
                    .unwrap_or_else(|| format!("Disp {}", self.settings.display_id));
                custom_combobox("display")
                    .selected_text(egui::RichText::new(disp_label).size(12.0))
                    .truncate()
                    .width(half_w)
                    .show_ui(ui, |ui| {
                        if self.displays.is_empty() { ui.label("(querying…)"); }
                        for d in &self.displays {
                            ui.selectable_value(&mut self.settings.display_id, d.id, format!("{} × {}", d.width, d.height));
                        }
                    });
            });
            ui.allocate_ui_with_layout(vec2(half_w, 48.0), egui::Layout::top_down(egui::Align::Min), |ui| {
                ui.set_max_width(half_w);
                col_header(ui, icon::EXPAND, "MAX SIZE");
                let size_label = if self.settings.max_size == 0 { "Full".to_string() } else { format!("{}px", self.settings.max_size) };
                custom_combobox("maxsize")
                    .selected_text(egui::RichText::new(size_label).size(12.0))
                    .truncate()
                    .width(half_w)
                    .show_ui(ui, |ui| {
                        for (val, txt) in [(0u32, "Full panel"), (1280, "1280px"), (1440, "1440px"), (1600, "1600px"), (1920, "1920px"), (2560, "2560px"), (3840, "3840px")] {
                            ui.selectable_value(&mut self.settings.max_size, val, txt);
                        }
                    });
            });
        });

        // 3. Streamer
        self.section_header(ui, icon::STREAMER, "STREAMER");
        let streamer_label = match self.streamer_forced {
            Some(true) => "On",
            Some(false) => "Off",
            None => "Auto",
        };
        custom_combobox("streamer")
            .selected_text(egui::RichText::new(streamer_label).size(13.0))
            .truncate()
            .width(inner_w)
            .show_ui(ui, |ui| {
                let auto_txt = if self.obs_detected { "Auto (OBS detectado)" } else { "Auto (baseado no OBS)" };
                ui.selectable_value(&mut self.streamer_forced, None, auto_txt);
                ui.selectable_value(&mut self.streamer_forced, Some(true), "On (Ocultar serials/IPs)");
                ui.selectable_value(&mut self.streamer_forced, Some(false), "Off (Mostrar serials/IPs)");
            });

        if self.stream.is_some() || self.flat.is_some() {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let spout_label = if self.spout_enabled { "OBS (Spout): on" } else { "OBS (Spout)" };
                if ui.add_sized([half_w, 32.0], egui::Button::selectable(self.spout_enabled, spout_label).truncate()).clicked() {
                    self.spout_enabled = !self.spout_enabled;
                    if !self.spout_enabled { self.spout = None; }
                    self.spout_error = None;
                }
                let vcam_label = if self.vcam_enabled { "VCam: on" } else { "VCam" };
                if ui.add_sized([half_w, 32.0], egui::Button::selectable(self.vcam_enabled, vcam_label).truncate()).clicked() {
                    self.vcam_enabled = !self.vcam_enabled;
                    if !self.vcam_enabled { self.vcam = None; }
                    self.vcam_error = None;
                }
            });
        }

        // 4. Rede
        self.section_header(ui, icon::WIFI, "REDE (WI-FI)");
        ui.horizontal(|ui| {
            let input_btn_w = 32.0;
            let input_w = (inner_w - input_btn_w - spacing_x).max(60.0);
            let resp = ui.add_sized(
                [input_w, 32.0],
                egui::TextEdit::singleline(&mut self.remote_input).hint_text("192.168.1.x[:5555]"),
            );
            let entered = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            let b = ui.add_sized([input_btn_w, 32.0], egui::Button::new(icon::LINK));
            if (b.clicked() || entered) && !self.remote_input.trim().is_empty() {
                let addr = self.remote_input.trim().to_string();
                self.remote_input.clear();
                self.start_remote_connect(addr, ctx.clone());
            }
        });
        
        if self.connecting {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(egui::RichText::new("Conectando…").size(12.0).color(Color32::from_rgb(0x8b, 0x5c, 0xf6)));
            });
        }

        if !self.remotes.is_empty() {
            ui.add_space(4.0);
            if self.remotes.len() > 1 {
                if ui.add_sized([inner_w, 28.0], egui::Button::new(format!("{} Connect all", icon::LINK))).clicked() {
                    self.start_connect_all(ctx.clone());
                }
                ui.add_space(2.0);
            }
            let mask = self.streamer_active();
            let mut connect_one = None;
            let mut forget = None;
            let del_btn_w = 32.0;
            let rem_btn_w = (inner_w - del_btn_w - spacing_x).max(60.0);
            for r in self.remotes.clone() {
                let shown = if mask { "•••".to_string() } else { r.clone() };
                ui.horizontal(|ui| {
                    let b1 = ui.add_sized([rem_btn_w, 28.0], egui::Button::new(format!("{} {shown}", icon::LINK)).truncate());
                    if b1.clicked() {
                        connect_one = Some(r.clone());
                    }
                    let b2 = ui.add_sized([del_btn_w, 28.0], egui::Button::new(icon::XMARK));
                    if b2.clicked() { forget = Some(r.clone()); }
                });
            }
            if let Some(r) = connect_one { self.start_remote_connect(r, ctx.clone()); }
            if let Some(r) = forget {
                adb::disconnect(&r);
                self.remotes.retain(|x| x != &r);
            }
        }

        // 5. Performance (Bitrate & FPS)
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.allocate_ui_with_layout(vec2(half_w, 48.0), egui::Layout::top_down(egui::Align::Min), |ui| {
                ui.set_max_width(half_w);
                col_header(ui, icon::BITRATE, "BITRATE");
                let bitrate_label = format!("{} Mbps", self.settings.bitrate_mbps);
                custom_combobox("bitrate")
                    .selected_text(egui::RichText::new(bitrate_label).size(12.0))
                    .truncate()
                    .width(half_w)
                    .show_ui(ui, |ui| {
                        for val in [8u32, 12, 16, 20, 24, 30, 40, 50, 60, 80] {
                            ui.selectable_value(&mut self.settings.bitrate_mbps, val, format!("{val} Mbps"));
                        }
                    });
            });
            ui.allocate_ui_with_layout(vec2(half_w, 48.0), egui::Layout::top_down(egui::Align::Min), |ui| {
                ui.set_max_width(half_w);
                col_header(ui, icon::FPS, "FPS");
                let fps_label = if self.settings.max_fps == 0 { "Max".to_string() } else { self.settings.max_fps.to_string() };
                custom_combobox("fps")
                    .selected_text(egui::RichText::new(fps_label).size(12.0))
                    .truncate()
                    .width(half_w)
                    .show_ui(ui, |ui| {
                        for (val, txt) in [(0u32, "Max"), (30, "30"), (60, "60"), (72, "72"), (90, "90"), (120, "120")] {
                            ui.selectable_value(&mut self.settings.max_fps, val, txt);
                        }
                    });
            });
        });

        // 6. Áudio
        ui.add_space(16.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 7.0;
            ui.label(egui::RichText::new(icon::AUDIO).size(11.0).color(Color32::from_rgb(0x8b, 0x8a, 0x96)));
            ui.label(egui::RichText::new("ÁUDIO").size(11.0).strong().color(Color32::from_rgb(0x8b, 0x8a, 0x96)));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                toggle_switch(ui, &mut self.settings.audio);
            });
        });

        // Output device: route the Quest's audio elsewhere (e.g. away from
        // your headphones) — Discord's "share sound" / OBS's Application
        // Audio Capture are process-based and still hear it either way.
        ui.add_space(6.0);
        let device_label = if self.settings.audio_output_device.is_empty() {
            "Padrão do sistema".to_string()
        } else {
            self.settings.audio_output_device.clone()
        };
        custom_combobox("audio_output_device")
            .selected_text(egui::RichText::new(device_label).size(12.0))
            .truncate()
            .width(inner_w)
            .show_ui(ui, |ui| {
                ui.selectable_value(
                    &mut self.settings.audio_output_device,
                    String::new(),
                    "Padrão do sistema",
                );
                for name in crate::audioplay::output_device_names() {
                    ui.selectable_value(&mut self.settings.audio_output_device, name.clone(), name);
                }
            })
            .response
            .on_hover_text(
                "Pra que o Discord/OBS continuem ouvindo o áudio do Quest mesmo sem \
                 ele sair no seu fone: escolha aqui uma saída que você não está \
                 ouvindo (ex: a saída HDMI de um monitor sem caixa de som). Precisa \
                 reconectar pra aplicar.",
            );

        // 7. Quality Presets
        ui.add_space(16.0);
        ui.separator();
        ui.add_space(12.0);
        ui.label(egui::RichText::new("QUALITY PRESET").size(11.0).strong().color(Color32::from_rgb(0x8b, 0x8a, 0x96)));
        ui.add_space(6.0);
        
        ui.horizontal(|ui| {
            let (ll_size, ll_fps, ll_mbps) = PRESET_LOWLAT;
            let is_lowlat = self.settings.max_size == ll_size && self.settings.max_fps == ll_fps && self.settings.bitrate_mbps == ll_mbps;
            let (hq_size, hq_fps, hq_mbps) = PRESET_HIQUAL;
            let is_hiqual = self.settings.max_size == hq_size && self.settings.max_fps == hq_fps && self.settings.bitrate_mbps == hq_mbps;

            if preset_button(ui, [half_w, 32.0], is_lowlat, icon::BOLT, "Low-Latency").clicked() {
                self.apply_quality_preset(ll_size, ll_fps, ll_mbps);
            }
            if preset_button(ui, [half_w, 32.0], is_hiqual, icon::GEM, "High-Quality").clicked() {
                self.apply_quality_preset(hq_size, hq_fps, hq_mbps);
            }
        });

        // 8. Botões de Ação Principais
        ui.add_space(18.0);

        let streaming = self.stream.is_some() || self.flat.is_some();

        if !streaming {
            let enabled = self.selected_serial.is_some();
            let quest = self.selected_is_quest();

            let connect_btn = egui::Button::new(
                egui::RichText::new(format!("{}  Connect", icon::PLAY))
                    .strong()
                    .size(13.5)
                    .color(Color32::WHITE),
            )
            .fill(Color32::from_rgb(0x8b, 0x5c, 0xf6))
            .corner_radius(egui::CornerRadius::same(8));

            if ui.add_sized([inner_w, 42.0], connect_btn).clicked() && enabled {
                if quest { self.connect_flat(&ctx); } else { self.connect(&ctx); }
            }

            if quest {
                ui.add_space(8.0);
                let panel_btn = egui::Button::new(
                    egui::RichText::new(format!("{}  Panel view", icon::PLAY))
                        .size(13.0)
                        .strong()
                        .color(Color32::from_rgb(0xe2, 0xe8, 0xf0)),
                )
                .fill(Color32::from_rgb(0x20, 0x1f, 0x2b))
                .stroke(egui::Stroke::new(1.0f32, Color32::from_rgb(0x31, 0x2f, 0x40)))
                .corner_radius(egui::CornerRadius::same(8));

                if ui.add_sized([inner_w, 38.0], panel_btn).clicked() && enabled {
                    self.connect(&ctx);
                }
            }
        } else {
            let disconnect_btn = egui::Button::new(
                egui::RichText::new(format!("{}  Disconnect", icon::STOP))
                    .strong()
                    .size(13.5)
                    .color(Color32::WHITE),
            )
            .fill(Color32::from_rgb(0xdc, 0x26, 0x26))
            .corner_radius(egui::CornerRadius::same(8));

            if ui.add_sized([inner_w, 42.0], disconnect_btn).clicked() {
                self.disconnect();
            }

            if !self.active_config_matches() {
                ui.add_space(8.0);
                let apply_btn = egui::Button::new(
                    egui::RichText::new(format!("{}  Apply settings", icon::REFRESH))
                        .size(13.0)
                        .color(Color32::WHITE),
                )
                .fill(Color32::from_rgb(0x3b, 0x82, 0xf6))
                .corner_radius(egui::CornerRadius::same(8));

                if ui.add_sized([inner_w, 36.0], apply_btn).clicked() {
                    if self.flat.is_some() { self.connect_flat(&ctx); } else { self.connect(&ctx); }
                }
            }

            ui.add_space(10.0);
            ui.horizontal(|ui| {
                let can_shoot = self.texture.is_some();
                if ui.add_sized([(inner_w - 8.0) * 0.5, 36.0], egui::Button::new(format!("{}  Screenshot", icon::CAMERA))).clicked() && can_shoot {
                    self.take_screenshot();
                }

                let recording = self.stream.as_ref().map(|s| s.is_recording()).unwrap_or(false)
                    || self.flat.as_ref().map(|f| f.is_recording()).unwrap_or(false);
                let (label, color) = if recording { (format!("{}  Stop", icon::STOP), Color32::from_rgb(0xff, 0x6b, 0x6b)) } else { (format!("{}  Record", icon::VIDEO), Color32::from_rgb(0xff, 0xc4, 0x4c)) };
                if ui.add_sized([(inner_w - 8.0) * 0.5, 36.0], egui::Button::new(egui::RichText::new(label).color(color))).clicked() {
                    self.toggle_recording();
                }
            });
        }
        
        if let Some(msg) = &self.capture_msg {
            ui.add_space(8.0);
            ui.label(egui::RichText::new(msg).size(12.0).color(Color32::from_rgb(0x8b, 0x8a, 0x96)));
        }
    }

    fn central_video(&mut self, ui: &mut egui::Ui) {
        let frame = egui::Frame::default()
            .fill(Color32::from_rgb(0x14, 0x13, 0x1a))
            .inner_margin(egui::Margin::ZERO);
        egui::CentralPanel::default().frame(frame).show_inside(ui, |ui| {
            let Some(texture) = self.texture.clone() else {
                let area = ui.available_rect_before_wrap();
                let painter = ui.painter_at(area);
                painter.rect_filled(area, 0.0, Color32::from_rgb(0x14, 0x13, 0x1a));

            ui.vertical_centered(|ui| {
                let avail_h = ui.available_height();
                let content_h = 320.0;
                if avail_h > content_h { ui.add_space((avail_h - content_h) * 0.4); }

                let (headset_rect, _) = ui.allocate_exact_size(vec2(200.0, 110.0), Sense::hover());
                draw_headset_graphic(&ui.painter(), headset_rect.center());
                ui.add_space(28.0);

                let msg = if self.stream.is_some() || self.flat.is_some() {
                    "Aguardando primeiro frame…"
                } else {
                    "Pick your Quest and hit Connect."
                };
                ui.label(egui::RichText::new(msg).size(22.0).strong().color(Color32::from_rgb(0xe2, 0xe8, 0xf0)));
                ui.add_space(8.0);
                ui.label(egui::RichText::new("Aguardando seleção de dispositivo para iniciar o stream.").size(14.0).color(Color32::from_rgb(0x8b, 0x8a, 0x96)));
                ui.add_space(24.0);

                let streaming = self.stream.is_some() || self.flat.is_some();
                let enabled = self.selected_serial.is_some() && !streaming;
                let quest = self.selected_is_quest();
                
                let btn = egui::Button::new(
                    egui::RichText::new(format!("{}  Connect", icon::PLAY))
                        .strong()
                        .size(14.0)
                        .color(Color32::WHITE),
                ).fill(Color32::from_rgb(0x8b, 0x5c, 0xf6)).corner_radius(egui::CornerRadius::same(6));

                if ui.add_sized([160.0, 44.0], btn).clicked() && enabled {
                    let ctx = ui.ctx().clone();
                    if quest { self.connect_flat(&ctx); } else { self.connect(&ctx); }
                }
            });
            return;
        };

        let area = ui.available_rect_before_wrap();
        let response = ui.allocate_rect(area, Sense::click_and_drag());
        let painter = ui.painter_at(area);

        let tex_w = self.tex_size[0] as f32;
        let tex_h = self.tex_size[1] as f32;
        let crop_w_px = self.uv.width() * tex_w;
        let crop_h_px = self.uv.height() * tex_h;
        let crop_aspect = (crop_w_px / crop_h_px).max(1e-3);
        let area_aspect = area.width() / area.height().max(1.0);

        let draw_size = if area_aspect > crop_aspect {
            vec2(area.height() * crop_aspect, area.height())
        } else {
            vec2(area.width(), area.width() / crop_aspect)
        };
        let draw_rect = Rect::from_center_size(area.center(), draw_size);

        painter.rect_filled(area, 0.0, Color32::from_rgb(0x14, 0x13, 0x1a));
        if self.lens_correct || self.rotation_deg.abs() > 0.01 {
            let (k1, k2) = if self.lens_correct { (self.lens_k1, self.lens_k2) } else { (0.0, 0.0) };
            draw_warped(&painter, texture.id(), draw_rect, self.uv, k1, k2, self.rotation_deg);
        } else {
            painter.image(texture.id(), draw_rect, self.uv, Color32::WHITE);
        }

        if response.dragged() {
            let d = response.drag_delta();
            let uv_dx = -d.x / draw_size.x * self.uv.width();
            let uv_dy = -d.y / draw_size.y * self.uv.height();
            self.uv = clamp_uv(self.uv.translate(vec2(uv_dx, uv_dy)));
        }

        if response.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll.abs() > 0.1 {
                if let Some(pos) = response.hover_pos() {
                    let factor = (1.0 - scroll * 0.0015).clamp(0.5, 1.5);
                    let cursor_uv = screen_to_uv(pos, draw_rect, self.uv);
                    self.uv = clamp_uv(scale_about(self.uv, cursor_uv, factor));
                }
            }
        }
        });
    }

    fn settings_modal(&mut self, ctx: &egui::Context) {
        if !self.settings_open { return; }
        
        let mut close_requested = false;

        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            close_requested = true;
        }

        let screen_rect = ctx.content_rect();
        egui::Area::new(egui::Id::new("settings_backdrop_area"))
            .order(egui::Order::Middle)
            .fixed_pos(screen_rect.min)
            .show(ctx, |ui| {
                let (rect, resp) = ui.allocate_exact_size(screen_rect.size(), egui::Sense::click());
                ui.painter().rect_filled(rect, 0.0, Color32::from_black_alpha(160));
                if resp.clicked() {
                    close_requested = true;
                }
            });

        let frame = egui::Frame::window(&ctx.global_style())
            .fill(Color32::from_rgb(0x1a, 0x19, 0x22))
            .stroke(egui::Stroke::new(1.0f32, Color32::from_rgb(0x31, 0x2f, 0x40)))
            .inner_margin(egui::Margin::same(24));

        egui::Window::new("Advanced Panel View Settings")
            .title_bar(false) // Custom title bar for flat look
            .frame(frame)
            .collapsible(false)
            .resizable(false)
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .default_width(560.0)
            .show(ctx, |ui| {
                // Custom Header
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(format!("{}  Advanced Panel View Settings", icon::GEAR))
                            .size(16.0)
                            .strong()
                            .color(Color32::from_rgb(0xe2, 0xe8, 0xf0)),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let close_btn = egui::Button::new(
                            egui::RichText::new(icon::XMARK)
                                .size(14.0)
                                .color(Color32::from_rgb(0x8b, 0x8a, 0x96)),
                        )
                        .fill(Color32::TRANSPARENT)
                        .stroke(egui::Stroke::NONE);

                        if ui.add(close_btn).on_hover_text("Fechar (Esc)").clicked() {
                            close_requested = true;
                        }
                    });
                });
                
                ui.add_space(16.0);
                ui.separator();
                ui.add_space(16.0);

                // Section 1: Presets
                ui.label(egui::RichText::new("LENS CALIBRATION PRESETS").size(11.0).strong().color(Color32::from_rgb(0x8b, 0x8a, 0x96)));
                ui.add_space(6.0);
                let avail_w = ui.available_width();
                let btn_w = (avail_w - 3.0 * 8.0) * 0.25;

                ui.horizontal(|ui| {
                    if ui.add_sized([btn_w, 36.0], egui::Button::new("Quest 3")).clicked() { self.apply_quest3_preset(); }
                    if ui.add_sized([btn_w, 36.0], egui::Button::new("Quest 3S")).clicked() { self.apply_quest3s_preset(); }
                    if ui.add_sized([btn_w, 36.0], egui::Button::new("Quest 2")).clicked() { self.apply_quest2_preset(); }
                    if ui.add_sized([btn_w, 36.0], egui::Button::new("Pro")).clicked() { self.apply_questpro_preset(); }
                });

                ui.add_space(16.0);
                ui.separator();
                ui.add_space(16.0);

                // Section 2: Crop
                ui.label(egui::RichText::new("CROP (CORTE DA LENTE)").size(11.0).strong().color(Color32::from_rgb(0x8b, 0x8a, 0x96)));
                ui.add_space(6.0);

                let is_left = (self.uv.min.x - 0.0).abs() < 1e-3 && (self.uv.max.x - 0.5).abs() < 1e-3;
                let is_right = (self.uv.min.x - 0.5).abs() < 1e-3 && (self.uv.max.x - 1.0).abs() < 1e-3;
                let is_full = (self.uv.min.x - 0.0).abs() < 1e-3 && (self.uv.max.x - 1.0).abs() < 1e-3;
                let is_center = (self.uv.min.x - 0.25).abs() < 1e-3 && (self.uv.max.x - 0.75).abs() < 1e-3;

                ui.horizontal(|ui| {
                    if ui.add_sized([btn_w, 36.0], egui::Button::selectable(is_left, "Left eye")).clicked() {
                        self.uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(0.5, 1.0));
                    }
                    if ui.add_sized([btn_w, 36.0], egui::Button::selectable(is_right, "Right eye")).clicked() {
                        self.uv = Rect::from_min_max(pos2(0.5, 0.0), pos2(1.0, 1.0));
                    }
                    if ui.add_sized([btn_w, 36.0], egui::Button::selectable(is_full, "Full panel")).clicked() {
                        self.uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
                    }
                    if ui.add_sized([btn_w, 36.0], egui::Button::selectable(is_center, "Center 50%")).clicked() {
                        self.uv = Rect::from_min_max(pos2(0.25, 0.25), pos2(0.75, 0.75));
                    }
                });

                ui.add_space(16.0);
                ui.separator();
                ui.add_space(16.0);

                // Section 3: Lens Distortion
                ui.columns(2, |cols| {
                    cols[0].checkbox(&mut self.lens_correct, "Flatten lens");
                    cols[0].add_space(8.0);
                    cols[0].add_enabled_ui(self.lens_correct, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new("Curve").color(Color32::from_rgb(0x8b, 0x8a, 0x96)));
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                ui.monospace(format!("{:.2}", self.lens_k1));
                            });
                        });
                        ui.add(egui::Slider::new(&mut self.lens_k1, -0.8..=0.8).show_value(false));

                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new("Edge").color(Color32::from_rgb(0x8b, 0x8a, 0x96)));
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                ui.monospace(format!("{:.2}", self.lens_k2));
                            });
                        });
                        ui.add(egui::Slider::new(&mut self.lens_k2, -0.4..=0.4).show_value(false));
                    });

                    cols[1].horizontal(|ui| {
                        ui.label(egui::RichText::new("Tilt").color(Color32::from_rgb(0x8b, 0x8a, 0x96)));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.monospace(format!("{:.1}°", self.rotation_deg));
                        });
                    });
                    cols[1].add(egui::Slider::new(&mut self.rotation_deg, -45.0..=45.0).show_value(false));

                    cols[1].add_space(8.0);
                    let mut zoom = 1.0 / self.uv.width().max(1e-3);
                    cols[1].horizontal(|ui| {
                        ui.label(egui::RichText::new("Zoom").color(Color32::from_rgb(0x8b, 0x8a, 0x96)));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.monospace(format!("{:.2}x", zoom));
                        });
                    });
                    if cols[1].add(egui::Slider::new(&mut zoom, 1.0..=8.0).show_value(false)).changed() {
                        let center = self.uv.center();
                        let half = (0.5 / zoom).min(0.5);
                        self.uv = clamp_uv(Rect::from_center_size(center, vec2(half * 2.0, half * 2.0)));
                    }

                    cols[1].add_space(8.0);
                    cols[1].label(egui::RichText::new("🖱 Drag to pan, scroll to zoom no painel principal").size(11.0).color(Color32::from_rgb(0x8b, 0x8a, 0x96)));
                });

                ui.add_space(16.0);
                ui.separator();
                ui.add_space(12.0);

                // Section 4: Opções Gerais
                ui.label(egui::RichText::new("OPÇÕES GERAIS").size(11.0).strong().color(Color32::from_rgb(0x8b, 0x8a, 0x96)));
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Auto-reconnect (reconectar automaticamente)").color(Color32::from_rgb(0xe2, 0xe8, 0xf0)));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if toggle_switch(ui, &mut self.auto_reconnect).changed() {
                            self.sync_auto_reconnect();
                        }
                    });
                });

                ui.add_space(20.0);

                // Footer
                ui.horizontal(|ui| {
                    if ui.button("⟳ Reset Default").clicked() {
                        self.lens_k1 = QUEST3_K1;
                        self.lens_k2 = QUEST3_K2;
                        self.rotation_deg = QUEST3_TILT;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let close_btn = egui::Button::new(
                            egui::RichText::new("Apply & Close")
                                .strong()
                                .color(Color32::WHITE),
                        )
                        .fill(Color32::from_rgb(0x8b, 0x5c, 0xf6))
                        .corner_radius(egui::CornerRadius::same(6));

                        if ui.add_sized([120.0, 36.0], close_btn).clicked() {
                            close_requested = true;
                        }
                    });
                });
            });

        if close_requested {
            self.settings_open = false;
        }
    }
}

fn draw_headset_graphic(painter: &egui::Painter, center: Pos2) {
    let top_tab = Rect::from_min_max(center - vec2(16.0, 56.0), center + vec2(16.0, -48.0));
    painter.rect_filled(top_tab, egui::CornerRadius::same(4), Color32::from_rgb(0x23, 0x21, 0x2f));

    let strap_l = Rect::from_min_max(center - vec2(104.0, 9.0), center - vec2(84.0, -9.0));
    painter.rect_filled(strap_l, egui::CornerRadius::same(5), Color32::from_rgb(0x28, 0x26, 0x36));
    let strap_r = Rect::from_min_max(center + vec2(84.0, -9.0), center + vec2(104.0, 9.0));
    painter.rect_filled(strap_r, egui::CornerRadius::same(5), Color32::from_rgb(0x28, 0x26, 0x36));

    let body_rect = Rect::from_center_size(center, vec2(174.0, 102.0));
    painter.rect(
        body_rect,
        egui::CornerRadius::same(34),
        Color32::from_rgb(0x1d, 0x1b, 0x27),
        egui::Stroke::new(2.0f32, Color32::from_rgb(0x31, 0x2f, 0x40)),
        egui::StrokeKind::Middle,
    );

    let plate_rect = Rect::from_center_size(center, vec2(154.0, 84.0));
    painter.rect(
        plate_rect,
        egui::CornerRadius::same(26),
        Color32::from_rgb(0x13, 0x12, 0x19),
        egui::Stroke::new(1.0f32, Color32::from_rgb(0x28, 0x26, 0x36)),
        egui::StrokeKind::Middle,
    );

    for &dx in &[-40.0f32, 0.0f32, 40.0f32] {
        let pill_center = center + vec2(dx, 0.0);
        let pill_rect = Rect::from_center_size(pill_center, vec2(20.0, 46.0));

        painter.rect(
            pill_rect,
            egui::CornerRadius::same(10),
            Color32::from_rgb(0x0c, 0x0b, 0x11),
            egui::Stroke::new(1.5f32, Color32::from_rgb(0x36, 0x33, 0x47)),
            egui::StrokeKind::Middle,
        );

        painter.rect_stroke(
            pill_rect.shrink(1.0),
            egui::CornerRadius::same(9),
            egui::Stroke::new(1.0f32, Color32::from_rgba_unmultiplied(139, 92, 246, 70)),
            egui::StrokeKind::Middle,
        );

        let top_lens = pill_center - vec2(0.0, 10.0);
        painter.circle(top_lens, 6.0, Color32::from_rgb(0x16, 0x13, 0x22), egui::Stroke::new(1.2f32, Color32::from_rgb(0x8b, 0x5c, 0xf6)));
        painter.circle_filled(top_lens, 3.2, Color32::from_rgb(0x23, 0x1c, 0x35));
        painter.circle_filled(top_lens + vec2(1.6, -1.6), 1.3, Color32::from_rgba_unmultiplied(255, 255, 255, 220));

        let bot_lens = pill_center + vec2(0.0, 10.0);
        painter.circle(bot_lens, 5.0, Color32::from_rgb(0x11, 0x0f, 0x18), egui::Stroke::new(1.0f32, Color32::from_rgb(0x63, 0x55, 0x88)));
        painter.circle_filled(bot_lens, 2.5, Color32::from_rgb(0x1d, 0x18, 0x29));
        painter.circle_filled(bot_lens + vec2(1.2, -1.2), 1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 160));
    }

    let led_pos = center + vec2(62.0, -28.0);
    painter.circle_filled(led_pos, 2.2, Color32::from_rgb(0x22, 0xc5, 0x5e));
    painter.circle(led_pos, 4.0, Color32::TRANSPARENT, egui::Stroke::new(1.0f32, Color32::from_rgba_unmultiplied(34, 197, 94, 90)));
}

fn install_custom_fonts(ctx: &egui::Context) {
    const NOTO_EMOJI: &[u8] = include_bytes!("../assets/NotoEmoji-Regular.ttf");
    const FA_SOLID: &[u8] = include_bytes!("../assets/fa-solid-900.ttf");
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert("fa_solid".to_owned(), Arc::new(egui::FontData::from_static(FA_SOLID)));
    fonts.font_data.insert("noto_emoji".to_owned(), Arc::new(egui::FontData::from_static(NOTO_EMOJI)));
    for fam in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        let family = fonts.families.entry(fam).or_default();
        family.push("fa_solid".to_owned());
        family.push("noto_emoji".to_owned());
    }
    ctx.set_fonts(fonts);
}

fn capture_dir() -> std::io::Result<PathBuf> {
    let base = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."));
    let dir = base.join("captures");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn capture_path(prefix: &str, ext: &str) -> std::io::Result<PathBuf> {
    let dir = capture_dir()?;
    let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    Ok(dir.join(format!("{prefix}_{ts}.{ext}")))
}

fn save_crop_png(frame: &Frame, uv: Rect) -> anyhow::Result<PathBuf> {
    let w = frame.width as usize;
    let h = frame.height as usize;
    if w == 0 || h == 0 { anyhow::bail!("empty frame"); }
    let wi = w as f32;
    let hi = h as f32;
    let x0 = (uv.min.x * wi).floor().clamp(0.0, wi - 1.0) as usize;
    let y0 = (uv.min.y * hi).floor().clamp(0.0, hi - 1.0) as usize;
    let x1 = ((uv.max.x * wi).ceil() as usize).clamp(x0 + 1, w);
    let y1 = ((uv.max.y * hi).ceil() as usize).clamp(y0 + 1, h);
    let cw = x1 - x0;
    let ch = y1 - y0;

    let mut out = vec![0u8; cw * ch * 4];
    for row in 0..ch {
        let src = ((y0 + row) * w + x0) * 4;
        let dst = row * cw * 4;
        out[dst..dst + cw * 4].copy_from_slice(&frame.rgba[src..src + cw * 4]);
    }

    let path = capture_path("quest", "png")?;
    let img = image::RgbaImage::from_raw(cw as u32, ch as u32, out)
        .ok_or_else(|| anyhow::anyhow!("frame buffer size mismatch"))?;
    img.save(&path)?;
    Ok(path)
}

fn draw_warped(
    painter: &egui::Painter,
    tex: egui::TextureId,
    rect: Rect,
    uv: Rect,
    k1: f32,
    k2: f32,
    rot_deg: f32,
) {
    use egui::epaint::{Mesh, Vertex};
    const N: usize = 48; 

    let (sin, cos) = rot_deg.to_radians().sin_cos();
    let center = rect.center();
    let half_w = rect.width() * 0.5;
    let half_h = rect.height() * 0.5;

    let mut mesh = Mesh::with_texture(tex);
    for j in 0..=N {
        for i in 0..=N {
            let fx = i as f32 / N as f32;
            let fy = j as f32 / N as f32;
            let ox = fx * 2.0 - 1.0;
            let oy = fy * 2.0 - 1.0;

            let r2 = ox * ox + oy * oy;
            let f = 1.0 + k1 * r2 + k2 * r2 * r2;
            let sx = (0.5 + 0.5 * ox * f).clamp(0.0, 1.0);
            let sy = (0.5 + 0.5 * oy * f).clamp(0.0, 1.0);
            let u = uv.min.x + sx * uv.width();
            let v = uv.min.y + sy * uv.height();

            let px = ox * half_w;
            let py = oy * half_h;
            let pos = pos2(center.x + px * cos - py * sin, center.y + px * sin + py * cos);

            mesh.vertices.push(Vertex { pos, uv: pos2(u, v), color: Color32::WHITE });
        }
    }

    let row = (N + 1) as u32;
    for j in 0..N as u32 {
        for i in 0..N as u32 {
            let a = j * row + i;
            let b = a + 1;
            let c = a + row;
            let d = c + 1;
            mesh.indices.extend_from_slice(&[a, b, c, b, d, c]);
        }
    }
    painter.add(egui::Shape::mesh(mesh));
}

fn screen_to_uv(pos: Pos2, draw_rect: Rect, uv: Rect) -> Pos2 {
    let fx = ((pos.x - draw_rect.min.x) / draw_rect.width()).clamp(0.0, 1.0);
    let fy = ((pos.y - draw_rect.min.y) / draw_rect.height()).clamp(0.0, 1.0);
    pos2(uv.min.x + fx * uv.width(), uv.min.y + fy * uv.height())
}

fn scale_about(uv: Rect, anchor: Pos2, factor: f32) -> Rect {
    let min = anchor + (uv.min - anchor) * factor;
    let max = anchor + (uv.max - anchor) * factor;
    Rect::from_min_max(min, max)
}

fn clamp_uv(mut r: Rect) -> Rect {
    let mut w = r.width().clamp(0.02, 1.0);
    let mut h = r.height().clamp(0.02, 1.0);
    let cx = r.center().x;
    let cy = r.center().y;
    if w > 1.0 { w = 1.0; }
    if h > 1.0 { h = 1.0; }
    let mut min = pos2(cx - w / 2.0, cy - h / 2.0);
    if min.x < 0.0 { min.x = 0.0; }
    if min.y < 0.0 { min.y = 0.0; }
    if min.x + w > 1.0 { min.x = 1.0 - w; }
    if min.y + h > 1.0 { min.y = 1.0 - h; }
    r = Rect::from_min_size(min, vec2(w, h));
    r
}

#[cfg(test)]
mod test_layout {
    use super::*;

    #[test]
    fn test_sidebar_layout() {
        let ctx = egui::Context::default();
        apply_purple_theme(&ctx);
        install_custom_fonts(&ctx);
        let mut app = App {
            devices: vec![
                Device {
                    serial: "340YC10G810KRV".into(),
                    state: "device".into(),
                    usb: true,
                    model: "Quest_3S".into(),
                }
            ],
            selected_serial: Some("340YC10G810KRV".into()),
            displays: vec![DisplayInfo { id: 0, width: 1920, height: 1080 }],
            display_fetch: Arc::new(Mutex::new(None)),
            settings: Settings {
                display_id: 0,
                max_size: 1920,
                bitrate_mbps: 30,
                max_fps: 60,
                audio: false,
            },
            stream: None,
            flat: None,
            texture: None,
            last_generation: 0,
            tex_size: [0, 0],
            last_frame: None,
            capture_msg: None,
            spout_enabled: false,
            spout: None,
            spout_error: None,
            vcam_enabled: false,
            vcam: None,
            vcam_error: None,
            uv: Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
            lens_correct: false,
            lens_k1: 0.0,
            lens_k2: 0.0,
            rotation_deg: 0.0,
            want_autostart: false,
            auto_reconnect: false,
            need_display_fetch: false,
            fetch_inflight: false,
            remote_input: String::new(),
            remotes: vec!["192.168.1.105:5555".into()],
            connect_result: Arc::new(Mutex::new(None)),
            connecting: false,
            streamer_forced: Some(false), // UNMASKED!
            obs_detected: false,
            last_obs_check: Instant::now(),
            persisted: Config::default(),
            dirty_since: None,
            settings_open: false,
        };

        // 1. Idle state
        let _ = ctx.run_ui(Default::default(), |ui| {
            let before = ui.available_rect_before_wrap().min.x;
            app.sidebar(ui);
            let after = ui.available_rect_before_wrap().min.x;
            let used = after - before;
            assert!((used - 302.0).abs() < 0.1, "Sidebar idle width expanded beyond expected: got {used}");
        });

        // 2. Streaming + Spout + VCam state
        app.spout_enabled = true;
        app.vcam_enabled = true;
        app.stream = Some(StreamHandle::dummy_for_test());
        let _ = ctx.run_ui(Default::default(), |ui| {
            let before = ui.available_rect_before_wrap().min.x;
            app.sidebar(ui);
            let after = ui.available_rect_before_wrap().min.x;
            let used = after - before;
            assert!((used - 302.0).abs() < 0.1, "Sidebar streaming width expanded beyond expected: got {used}");
        });
    }

    #[test]
    fn test_settings_modal_close() {
        let ctx = egui::Context::default();
        let mut app = App {
            devices: vec![],
            selected_serial: None,
            displays: vec![],
            display_fetch: Arc::new(Mutex::new(None)),
            settings: Settings {
                display_id: 0,
                max_size: 1920,
                bitrate_mbps: 30,
                max_fps: 60,
                audio: false,
            },
            stream: None,
            flat: None,
            texture: None,
            last_generation: 0,
            tex_size: [0, 0],
            last_frame: None,
            capture_msg: None,
            spout_enabled: false,
            spout: None,
            spout_error: None,
            vcam_enabled: false,
            vcam: None,
            vcam_error: None,
            uv: Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
            lens_correct: false,
            lens_k1: 0.0,
            lens_k2: 0.0,
            rotation_deg: 0.0,
            want_autostart: false,
            auto_reconnect: false,
            need_display_fetch: false,
            fetch_inflight: false,
            remote_input: String::new(),
            remotes: vec![],
            connect_result: Arc::new(Mutex::new(None)),
            connecting: false,
            streamer_forced: None,
            obs_detected: false,
            last_obs_check: Instant::now(),
            persisted: Config::default(),
            dirty_since: None,
            settings_open: true,
        };

        // Modal is open
        assert!(app.settings_open);

        // Run with Escape key pressed
        let mut raw_input = egui::RawInput::default();
        raw_input.events.push(egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::default(),
        });
        let _ = ctx.run_ui(raw_input, |_ui| {
            app.settings_modal(&ctx);
        });

        // Now settings_open must be false
        assert!(!app.settings_open, "Settings modal did not close on Escape key!");
    }
}

