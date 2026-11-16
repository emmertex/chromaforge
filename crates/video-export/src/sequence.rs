//! Encode a sequence of frames the caller makes (a timelapse of stills) as a silent movie.
//!
//! Same encoders and muxer as [`crate::export_clip`]; the frames come from the caller instead of
//! a decoded clip. The file is written to `<out>.partial` and renamed by [`SequenceWriter::finish`];
//! dropping the writer unfinished (an error, a cancel) removes the partial file. The writer holds a
//! lock on its partial file: one left by a crash is reused, one another export is writing is not.
//!
//! Sources: own design over FilmCraft's public crate APIs.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use filmcraft_isobmff::{Brand, Mp4Writer, WriteSample, WriterOptions};
use lcc_raster::Rgb32f;

use crate::native::{ClipFormat, ClipReport, ExportClipError, MAX_FRAMES, MAX_SIDE, Result, VideoOut, enc, io, partial_path, ycbcr_with};

/// Most partial files tried for one output (`<out>.partial`, `<out>.2.partial`, …).
const PARTIAL_SLOTS: u32 = 16;

/// Open and lock a partial file for `out`: `<out>.partial` unless another writer holds it (then
/// `<out>.2.partial` and on), truncating a stale one a crashed export left. The lock lasts as long
/// as the file is open.
fn open_partial(out: &Path) -> Result<(PathBuf, File)> {
    let first = partial_path(out)?;
    for n in 1..=PARTIAL_SLOTS {
        let path = if n == 1 {
            first.clone()
        } else {
            let mut name = out.file_name().unwrap_or_default().to_os_string();
            name.push(format!(".{n}.partial"));
            out.with_file_name(name)
        };
        // (no truncate on open: the file may be another writer's until the lock says otherwise)
        let file = File::options().read(true).write(true).create(true).truncate(false).open(&path).map_err(io)?;
        match file.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => continue,
            // a file system without locks: take the file as before
            Err(std::fs::TryLockError::Error(e)) if e.kind() == std::io::ErrorKind::Unsupported => {}
            Err(std::fs::TryLockError::Error(e)) => return Err(io(e)),
        }
        file.set_len(0).map_err(io)?;
        return Ok((path, file));
    }
    Err(ExportClipError::BadOptions(format!("{} is being written by {PARTIAL_SLOTS} other exports", out.display())))
}

/// Writes frames of one fixed size at a fixed rate to an H.264 MP4 or ProRes MOV (no audio).
pub struct SequenceWriter {
    mux: Option<Mp4Writer<BufWriter<File>>>,
    video: VideoOut,
    track: usize,
    /// The audio track and its packets, written when the movie is finished.
    audio: Option<(usize, crate::EncodedAudioTrack)>,
    partial: PathBuf,
    out: PathBuf,
    frames: i64,
    width: u32,
    height: u32,
    fps_num: u32,
    fps_den: u32,
}

impl SequenceWriter {
    /// A writer for `width × height` frames (even, 16..=[`MAX_SIDE`]) at `fps_num / fps_den`
    /// frames per second (0.1..=240). `quality` 1–100 as in [`crate::ClipExportOptions`].
    pub fn create(out: &Path, format: ClipFormat, quality: u8, width: u32, height: u32, fps_num: u32, fps_den: u32) -> Result<Self> {
        Self::create_with_audio(out, format, quality, width, height, fps_num, fps_den, None)
    }

    /// [`Self::create`] with an encoded audio track (from [`crate::encode_pcm_track`] for the same
    /// `format`), muxed beside the frames.
    #[allow(clippy::too_many_arguments)]
    pub fn create_with_audio(
        out: &Path,
        format: ClipFormat,
        quality: u8,
        width: u32,
        height: u32,
        fps_num: u32,
        fps_den: u32,
        audio: Option<crate::EncodedAudioTrack>,
    ) -> Result<Self> {
        let bad = |m: String| Err(ExportClipError::BadOptions(m));
        if !(16..=MAX_SIDE).contains(&width) || !(16..=MAX_SIDE).contains(&height) || !width.is_multiple_of(2) || !height.is_multiple_of(2) {
            return bad(format!("frame size {width}×{height} must be even and 16–{MAX_SIDE}"));
        }
        if fps_num == 0 || fps_den == 0 || f64::from(fps_num) / f64::from(fps_den) > 240.0 || f64::from(fps_num) / f64::from(fps_den) < 0.1 {
            return bad(format!("frame rate {fps_num}/{fps_den} is out of range"));
        }
        let brand = match format {
            ClipFormat::H264Mp4 => Brand::Mp4,
            ClipFormat::ProResMov => Brand::Mov,
        };
        let video = VideoOut::new(format, quality.clamp(1, 100), width, height, fps_num, fps_den)?;
        let (partial, file) = open_partial(out)?;
        let made = (|| {
            let mut mux = Mp4Writer::new(BufWriter::new(file), WriterOptions::new(brand)).map_err(enc)?;
            let track = mux.add_track(video.track_config()).map_err(enc)?;
            let audio = match audio {
                Some(a) => Some((mux.add_track(a.track_config()).map_err(enc)?, a)),
                None => None,
            };
            Ok::<_, ExportClipError>((mux, track, audio))
        })();
        let (mux, track, audio) = match made {
            Ok(v) => v,
            Err(e) => {
                let _ = std::fs::remove_file(&partial);
                return Err(e);
            }
        };
        Ok(Self { mux: Some(mux), video, track, audio, partial, out: out.to_path_buf(), frames: 0, width, height, fps_num, fps_den })
    }

    /// Frames written so far.
    pub fn frames(&self) -> u64 {
        self.frames as u64
    }

    /// Append one frame of **encoded** R'G'B' values (0..=1, e.g. an sRGB render divided by 255),
    /// stored as BT.709 video without a transfer change. Its size must be the writer's.
    pub fn push_encoded(&mut self, frame: &Rgb32f) -> Result<()> {
        self.push_inner(frame, false)
    }

    /// Append one frame of linear light (the [`crate::GradeFrame`] convention).
    pub fn push_linear(&mut self, frame: &Rgb32f) -> Result<()> {
        self.push_inner(frame, true)
    }

    fn push_inner(&mut self, frame: &Rgb32f, linear: bool) -> Result<()> {
        if frame.width != self.width as usize || frame.height != self.height as usize || frame.data.len() != frame.width * frame.height {
            return Err(ExportClipError::BadOptions(format!("frame is {}×{}, the movie {}×{}", frame.width, frame.height, self.width, self.height)));
        }
        if self.frames >= MAX_FRAMES {
            return Err(ExportClipError::BadOptions("the movie is too long".into()));
        }
        let mux = self.mux.as_mut().ok_or(ExportClipError::Internal)?;
        for p in self.video.encode_ycbcr(&ycbcr_with(frame, linear), self.frames)? {
            mux.write_sample(self.track, WriteSample { data: &p.data, duration: self.fps_den, composition_offset: p.cto, is_sync: p.key })
                .map_err(enc)?;
        }
        self.frames += 1;
        Ok(())
    }

    /// Flush the encoder, finish the file and move it to its place.
    pub fn finish(mut self) -> Result<ClipReport> {
        if self.frames == 0 {
            return Err(ExportClipError::BadOptions("no frames".into()));
        }
        // Don't take mux yet; keep it in self.mux so Drop removes the partial
        // file if any write fails.
        for p in self.video.flush() {
            let mux = self.mux.as_mut().ok_or(ExportClipError::Internal)?;
            mux.write_sample(self.track, WriteSample { data: &p.data, duration: self.fps_den, composition_offset: p.cto, is_sync: p.key })
                .map_err(enc)?;
        }
        let has_audio = self.audio.is_some();
        if let Some((tr, a)) = self.audio.take() {
            for p in &a.packets {
                let mux = self.mux.as_mut().ok_or(ExportClipError::Internal)?;
                mux.write_sample(tr, WriteSample { data: &p.data, duration: p.duration, composition_offset: 0, is_sync: true }).map_err(enc)?;
            }
        }
        // All writes succeeded; now finalize and take the mux. Past this point Drop no longer sees the
        // mux, so a failure here (finish, flush, sync, rename) removes the partial file itself.
        let finished = (|| {
            let mux = self.mux.take().ok_or(ExportClipError::Internal)?;
            let mut out = mux.finish().map_err(enc)?;
            out.flush().map_err(io)?;
            let file = out.into_inner().map_err(|e| io(e.error()))?;
            file.sync_all().map_err(io)?;
            drop(file);
            std::fs::rename(&self.partial, &self.out).map_err(io)
        })();
        if let Err(e) = finished {
            let _ = std::fs::remove_file(&self.partial);
            return Err(e);
        }
        let frames = self.frames as u64;
        Ok(ClipReport {
            frames,
            duration: frames as f64 * f64::from(self.fps_den) / f64::from(self.fps_num),
            path: self.out.clone(),
            width: self.width,
            height: self.height,
            has_audio,
        })
    }
}

impl Drop for SequenceWriter {
    fn drop(&mut self) {
        // unfinished (error, cancel): never leave a truncated movie behind
        if self.mux.take().is_some() {
            let _ = std::fs::remove_file(&self.partial);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_frames_and_rejects_bad_options() {
        let dir = std::env::temp_dir().join(format!("lcc-seq-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("t.mp4");
        assert!(SequenceWriter::create(&out, ClipFormat::H264Mp4, 80, 15, 16, 24, 1).is_err());
        assert!(SequenceWriter::create(&out, ClipFormat::H264Mp4, 80, 16, 16, 0, 1).is_err());
        assert!(SequenceWriter::create(&out, ClipFormat::H264Mp4, 80, 16, 16, 1000, 1).is_err());
        let mut w = SequenceWriter::create(&out, ClipFormat::H264Mp4, 80, 32, 16, 24, 1).unwrap();
        assert!(w.push_encoded(&Rgb32f::from_fn(16, 16, |_, _| [0.5; 3])).is_err());
        for i in 0..5 {
            w.push_encoded(&Rgb32f::from_fn(32, 16, |_, _| [i as f32 / 5.0; 3])).unwrap();
        }
        let r = w.finish().unwrap();
        assert_eq!(r.frames, 5);
        assert_eq!(lcc_video::FrameReader::open(&out).unwrap().frame_times().len(), 5);
        assert!(!dir.join("t.mp4.partial").exists());
        // dropped unfinished: nothing left
        let out2 = dir.join("u.mov");
        let mut w = SequenceWriter::create(&out2, ClipFormat::ProResMov, 50, 32, 16, 25, 1).unwrap();
        w.push_encoded(&Rgb32f::from_fn(32, 16, |_, _| [0.5; 3])).unwrap();
        drop(w);
        assert!(!out2.exists() && !dir.join("u.mov.partial").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn muxes_an_audio_track() {
        let dir = std::env::temp_dir().join(format!("lcc-seq-audio-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let no = std::sync::atomic::AtomicBool::new(false);
        let pcm: Vec<f32> = (0..48_000 * 2).map(|i| ((i / 2) as f32 * 0.05).sin() * 0.4).collect();
        for (name, format) in [("a.mp4", ClipFormat::H264Mp4), ("a.mov", ClipFormat::ProResMov)] {
            let out = dir.join(name);
            let track = crate::encode_pcm_track(&pcm, 2, 48_000, format, &no).unwrap();
            let mut w = SequenceWriter::create_with_audio(&out, format, 60, 32, 16, 10, 1, Some(track)).unwrap();
            for _ in 0..10 {
                w.push_encoded(&Rgb32f::from_fn(32, 16, |_, _| [0.3; 3])).unwrap();
            }
            let r = w.finish().unwrap();
            assert!(r.has_audio, "{name}");
            let a = lcc_video::AudioReader::open(&out).unwrap();
            assert_eq!(a.info().channels, 2, "{name}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A partial file a crashed export left behind is reused, not an error.
    #[test]
    fn a_stale_partial_file_is_reused() {
        let dir = std::env::temp_dir().join(format!("lcc-seq-stale-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("t.mp4");
        std::fs::write(dir.join("t.mp4.partial"), b"leftover").unwrap();
        let w = SequenceWriter::create(&out, ClipFormat::H264Mp4, 80, 32, 16, 24, 1).unwrap();
        assert_eq!(w.partial, dir.join("t.mp4.partial"));
        drop(w);
        assert!(!dir.join("t.mp4.partial").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Two exports to one output write separate partial files; neither truncates the other's.
    #[test]
    fn a_partial_file_in_use_gets_a_second_name() {
        let dir = std::env::temp_dir().join(format!("lcc-seq-busy-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("t.mp4");
        let mut a = SequenceWriter::create(&out, ClipFormat::H264Mp4, 80, 32, 16, 24, 1).unwrap();
        let b = SequenceWriter::create(&out, ClipFormat::H264Mp4, 80, 32, 16, 24, 1).unwrap();
        assert_eq!(a.partial, dir.join("t.mp4.partial"));
        assert_eq!(b.partial, dir.join("t.mp4.2.partial"));
        drop(b);
        let img = Rgb32f::new(32, 16);
        a.push_encoded(&img).unwrap();
        a.finish().unwrap();
        assert!(out.exists() && !dir.join("t.mp4.partial").exists() && !dir.join("t.mp4.2.partial").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn finish_with_no_frames_removes_partial_file() {
        let dir = std::env::temp_dir().join(format!("lcc-seq-noframes-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("t.mp4");
        let w = SequenceWriter::create(&out, ClipFormat::H264Mp4, 80, 32, 16, 24, 1).unwrap();
        // Don't push any frames
        let result = w.finish();
        assert!(result.is_err(), "expected error on no frames");
        // The partial file should be removed (not left behind)
        let partial = dir.join("t.mp4.partial");
        assert!(!partial.exists(), "partial file should be removed on finish() error");
        assert!(!out.exists(), "output file should not exist on error");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A failure after the last frame (here the rename onto a folder) leaves no partial file behind.
    #[test]
    fn finish_failing_late_removes_partial_file() {
        let dir = std::env::temp_dir().join(format!("lcc-seq-late-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("t.mp4");
        std::fs::create_dir_all(&out).unwrap();
        let mut w = SequenceWriter::create(&out, ClipFormat::H264Mp4, 80, 32, 16, 24, 1).unwrap();
        w.push_encoded(&Rgb32f::new(32, 16)).unwrap();
        assert!(w.finish().is_err());
        assert!(!dir.join("t.mp4.partial").exists(), "partial file should be removed on a late finish() error");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
