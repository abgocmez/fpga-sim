//! HDMI and audio codec panels.

use super::{App, dialogs};
use crate::model::{QuantityWidget, storage_rows, toggle_state, toggle_value};
use eframe::egui::{self, Color32, ColorImage, RichText, TextureOptions};

/// Warning colour for guest writes that reach the host file.
const WARN: Color32 = Color32::from_rgb(235, 160, 20);

/// The name of a Pmod connector on the board: `JA`..`JE`, `CAM` for the camera connector.
pub(super) fn pmod_port_name(port: u8) -> &'static str {
    ["JA", "JB", "JC", "JD", "JE", "CAM"]
        .get(usize::from(port))
        .copied()
        .unwrap_or("?")
}

/// The storage table: device, image, size, simulated part and access.
pub(super) fn storage_table(ui: &mut egui::Ui, st: &sim_api::StorageStatus) {
    egui::Grid::new("storage_table")
        .striped(true)
        .num_columns(5)
        .spacing([10.0, 3.0])
        .show(ui, |ui| {
            for h in ["device", "image / source", "size", "model", "access"] {
                ui.strong(h);
            }
            ui.end_row();
            for r in storage_rows(st) {
                ui.label(r.device);
                ui.label(RichText::new(&r.source).monospace().small())
                    .on_hover_text(r.source.clone());
                ui.label(&r.size);
                ui.label(RichText::new(&r.model).small());
                if r.writable {
                    ui.label(RichText::new("writable").color(WARN).strong())
                        .on_hover_text(
                            "Guest writes change the host file itself: they are not discarded.",
                        );
                } else {
                    ui.label(RichText::new("read-only").color(Color32::GRAY))
                        .on_hover_text(
                            "Writes discarded: they go to a throw-away overlay, and the host \n                             file is never modified.",
                        );
                }
                ui.end_row();
            }
        });
}

/// What the LINE IN source box offers.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum AudioChoice {
    Silence,
    Tone,
    File,
}

/// What the HDMI RX source box offers.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum VideoChoice {
    Bars,
    Gradient,
    File,
}

/// Widget state of the two panels.
pub(super) struct MediaUi {
    pub video_choice: VideoChoice,
    pub video_size: String,
    pub video_file: String,
    pub audio_choice: AudioChoice,
    pub tone_hz: f32,
    pub tone_amp: f32,
    pub audio_file: String,
    pub listen: bool,
    pub notice: Option<String>,
    pub tex: Option<egui::TextureHandle>,
    pub tex_frame: Option<u32>,
    #[cfg(feature = "audio")]
    pub speaker: Option<crate::audio_out::Speaker>,
}

impl Default for MediaUi {
    fn default() -> MediaUi {
        MediaUi {
            video_choice: VideoChoice::Bars,
            video_size: "320x240".to_string(),
            video_file: String::new(),
            audio_choice: AudioChoice::Tone,
            tone_hz: 440.0,
            tone_amp: 0.5,
            audio_file: String::new(),
            listen: false,
            notice: None,
            tex: None,
            tex_frame: None,
            #[cfg(feature = "audio")]
            speaker: None,
        }
    }
}

impl App {
    fn video_spec_text(&self) -> String {
        let m = &self.media;
        match m.video_choice {
            VideoChoice::Bars => format!("bars:{}", m.video_size.trim()),
            VideoChoice::Gradient => format!("gradient:{}", m.video_size.trim()),
            VideoChoice::File => m.video_file.trim().to_string(),
        }
    }

    fn audio_spec_text(&self) -> String {
        let m = &self.media;
        match m.audio_choice {
            AudioChoice::Silence => "silence".to_string(),
            AudioChoice::Tone => format!("tone:{}:{}", m.tone_hz, m.tone_amp),
            AudioChoice::File => m.audio_file.trim().to_string(),
        }
    }

    /// HDMI: the TX picture and the RX source.
    pub(super) fn hdmi_panel(&mut self, ui: &mut egui::Ui) {
        let counters = self.model.status.media.clone().unwrap_or_default();
        ui.label(format!(
            "TX: {} frame(s){}",
            counters.video_frames,
            counters
                .video_size
                .map(|(w, h)| format!(", {w}x{h}"))
                .unwrap_or_default()
        ));
        // Upload the newest frame once.
        if let Some(v) = &self.model.video {
            if self.media.tex_frame != Some(v.frame) || self.media.tex.is_none() {
                let img = ColorImage::from_rgba_unmultiplied(
                    [v.width as usize, v.height as usize],
                    &v.rgba,
                );
                match &mut self.media.tex {
                    Some(t) => t.set(img, TextureOptions::LINEAR),
                    None => {
                        self.media.tex = Some(ui.ctx().load_texture(
                            "hdmi_tx",
                            img,
                            TextureOptions::LINEAR,
                        ));
                    }
                }
                self.media.tex_frame = Some(v.frame);
            }
        }
        match (&self.media.tex, &self.model.video) {
            (Some(tex), Some(v)) => {
                let w = ui.available_width().min(300.0);
                let h = w * v.height as f32 / v.width.max(1) as f32;
                ui.add(egui::Image::new((tex.id(), egui::vec2(w, h))));
                ui.label(
                    RichText::new(format!("frame {}", v.frame))
                        .small()
                        .color(Color32::GRAY),
                );
            }
            _ => {
                ui.label(
                    RichText::new(
                        "No complete frame yet. The PL must scan out a frame buffer and the \
                         simulation has to run for a few milliseconds.",
                    )
                    .small()
                    .color(Color32::GRAY),
                );
            }
        }

        ui.separator();
        ui.strong("RX source (what the PL receives)");
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.media.video_choice, VideoChoice::Bars, "bars");
            ui.selectable_value(
                &mut self.media.video_choice,
                VideoChoice::Gradient,
                "gradient",
            );
            ui.selectable_value(&mut self.media.video_choice, VideoChoice::File, "PNG file");
        });
        match self.media.video_choice {
            VideoChoice::File => {
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.media.video_file)
                            .desired_width(170.0)
                            .hint_text("picture.png"),
                    );
                    if ui.button("Browse…").clicked() {
                        if let Some(f) = dialogs::pick_file(
                            "HDMI RX picture",
                            &self.draft.dir,
                            &[("PNG", &["png"])],
                        ) {
                            self.media.video_file = f.to_string_lossy().into_owned();
                        }
                    }
                });
            }
            _ => {
                ui.horizontal(|ui| {
                    ui.label("size");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.media.video_size).desired_width(80.0),
                    );
                    ui.label(
                        RichText::new("(the PL raster decides what is used)")
                            .small()
                            .color(Color32::GRAY),
                    );
                });
            }
        }
        if ui.button("Apply picture").clicked() {
            let text = self.video_spec_text();
            match sim_api::parse_video_spec(&text, &self.draft.dir) {
                Ok(spec) => {
                    self.model.set_video_source(spec);
                    self.media.notice = None;
                }
                Err(e) => self.media.notice = Some(e.message),
            }
        }
        if let Some(n) = &self.media.notice {
            ui.colored_label(Color32::from_rgb(240, 120, 90), n);
        }
    }

    /// Audio codec: levels, listening and the LINE IN source.
    pub(super) fn audio_panel(&mut self, ui: &mut egui::Ui) {
        let c = self.model.status.media.clone().unwrap_or_default();
        ui.label(format!(
            "DAC: {} frame(s), {} codec register write(s)",
            c.audio_frames, c.codec_writes
        ));
        ui.add(
            egui::ProgressBar::new(self.model.audio_level.clamp(0.0, 1.0))
                .desired_width(ui.available_width().min(260.0))
                .text(format!("level {:.0} %", self.model.audio_level * 100.0)),
        );

        #[cfg(feature = "audio")]
        {
            let before = self.media.listen;
            ui.checkbox(&mut self.media.listen, "listen on the speakers");
            if self.media.listen && self.media.speaker.is_none() {
                match crate::audio_out::Speaker::open() {
                    Ok(s) => self.media.speaker = Some(s),
                    Err(e) => {
                        self.media.listen = false;
                        self.media.notice = Some(e);
                    }
                }
            }
            if !self.media.listen {
                self.media.speaker = None;
            }
            if before != self.media.listen {
                self.model.audio_wanted = self.media.listen;
            }
            if let Some(s) = &self.media.speaker {
                ui.label(
                    RichText::new(format!(
                        "{} at {} Hz, {} ms queued, {} frame(s) dropped",
                        s.name,
                        s.rate,
                        s.queued() * 1000 / 48_000,
                        s.dropped()
                    ))
                    .small()
                    .color(Color32::GRAY),
                );
                ui.label(
                    RichText::new(
                        "Audio plays what the simulation has produced. Below real-time \
                         speed it has gaps; above it, old audio is dropped.",
                    )
                    .small()
                    .color(Color32::GRAY),
                );
            }
        }
        #[cfg(not(feature = "audio"))]
        ui.label(
            RichText::new("This build has no speaker output; use audio capture instead.")
                .small()
                .color(Color32::GRAY),
        );

        ui.separator();
        ui.strong("LINE IN source (what the codec hears)");
        ui.horizontal(|ui| {
            ui.selectable_value(
                &mut self.media.audio_choice,
                AudioChoice::Silence,
                "silence",
            );
            ui.selectable_value(&mut self.media.audio_choice, AudioChoice::Tone, "tone");
            ui.selectable_value(&mut self.media.audio_choice, AudioChoice::File, "WAV file");
        });
        match self.media.audio_choice {
            AudioChoice::Tone => {
                ui.add(
                    egui::Slider::new(&mut self.media.tone_hz, 50.0..=8000.0)
                        .logarithmic(true)
                        .suffix(" Hz"),
                );
                ui.add(egui::Slider::new(&mut self.media.tone_amp, 0.0..=1.0).text("amplitude"));
            }
            AudioChoice::File => {
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.media.audio_file)
                            .desired_width(170.0)
                            .hint_text("sound.wav"),
                    );
                    if ui.button("Browse…").clicked() {
                        if let Some(f) = dialogs::pick_file(
                            "LINE IN audio",
                            &self.draft.dir,
                            &[("WAV", &["wav"])],
                        ) {
                            self.media.audio_file = f.to_string_lossy().into_owned();
                        }
                    }
                });
            }
            AudioChoice::Silence => {}
        }
        if ui.button("Apply sound").clicked() {
            let text = self.audio_spec_text();
            match sim_api::parse_audio_spec(&text, &self.draft.dir) {
                Ok(spec) => {
                    self.model.set_audio_source(spec);
                    self.media.notice = None;
                }
                Err(e) => self.media.notice = Some(e.message),
            }
        }
    }

    /// The Pmod devices: every attached device with its state line, its picture when it has one
    /// (the OLED display), and the quantities the host may set (a checkbox for a 0/1 level, a
    /// slider otherwise).
    pub(super) fn devices_panel(&mut self, ui: &mut egui::Ui) {
        for row in self.model.device_rows() {
            let port = row.port;
            ui.strong(format!("{} {}", pmod_port_name(port), row.kind));
            if let Some((_, _, _, w, h, rgba)) = self
                .model
                .device_images
                .iter()
                .find(|d| d.0 == port)
                .cloned()
            {
                let img = ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba);
                let tex =
                    ui.ctx()
                        .load_texture(format!("device{port}"), img, TextureOptions::NEAREST);
                let width = ui.available_width().min(w as f32 * 2.5);
                ui.add(egui::Image::new((
                    tex.id(),
                    egui::vec2(width, width * h as f32 / w as f32),
                )));
            }
            ui.label(RichText::new(&row.state).small().color(Color32::GRAY));
            for (q, widget) in row.quantities {
                match widget {
                    QuantityWidget::Toggle => {
                        let mut on = toggle_state(q.value);
                        let r = ui.checkbox(&mut on, format!("{} ({})", q.name, q.unit));
                        if r.changed() {
                            self.model.set_device_value(port, &q.name, toggle_value(on));
                        }
                    }
                    QuantityWidget::Slider => {
                        let mut v = q.value;
                        let r = ui.add(
                            egui::Slider::new(&mut v, q.min..=q.max)
                                .text(format!("{} ({})", q.name, q.unit))
                                .clamping(egui::SliderClamping::Always),
                        );
                        if r.changed() {
                            self.model.set_device_value(port, &q.name, v);
                        }
                    }
                }
            }
        }
    }

    /// Moves fetched DAC audio to the speaker.
    pub(super) fn pump_audio(&mut self) {
        #[cfg(feature = "audio")]
        {
            let samples = self.model.take_audio();
            if let Some(s) = &self.media.speaker {
                if !samples.is_empty() {
                    s.push(&samples);
                }
            }
        }
    }
}
