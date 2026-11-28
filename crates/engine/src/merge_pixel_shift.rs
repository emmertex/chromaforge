//! The Pixel Shift merge as a job: the frames are read and decoded, registered (their positions measured on the data and
//! the image reconstructed from them) or, when asked or when the series cannot be registered, placed on the camera's own
//! pattern, and the result is written as a LinearRaw DNG carrying frame 01's colour, lens data and metadata, where it
//! came from and how it was merged.
//!
//! Cancelling at any point (decoding, measuring, reconstructing, before and after the DNG is built) ends the job with
//! "cancelled" before anything is written to the library: the file is written by [`crate::Session::finish_merge`] only
//! from a finished result.

use std::sync::Mutex;

use lcc_merge::pixelshift::{self, DngSource};
use lcc_merge::pixelshift_registered::pipeline::{Options, Report, merge_series_registered};
use lcc_merge::pixelshift_registered::register::FallbackReason;
use lcc_raw::merge_provenance::{ENGINE, ENGINE_REGISTERED, Provenance, Registration, SourceFrame, Window};
use serde_json::{Value, json};

use crate::merge::{MergeJob, MergeKind, MergeOutput, PIXEL_SHIFT_PACKING, ProgressFn};

/// The reason code of a series that was merged plainly because the registered merge would not have fitted in memory.
pub const REASON_MEMORY: &str = "memory";

/// The short code of a reason the registered merge was not used (written to the file and the catalog).
pub fn reason_code(r: &FallbackReason) -> &'static str {
    match r {
        FallbackReason::NoTexture => "noTexture",
        FallbackReason::TooFewTiles { .. } => "tooFewTiles",
        FallbackReason::NoStart | FallbackReason::FrameCount(_) => "noMatch",
        FallbackReason::MeasuredWorse(_) => "measuredWorse",
    }
}

/// The reason in plain words (English; the app translates it).
pub fn reason_message(code: &str) -> &'static str {
    match code {
        "noTexture" | "tooFewTiles" => "not enough texture to measure the frame positions",
        "measuredWorse" => "the measured positions were less reliable than the camera's own pattern",
        REASON_MEMORY => "there was not enough memory for the registered merge",
        _ => "the frames could not be matched to each other",
    }
}

fn verdict_text(report: &Report) -> Option<String> {
    use lcc_merge::pixelshift_registered::gate::GateVerdict as V;
    report.gate.as_ref().map(|g| {
        match g.verdict {
            V::Better => "better",
            V::Equal => "equal",
            V::Worse => "worse",
        }
        .to_string()
    })
}

/// What the stage of the progress fraction `f` (of the registered merge's own scale) is called.
fn stage(f: f32) -> &'static str {
    if f < 0.15 {
        "Reading frames"
    } else if f < 0.40 {
        "Measuring positions"
    } else {
        "Reconstructing"
    }
}

impl MergeJob {
    /// A pixel-shift merge. `progress(fraction, stage)` returns false to cancel.
    pub(crate) fn run_pixel_shift(&self, progress: &ProgressFn) -> std::result::Result<MergeOutput, String> {
        let plan = self.pixel_shift.as_ref().ok_or("the photos were not checked as a pixel-shift series")?;
        let pattern = pixelshift::pattern_for(plan.count).ok_or_else(|| format!("{}-shot pixel-shift series are not supported yet", plan.count))?;
        if self.sources.len() != pattern.shifts.len() {
            return Err(format!("the series has {} shots but {} photos were given", plan.count, self.sources.len()));
        }
        if !progress(0.0, "Reading frame 1") {
            return Err("cancelled".into());
        }
        // frame 01 gives the result its colour, lens data, metadata and place on the sensor
        let bytes = (self.read)(&self.sources[0].1)?;
        let mut first = crate::raw_decoder::decode(&bytes).map_err(|e| format!("{}: {e}", self.sources[0].1))?;
        let lens = first.opcodes.list3.clone();
        // the colour fit sees the raw without its lens warp, as when the photo opens
        first.opcodes.list3.retain(|op| !op.is_lens_correction());
        let tags = crate::camera_preview::starting_colour_tags(&mut first, &bytes);
        first.opcodes.list3.clone_from(&lens);
        let mut color = tags.unwrap_or_else(|| first.color.clone());
        if color.as_shot_neutral.is_none()
            && color.as_shot_white_xy.is_none()
            && let Some(m) = first.wb_multipliers.filter(|m| m.iter().all(|v| *v > 0.0 && v.is_finite()))
        {
            color.as_shot_neutral = Some(m.map(|v| 1.0 / f64::from(v)));
        }
        let window = lcc_raw::sensor_window(&bytes, [first.active_area.width, first.active_area.height]).map(|w| Window {
            sensor: w.sensor,
            origin: w.origin,
            size: w.size,
        });
        // frame 01's own JPEG goes into the file as its preview: the colour look is fitted to it when the file opens, as it
        // is for the camera's raw
        let preview = lcc_raw::embedded_preview(&bytes)
            .filter(|jpeg| jpeg.starts_with(&[0xff, 0xd8]))
            .map(|jpeg| lcc_raw::DngPreview { jpeg, color_space: lcc_raw::embedded_preview_color_space(&bytes) });
        drop(bytes);
        // the catalog's descriptive fields and keywords win; frame 01's file fills in the rest (camera, lens, exposure,
        // capture time with its sub-seconds and offset). Rating and label are not carried.
        let mut metadata = plan.metadata.clone();
        metadata.fill_missing(&first.metadata);
        metadata.rating = None;
        metadata.label = None;
        if metadata.capture_time.is_none() {
            metadata.capture_time = plan.frames.first().and_then(|f| lcc_meta::DateTime::parse_iso(&f.captured));
        }
        let orientation = first.orientation;
        let first = Mutex::new(Some(first));
        let load = |i: usize| -> std::result::Result<lcc_raw::RawImage, String> {
            if i == 0
                && let Some(f) = first.lock().ok().and_then(|mut g| g.take())
            {
                return Ok(f);
            }
            let path = &self.sources[i].1;
            let bytes = (self.read)(path)?;
            crate::raw_decoder::decode(&bytes).map_err(|e| format!("{path}: {e}"))
        };
        let wanted = matches!(self.kind, MergeKind::PixelShift { registered: true });
        let (merged, registration, fallback) = if wanted && !plan.memory_limited {
            let step = |f: f32| progress(0.02 + 0.78 * f, stage(f));
            let (merged, report) =
                merge_series_registered(pattern, 0..self.sources.len(), &load, &step, &Options::default()).map_err(|e| e.to_string())?;
            let registration = registration_of(&report);
            let fallback = report.reason.as_ref().map(|r| reason_code(r).to_string());
            (merged, registration, fallback)
        } else {
            // the plain merge: frames are placed one at a time (the merged image is the only large buffer)
            let step = |f: f32| progress(0.02 + 0.78 * f, &format!("Merging frame {} of {}", (f * plan.count as f32) as usize + 1, plan.count));
            let mut plain_load = |i: usize| load(i);
            let merged = pixelshift::merge_series(pattern, 0..self.sources.len(), &mut plain_load, &step).map_err(|e| e.to_string())?;
            let memory = wanted.then(|| REASON_MEMORY.to_string());
            let registration =
                Registration { registered: false, verdict: None, reason: memory.clone(), gains: false, tiles: 0, models: Vec::new(), made_at: None };
            (merged, registration, memory)
        };
        // when it was made: two merges of one series are never the same file, whatever the method
        let registration = Registration { made_at: Some(chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)), ..registration };
        if !progress(0.82, "Writing DNG") {
            return Err("cancelled".into());
        }
        let (width, height, filled) = (merged.width, merged.height, merged.filled);
        let provenance = Provenance {
            engine: if registration.registered { ENGINE_REGISTERED } else { ENGINE },
            shot_count: plan.count,
            reference: 1,
            scale: pattern.scale as u16,
            window,
            frames: plan
                .frames
                .iter()
                .map(|f| SourceFrame { index: f.index, file_name: f.file_name.clone(), captured: f.captured.clone(), counter: f.counter })
                .collect(),
            registration: Some(registration.clone()),
        };
        let source = DngSource { metadata, color, orientation, lens, provenance, preview };
        let dng = pixelshift::write_dng(merged, source, PIXEL_SHIFT_PACKING).map_err(|e| e.to_string())?;
        // a cancel that came while the file was being built still leaves nothing behind
        if !progress(0.99, "Writing DNG") {
            return Err("cancelled".into());
        }
        progress(1.0, "Done");
        let mut info = json!({"kind": "pixelShift", "shots": plan.count, "width": width, "height": height, "filledNodes": filled});
        info["method"] = json!(if registration.registered { "registered" } else { "plain" });
        info["registration"] = registration_json(&registration);
        if let Some(code) = fallback {
            info["fallback"] = json!({"reason": code, "message": reason_message(&code)});
        }
        Ok(MergeOutput { dng, preview: None, crop: None, info, layers: None })
    }
}

fn registration_of(report: &Report) -> Registration {
    Registration {
        registered: report.registered,
        verdict: verdict_text(report),
        reason: report.reason.as_ref().map(|r| reason_code(r).to_string()),
        gains: report.gains.as_ref().is_some_and(|g| g.significant()),
        tiles: u32::try_from(report.tiles).unwrap_or(u32::MAX),
        made_at: None,
        models: if report.registered {
            report.models.iter().map(|m| [m.t[0] as f32, m.t[1] as f32, m.theta as f32, m.scale as f32]).collect()
        } else {
            Vec::new()
        },
    }
}

/// How a merge was made, as the result of the job reports it (the catalog keeps the same without the models).
fn registration_json(r: &Registration) -> Value {
    json!({"registered": r.registered, "verdict": r.verdict, "reason": r.reason, "gains": r.gains, "tiles": r.tiles})
}

/// What the catalog remembers of how a merge was made, from the job's result.
pub(crate) fn catalog_registration(info: &Value) -> Option<lcc_catalog::MergeRegistration> {
    let r = info.get("registration")?;
    Some(lcc_catalog::MergeRegistration {
        registered: r.get("registered")?.as_bool()?,
        verdict: r.get("verdict").and_then(Value::as_str).map(str::to_string),
        reason: r.get("reason").and_then(Value::as_str).map(str::to_string),
        gains: r.get("gains").and_then(Value::as_bool).unwrap_or(false),
    })
}

/// The same for a merged file imported on its own: from its XMP block.
pub(crate) fn catalog_registration_of_file(r: &Registration) -> lcc_catalog::MergeRegistration {
    lcc_catalog::MergeRegistration { registered: r.registered, verdict: r.verdict.clone(), reason: r.reason.clone(), gains: r.gains }
}
