//! Where a pixel-shift merge came from, as an XMP block inside the merged DNG.
//!
//! The block lives in the DNG's own XMP packet (a namespace of its own, [`ns`], prefix `lcps`), so it travels with the
//! file: a copy, a move or a re-import elsewhere keeps it. It says which frames were merged and how, and where the
//! stored frames sat on the camera's sensor, which a later Vignetting Calibration needs (the DNG has no maker note).
//!
//! ```xml
//! <rdf:Description rdf:about="" xmlns:lcps="<homepage>/ns/pixelshift/1.0/"
//!     lcps:Engine="1" lcps:ShotCount="16" lcps:Reference="1" lcps:Scale="2"
//!     lcps:SensorWidth="8280" lcps:SensorHeight="5520"
//!     lcps:WindowX="0" lcps:WindowY="0" lcps:WindowWidth="8280" lcps:WindowHeight="5520">
//!   <lcps:Frames><rdf:Seq>
//!     <rdf:li rdf:parseType="Resource"><lcps:Index>1</lcps:Index><lcps:FileName>…</lcps:FileName>
//!       <lcps:Captured>…</lcps:Captured><lcps:Counter>…</lcps:Counter></rdf:li> …
//!   </rdf:Seq></lcps:Frames>
//! </rdf:Description>
//! ```
//!
//! **Forward compatibility.** A file is read by builds older and newer than the one that wrote it. Only the block's own
//! structural limits make a block invalid (the packet size, the shot count, the frame list and its numbers, the scale,
//! the strings' lengths, the sensor window; see [`Provenance::from_packet`]). A value a newer version may have added or
//! changed (the `Method`, `GateVerdict` or `Reason` text, the `Engine` number) that this version does not know reads as
//! unknown or none, and never makes the block, the file or the library unreadable: the frames are still found.
//!
//! `Index` is the frame's number in the series (1-based) and `Reference` the number of the frame the merged image
//! belongs to (always 1: the frame whose metadata, colour and lens data the merge carries). `Scale` is merged
//! pixels per sensor pixel. The `Window*` fields are the stored frame's place on the sensor (as
//! [`crate::sensor_window::SensorWindow`]); the merged image covers that window at `Scale` times its resolution.

/// The XMP namespace of the block, under the app's homepage like the other namespaces of its own (`<homepage>/ns/pixelshift/1.0/`).
pub fn ns() -> String {
    format!("{}/ns/pixelshift/1.0/", lcc_brand::HOMEPAGE.trim_end_matches('/'))
}
/// What the merge engine's output is called in the merged file's metadata (`xmp:CreatorTool`), after the app's name.
pub const SOFTWARE_SUFFIX: &str = "Pixel Shift Merge";
/// Version of the merge engine that wrote the file (bumped when its output changes).
pub const ENGINE: u32 = 1;
/// The engine version of a registered merge (positions measured and the image reconstructed from them); the plain merge
/// keeps [`ENGINE`].
pub const ENGINE_REGISTERED: u32 = 2;

/// The most XMP the reader looks at: a bigger packet is no block (a merge writes a few kilobytes).
pub const MAX_PACKET_BYTES: usize = 1 << 20;
/// The shot counts of a Nikon pixel-shift series.
pub const SHOT_COUNTS: [u16; 4] = [4, 8, 16, 32];
/// The most frames a block lists: the largest shot count.
pub const MAX_FRAMES: usize = 32;
/// The longest file name and capture time a block may hold, in bytes.
pub const MAX_NAME_BYTES: usize = 512;
pub const MAX_CAPTURED_BYTES: usize = 64;
/// The largest merged pixels per sensor pixel a block may say, and the largest sensor side its window may name.
pub const MAX_SCALE: u16 = 8;
pub const MAX_SENSOR_SIDE: usize = 65_535;
/// The gate verdicts a merge writes (`GateVerdict`); any other value reads as no verdict.
pub const VERDICTS: [&str; 3] = ["better", "equal", "worse"];
/// The reasons a merge was plain that this version knows (`Reason`); any other value (a newer version's) reads as none.
pub const REASONS: [&str; 5] = ["noTexture", "tooFewTiles", "noMatch", "measuredWorse", "memory"];
/// The most tiles a block may claim (a series has at most 96); a larger number reads as 0.
pub const MAX_TILES: u32 = 100_000;
/// The longest `MergedAt` text read (an ISO 8601 time with milliseconds is 24 characters).
pub const MAX_MADE_AT_BYTES: usize = 40;
/// What a frame's model may say: a shift in sensor sites, a rotation in radians and a scale. Real merges stay far inside
/// (tens of sites, 1e-4 radians, 1e-3); a block outside them has no models.
pub const MAX_MODEL_SHIFT: f32 = 1000.0;
pub const MAX_MODEL_TURN: f32 = 0.1;

/// One merged frame.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SourceFrame {
    /// Number in the series, 1-based.
    pub index: u16,
    pub file_name: String,
    /// Capture time as the metadata gives it (ISO 8601, local), empty when unknown.
    pub captured: String,
    /// The frame's exposure counter (maker note).
    pub counter: u32,
}

/// The stored frames' place on the sensor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Window {
    pub sensor: [usize; 2],
    pub origin: [usize; 2],
    pub size: [usize; 2],
}

/// How the image was merged: by registration (the frames' positions measured on the data) or plainly (the camera's own
/// pattern). Written in the same block, additively: a reader that does not know it ignores it.
///
/// ```xml
/// lcps:Method="registered" lcps:GateVerdict="better" lcps:GainsApplied="true" lcps:Tiles="87"
/// lcps:Models="0 0 0 0;0.78 0.05 1.2e-6 -3.1e-6;…"   (per frame: shift x, shift y, rotation, scale)
/// lcps:Method="plain" lcps:Reason="noTexture" lcps:GainsApplied="false"
/// lcps:MergedAt="2026-11-25T10:20:30.123Z"   (every merge of a series is its own file)
/// ```
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Registration {
    /// The image is the registered reconstruction (false: the plain merge).
    pub registered: bool,
    /// The hold-out test of the measured positions against the camera's own pattern: `better`, `equal` or `worse`.
    pub verdict: Option<String>,
    /// Why the plain merge was used, when it was used for a reason (a short code such as `noTexture`).
    pub reason: Option<String>,
    /// Per-frame brightness gains were applied.
    pub gains: bool,
    /// Tiles the positions rest on.
    pub tiles: u32,
    /// Each frame's shift (x, y in sensor sites), rotation (radians) and scale against frame 1; empty for a plain merge.
    pub models: Vec<[f32; 4]>,
    /// When the merge was made (UTC, ISO 8601 with milliseconds). It makes every merge of a series its own file: two
    /// merges of the same series with the same method are never the same bytes.
    pub made_at: Option<String>,
}

/// The provenance block.
#[derive(Clone, Debug, PartialEq)]
pub struct Provenance {
    pub engine: u32,
    pub shot_count: u16,
    pub reference: u16,
    pub scale: u16,
    pub window: Option<Window>,
    pub frames: Vec<SourceFrame>,
    /// How it was merged; `None` in a file from before registration existed.
    pub registration: Option<Registration>,
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&amp;", "&")
}

impl Provenance {
    /// The `rdf:Description` element, to go inside an XMP packet's `rdf:RDF`.
    pub fn to_description(&self) -> String {
        let ns = ns();
        let mut s = format!(
            "<rdf:Description rdf:about=\"\" xmlns:lcps=\"{ns}\" lcps:Engine=\"{}\" lcps:ShotCount=\"{}\" lcps:Reference=\"{}\" lcps:Scale=\"{}\"",
            self.engine, self.shot_count, self.reference, self.scale
        );
        if let Some(w) = &self.window {
            s += &format!(
                " lcps:SensorWidth=\"{}\" lcps:SensorHeight=\"{}\" lcps:WindowX=\"{}\" lcps:WindowY=\"{}\" lcps:WindowWidth=\"{}\" lcps:WindowHeight=\"{}\"",
                w.sensor[0], w.sensor[1], w.origin[0], w.origin[1], w.size[0], w.size[1]
            );
        }
        if let Some(r) = &self.registration {
            s += &format!(
                " lcps:Method=\"{}\" lcps:GainsApplied=\"{}\" lcps:Tiles=\"{}\"",
                if r.registered { "registered" } else { "plain" },
                r.gains,
                r.tiles
            );
            if let Some(v) = &r.verdict {
                s += &format!(" lcps:GateVerdict=\"{}\"", escape(v));
            }
            if let Some(v) = &r.reason {
                s += &format!(" lcps:Reason=\"{}\"", escape(v));
            }
            if let Some(t) = &r.made_at {
                s += &format!(" lcps:MergedAt=\"{}\"", escape(t));
            }
            if !r.models.is_empty() {
                let models: Vec<String> = r.models.iter().map(|m| m.iter().map(f32::to_string).collect::<Vec<_>>().join(" ")).collect();
                s += &format!(" lcps:Models=\"{}\"", models.join(";"));
            }
        }
        s += "><lcps:Frames><rdf:Seq>";
        for f in &self.frames {
            s += &format!(
                "<rdf:li rdf:parseType=\"Resource\"><lcps:Index>{}</lcps:Index><lcps:FileName>{}</lcps:FileName><lcps:Captured>{}</lcps:Captured><lcps:Counter>{}</lcps:Counter></rdf:li>",
                f.index,
                escape(&f.file_name),
                escape(&f.captured),
                f.counter
            );
        }
        s + "</rdf:Seq></lcps:Frames></rdf:Description>"
    }

    /// `packet` (an XMP packet) with this block added inside its `rdf:RDF`; the packet unchanged when it has none.
    pub fn add_to_packet(&self, packet: &str) -> String {
        match packet.rfind("</rdf:RDF>") {
            Some(at) => format!("{}{}{}", &packet[..at], self.to_description(), &packet[at..]),
            None => packet.to_string(),
        }
    }

    /// Read the block from an XMP packet; `None` when the packet has none or it is unusable. A file is untrusted input:
    /// a packet over [`MAX_PACKET_BYTES`], a block that breaks one of the limits above (shot count, frame list, indices,
    /// strings, scale, window) or is malformed is no block at all, so the file reads as a normal DNG.
    pub fn from_packet(packet: &str) -> Option<Provenance> {
        if packet.len() > MAX_PACKET_BYTES {
            return None;
        }
        let at = packet.find("xmlns:lcps=")?;
        let start = packet[..at].rfind("<rdf:Description")?;
        let end = start + packet[start..].find("</rdf:Description>")?;
        let d = &packet[start..end];
        let head_end = d.find('>')?;
        let head = &d[..head_end];
        let attr = |name: &str| -> Option<String> {
            let key = format!("lcps:{name}=\"");
            let i = head.find(&key)? + key.len();
            Some(unescape(&head[i..i + head[i..].find('"')?]))
        };
        let num = |name: &str| attr(name)?.parse::<usize>().ok();
        let window = (|| {
            Some(Window {
                sensor: [num("SensorWidth")?, num("SensorHeight")?],
                origin: [num("WindowX")?, num("WindowY")?],
                size: [num("WindowWidth")?, num("WindowHeight")?],
            })
        })();
        let mut frames = Vec::new();
        let mut rest = &d[head_end..];
        while let Some(i) = rest.find("<rdf:li") {
            // (no series has more frames than its largest shot count: more is not a block, and the loop is bounded)
            if frames.len() >= MAX_FRAMES {
                return None;
            }
            let j = i + rest[i..].find("</rdf:li>")?;
            let item = &rest[i..j];
            let tag = |name: &str| -> Option<String> {
                let open = format!("<lcps:{name}>");
                let a = item.find(&open)? + open.len();
                let b = a + item[a..].find(&format!("</lcps:{name}>"))?;
                Some(unescape(&item[a..b]))
            };
            frames.push(SourceFrame {
                index: tag("Index")?.parse().ok()?,
                file_name: tag("FileName")?,
                captured: tag("Captured").unwrap_or_default(),
                counter: tag("Counter")?.parse().ok()?,
            });
            rest = &rest[j..];
        }
        let shot_count = u16::try_from(num("ShotCount")?).ok()?;
        // how it was merged: what this version knows is read, anything else reads as unknown (see the rule above)
        let registration = read_registration(&attr, shot_count);
        let p = Provenance {
            engine: attr("Engine").and_then(|e| e.parse::<u32>().ok()).unwrap_or(0),
            shot_count,
            reference: u16::try_from(num("Reference")?).ok()?,
            scale: u16::try_from(num("Scale")?).ok()?,
            window,
            frames,
            registration,
        };
        p.is_sane().then_some(p)
    }

    /// [`Provenance::from_packet`] on the bytes of a packet (lossy UTF-8; nothing is allocated for one over the limit).
    pub fn from_bytes(packet: &[u8]) -> Option<Provenance> {
        if packet.len() > MAX_PACKET_BYTES {
            return None;
        }
        Provenance::from_packet(&String::from_utf8_lossy(packet))
    }

    /// Whether the block says things a merge could have written: a shot count of a Nikon series, a reference frame and
    /// frame numbers inside it (each once, no more frames than shots), a scale of 1 to [`MAX_SCALE`], short strings, and
    /// a sensor window that lies inside its sensor.
    fn is_sane(&self) -> bool {
        if !SHOT_COUNTS.contains(&self.shot_count) || !(1..=self.shot_count).contains(&self.reference) || !(1..=MAX_SCALE).contains(&self.scale) {
            return false;
        }
        if self.frames.len() > usize::from(self.shot_count) {
            return false;
        }
        let mut seen = 0u64;
        for f in &self.frames {
            if !(1..=self.shot_count).contains(&f.index)
                || f.file_name.is_empty()
                || f.file_name.len() > MAX_NAME_BYTES
                || f.captured.len() > MAX_CAPTURED_BYTES
            {
                return false;
            }
            let bit = 1u64 << (f.index - 1);
            if seen & bit != 0 {
                return false;
            }
            seen |= bit;
        }
        self.window.is_none_or(|w| {
            let inside = |at: usize, size: usize, sensor: usize| size > 0 && at.checked_add(size).is_some_and(|end| end <= sensor);
            (1..=MAX_SENSOR_SIDE).contains(&w.sensor[0])
                && (1..=MAX_SENSOR_SIDE).contains(&w.sensor[1])
                && inside(w.origin[0], w.size[0], w.sensor[0])
                && inside(w.origin[1], w.size[1], w.sensor[1])
        })
    }
}

/// How the image was merged, from the attributes of a block: what this version knows is read; anything else (a newer
/// version's method, verdict or reason, an out-of-range number, malformed text) reads as unknown or none and never
/// makes the block, the file or the library unreadable (the forward-compatibility rule in the module docs). `None` when
/// the block has no `Method` or one this version does not know: how it was made is then not known.
fn read_registration(attr: &dyn Fn(&str) -> Option<String>, shot_count: u16) -> Option<Registration> {
    let registered = match attr("Method")?.as_str() {
        "registered" => true,
        "plain" => false,
        _ => return None,
    };
    let known = |name: &str, set: &[&str]| attr(name).filter(|v| set.contains(&v.as_str()));
    Some(Registration {
        registered,
        verdict: known("GateVerdict", &VERDICTS),
        reason: known("Reason", &REASONS),
        gains: attr("GainsApplied").is_some_and(|g| g == "true"),
        tiles: attr("Tiles").and_then(|t| t.parse::<u32>().ok()).filter(|t| *t <= MAX_TILES).unwrap_or(0),
        made_at: attr("MergedAt").filter(|t| plausible_time(t)),
        models: attr("Models").map(|t| read_models(&t, shot_count)).unwrap_or_default(),
    })
}

/// A time as a merge writes it: ISO 8601 characters only, short.
fn plausible_time(t: &str) -> bool {
    !t.is_empty() && t.len() <= MAX_MADE_AT_BYTES && t.bytes().all(|b| b.is_ascii_digit() || matches!(b, b'-' | b':' | b'.' | b'T' | b'Z' | b'+'))
}

/// The per-frame models of a block: at most one per shot, each four finite numbers in range; one that is not makes it
/// none at all (a model set that does not hold together is not used piecemeal).
fn read_models(text: &str, shot_count: u16) -> Vec<[f32; 4]> {
    let mut models = Vec::new();
    for entry in text.split(';') {
        if models.len() >= usize::from(shot_count) {
            return Vec::new();
        }
        let values: Vec<f32> = entry.split_whitespace().filter_map(|x| x.parse().ok()).collect();
        let Ok(m) = <[f32; 4]>::try_from(values) else { return Vec::new() };
        if !m.iter().all(|v| v.is_finite())
            || m[0].abs() > MAX_MODEL_SHIFT
            || m[1].abs() > MAX_MODEL_SHIFT
            || m[2].abs() > MAX_MODEL_TURN
            || m[3].abs() > MAX_MODEL_TURN
        {
            return Vec::new();
        }
        models.push(m);
    }
    models
}

/// Whether an XMP packet carries the block (cheap: no parsing).
pub fn is_marked(packet: &[u8]) -> bool {
    let ns = ns();
    let needle = ns.as_bytes();
    packet.windows(needle.len()).any(|w| w == needle)
}

/// The block of a merged DNG (`bytes` is the whole file, or at least its headers and XMP).
pub fn from_dng(bytes: &[u8]) -> Option<Provenance> {
    let tiff = lcc_tiff::Tiff::parse(bytes).ok()?;
    let packet = tiff.ifds.first()?.bytes(lcc_tiff::tags::XMP)?;
    Provenance::from_bytes(packet)
}

/// Whether an XMP packet carries a usable block: marked, within the limits and well formed. This (not
/// [`is_marked`] alone) is what makes a DNG a merge for the reader.
pub fn is_valid(packet: &[u8]) -> bool {
    is_marked(packet) && Provenance::from_bytes(packet).is_some()
}

/// The `xmp:CreatorTool` a merged file gets and is recognised by.
pub fn software() -> String {
    format!("{} {SOFTWARE_SUFFIX}", lcc_brand::DISPLAY_NAME)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Provenance {
        Provenance {
            engine: ENGINE,
            shot_count: 4,
            reference: 1,
            scale: 2,
            window: Some(Window { sensor: [800, 600], origin: [100, 0], size: [600, 600] }),
            frames: (1..=4)
                .map(|i| SourceFrame {
                    index: i,
                    file_name: format!("test <{i}> & \"x\".nef"),
                    captured: format!("2030-01-02T03:04:0{i}"),
                    counter: 900 + u32::from(i),
                })
                .collect(),
            registration: None,
        }
    }

    #[test]
    fn how_the_image_was_merged_is_written_additively_and_read_back() {
        let packet = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"><rdf:Description rdf:about=\"\"/></rdf:RDF></x:xmpmeta>";
        let registered = Registration {
            registered: true,
            verdict: Some("better".into()),
            reason: None,
            gains: true,
            tiles: 87,
            models: vec![[0.0, 0.0, 0.0, 0.0], [0.785_432, -0.051_7, 1.25e-6, -3.5e-6]],
            made_at: Some("2030-01-02T03:04:05.678Z".into()),
        };
        let plain = Registration {
            registered: false,
            verdict: None,
            reason: Some("noTexture".into()),
            gains: false,
            tiles: 0,
            models: vec![],
            made_at: None,
        };
        for r in [registered, plain] {
            let p = Provenance { registration: Some(r), ..sample() };
            assert_eq!(Provenance::from_packet(&p.add_to_packet(packet)), Some(p));
        }
        // a block without the attributes (a file from before) reads as no registration, and the frames are as ever
        let old = sample().add_to_packet(packet);
        assert!(!old.contains("lcps:Method"));
        assert_eq!(Provenance::from_packet(&old).unwrap().registration, None);
        // a reader of the older kind ignores the new attributes: they are plain attributes of the same description
        let new = Provenance {
            registration: Some(Registration { registered: true, models: vec![[1.0, 2.0, 3.0, 4.0]], ..Default::default() }),
            ..sample()
        };
        assert!(new.to_description().starts_with("<rdf:Description") && new.to_description().contains(" lcps:Models=\"1 2 3 4\""));
        // 16 frames of models stay small
        let sixteen =
            Registration { registered: true, models: vec![[0.785_432_1, -0.051_734_5, 1.254_321e-6, -3.543_21e-6]; 16], ..Default::default() };
        let text = Provenance { registration: Some(sixteen), ..sample() }.to_description();
        let models = text.split("lcps:Models=\"").nth(1).and_then(|t| t.split('"').next()).unwrap_or_default();
        assert!(models.len() < 1000, "{}", models.len());
    }

    #[test]
    fn the_block_round_trips_through_a_packet() {
        let packet = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"><rdf:Description rdf:about=\"\"/></rdf:RDF></x:xmpmeta>";
        let p = sample();
        let with = p.add_to_packet(packet);
        assert!(is_marked(with.as_bytes()) && !is_marked(packet.as_bytes()));
        assert_eq!(Provenance::from_packet(&with), Some(p.clone()));
        // no window, no frames
        let bare = Provenance { window: None, frames: vec![], ..p };
        assert_eq!(Provenance::from_packet(&bare.add_to_packet(packet)), Some(bare));
        assert_eq!(Provenance::from_packet(packet), None);
        assert_eq!(sample().add_to_packet("no rdf here"), "no rdf here");
    }

    const PACKET: &str = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"><rdf:Description rdf:about=\"\"/></rdf:RDF></x:xmpmeta>";

    fn valid(shots: u16) -> Provenance {
        Provenance {
            engine: ENGINE,
            shot_count: shots,
            reference: 1,
            scale: 2,
            window: Some(Window { sensor: [800, 600], origin: [100, 0], size: [600, 600] }),
            frames: (1..=shots)
                .map(|i| SourceFrame {
                    index: i,
                    file_name: format!("frame {i}.nef"),
                    captured: format!("2030-01-02T03:04:{i:02}"),
                    counter: 900 + u32::from(i),
                })
                .collect(),
            registration: None,
        }
    }

    fn read(p: &Provenance) -> Option<Provenance> {
        Provenance::from_packet(&p.add_to_packet(PACKET))
    }

    /// Every shot count of a Nikon series reads, with the largest names and strings the block allows.
    #[test]
    fn a_block_at_its_limits_reads() {
        for shots in SHOT_COUNTS {
            assert_eq!(read(&valid(shots)).as_ref(), Some(&valid(shots)), "{shots} shots");
        }
        let mut p = valid(32);
        p.frames[0].file_name = "n".repeat(MAX_NAME_BYTES);
        p.frames[0].captured = "c".repeat(MAX_CAPTURED_BYTES);
        p.scale = MAX_SCALE;
        p.window = Some(Window { sensor: [MAX_SENSOR_SIDE, MAX_SENSOR_SIDE], origin: [0, 0], size: [MAX_SENSOR_SIDE, MAX_SENSOR_SIDE] });
        assert_eq!(read(&p), Some(p));
    }

    /// A block that says something no merge writes is no block: the file reads as the DNG it is.
    #[test]
    fn a_block_that_breaks_a_limit_is_no_block() {
        type Change = Box<dyn Fn(&mut Provenance)>;
        let window = |sensor: [usize; 2], origin: [usize; 2], size: [usize; 2]| Some(Window { sensor, origin, size });
        let breaks: Vec<(&str, Change)> = vec![
            ("shot count 0", Box::new(|p| p.shot_count = 0)),
            ("shot count 3", Box::new(|p| p.shot_count = 3)),
            ("shot count 17", Box::new(|p| p.shot_count = 17)),
            ("shot count 65535", Box::new(|p| p.shot_count = u16::MAX)),
            ("reference 0", Box::new(|p| p.reference = 0)),
            ("reference past the shots", Box::new(|p| p.reference = 5)),
            ("scale 0", Box::new(|p| p.scale = 0)),
            ("scale 9", Box::new(|p| p.scale = MAX_SCALE + 1)),
            (
                "more frames than shots",
                Box::new(|p| p.frames.push(SourceFrame { index: 1, file_name: "x".into(), captured: String::new(), counter: 1 })),
            ),
            ("frame 0", Box::new(|p| p.frames[0].index = 0)),
            ("frame past the shots", Box::new(|p| p.frames[0].index = 5)),
            ("frame twice", Box::new(|p| p.frames[1].index = 1)),
            ("no file name", Box::new(|p| p.frames[0].file_name.clear())),
            ("file name too long", Box::new(|p| p.frames[0].file_name = "n".repeat(MAX_NAME_BYTES + 1))),
            ("capture time too long", Box::new(|p| p.frames[0].captured = "c".repeat(MAX_CAPTURED_BYTES + 1))),
            ("window of nothing", Box::new(move |p| p.window = window([800, 600], [0, 0], [0, 600]))),
            ("window past its sensor", Box::new(move |p| p.window = window([800, 600], [300, 0], [600, 600]))),
            ("window at the end of numbers", Box::new(move |p| p.window = window([800, 600], [usize::MAX, 0], [600, 600]))),
            ("window size at the end of numbers", Box::new(move |p| p.window = window([800, 600], [1, 0], [usize::MAX, 600]))),
            ("sensor of nothing", Box::new(move |p| p.window = window([0, 0], [0, 0], [0, 0]))),
            ("sensor bigger than any", Box::new(move |p| p.window = window([MAX_SENSOR_SIDE + 1, 600], [0, 0], [600, 600]))),
        ];
        for (what, change) in breaks {
            let mut p = valid(4);
            change(&mut p);
            assert_eq!(read(&p), None, "{what}");
            // (the same block in a DNG's XMP makes no merge either)
            assert!(!is_valid(p.add_to_packet(PACKET).as_bytes()), "{what}");
        }
        assert!(is_valid(valid(4).add_to_packet(PACKET).as_bytes()));
    }

    /// Text that is not the block, or damages it, is no block and no panic: missing and extra fields, wrong types,
    /// negative and huge numbers, truncation, unclosed tags.
    #[test]
    fn a_damaged_block_is_no_block() {
        let good = valid(4).add_to_packet(PACKET);
        let cut = good.find("<lcps:Frames>").unwrap_or(0) + 20;
        for (what, text) in [
            ("no shot count", good.replace("lcps:ShotCount=\"4\"", "")),
            ("no reference", good.replace("lcps:Reference=\"1\"", "")),
            ("no scale", good.replace("lcps:Scale=\"2\"", "")),
            ("a word for a number", good.replace("lcps:ShotCount=\"4\"", "lcps:ShotCount=\"four\"")),
            ("a negative number", good.replace("lcps:Scale=\"2\"", "lcps:Scale=\"-2\"")),
            ("a fraction", good.replace("lcps:Scale=\"2\"", "lcps:Scale=\"2.5\"")),
            ("a number with an exponent", good.replace("lcps:Reference=\"1\"", "lcps:Reference=\"1e9\"")),
            ("a number past any size", good.replace("lcps:Reference=\"1\"", "lcps:Reference=\"99999999999999999999999\"")),
            ("an empty number", good.replace("lcps:Reference=\"1\"", "lcps:Reference=\"\"")),
            ("a frame without its number", good.replace("<lcps:Index>1</lcps:Index>", "")),
            ("a frame without its counter", good.replace("<lcps:Counter>901</lcps:Counter>", "")),
            ("a frame number that is not one", good.replace("<lcps:Index>1</lcps:Index>", "<lcps:Index>-1</lcps:Index>")),
            ("a frame counter past any size", good.replace("<lcps:Counter>901</lcps:Counter>", "<lcps:Counter>4294967296</lcps:Counter>")),
            ("an unclosed frame", good.replace("</rdf:li>", "")),
            ("an unclosed description", good.replace("</rdf:Description>", "")),
            ("an unclosed attribute", good.replace("lcps:Scale=\"2\"", "lcps:Scale=\"2")),
            ("cut in the middle", good[..cut].to_string()),
            ("no block", PACKET.to_string()),
            ("nothing", String::new()),
        ] {
            assert_eq!(Provenance::from_packet(&text), None, "{what}");
        }
        // fields it doesn't know about are not an error
        let extra = good.replace("lcps:Scale=\"2\"", "lcps:Scale=\"2\" lcps:Unknown=\"1\" other:Thing=\"x\"");
        assert_eq!(Provenance::from_packet(&extra), Some(valid(4)));
    }

    /// A packet over the limit is not looked at, a long list of frames is refused at its limit, and neither takes
    /// work or memory beyond it.
    #[test]
    fn a_huge_packet_or_a_long_list_is_refused_quickly() {
        let big = format!("{}{}", valid(4).add_to_packet(PACKET), " ".repeat(MAX_PACKET_BYTES));
        assert_eq!(Provenance::from_packet(&big), None);
        assert_eq!(Provenance::from_bytes(big.as_bytes()), None);
        assert!(!is_valid(big.as_bytes()));
        let item =
            "<rdf:li rdf:parseType=\"Resource\"><lcps:Index>1</lcps:Index><lcps:FileName>a</lcps:FileName><lcps:Counter>1</lcps:Counter></rdf:li>";
        let many = valid(4).add_to_packet(PACKET).replace("</rdf:Seq>", &format!("{}</rdf:Seq>", item.repeat(5000)));
        assert!(many.len() < MAX_PACKET_BYTES);
        assert_eq!(Provenance::from_packet(&many), None);
        // bytes that are not UTF-8 are read lossily, and a block among them still reads
        let mut bytes = valid(4).add_to_packet(PACKET).into_bytes();
        bytes.splice(0..0, [0xff, 0xfe, 0xc3, 0x28]);
        assert_eq!(Provenance::from_bytes(&bytes), Some(valid(4)));
        assert_eq!(Provenance::from_bytes(&[0xff; 100]), None);
    }

    /// A registered block as a merge writes it, with `edit` applied to its text.
    fn registered_block(edit: impl Fn(&str) -> String) -> Option<Provenance> {
        let reg = Registration {
            registered: true,
            verdict: Some("better".into()),
            reason: None,
            gains: true,
            tiles: 54,
            models: vec![[0.0, 0.0, 0.0, 0.0], [0.78, -0.05, 1.2e-6, -3.1e-6]],
            made_at: Some("2030-01-02T03:04:05.678Z".into()),
        };
        let p = Provenance { engine: ENGINE_REGISTERED, registration: Some(reg), ..valid(16) };
        Provenance::from_packet(&edit(&p.add_to_packet(PACKET)))
    }

    /// The forward-compatibility rule: a value this version does not know reads as unknown or none, and the block, with its
    /// frames, is still there; only the structural limits make a block invalid.
    #[test]
    fn values_a_newer_version_may_write_read_as_unknown_and_never_make_the_block_unreadable() {
        let base = registered_block(str::to_string).unwrap();
        let frames = base.frames.clone();
        let reg = |e: &dyn Fn(&str) -> String| registered_block(e).expect("the block still reads").registration;
        // a method this version does not know: how it was made is unknown, the frames are found
        let r = registered_block(|t| t.replace("lcps:Method=\"registered\"", "lcps:Method=\"neural\"")).unwrap();
        assert_eq!((r.registration, r.frames), (None, frames.clone()));
        // a verdict or a reason it does not know: none; the rest stays
        let r = reg(&|t| t.replace("\"better\"", "\"excellent\"")).unwrap();
        assert_eq!((r.verdict, r.registered, r.tiles), (None, true, 54));
        let plain = Provenance {
            registration: Some(Registration { registered: false, reason: Some("noTexture".into()), ..Default::default() }),
            ..valid(16)
        };
        let read_plain = |e: &dyn Fn(&str) -> String| Provenance::from_packet(&e(&plain.add_to_packet(PACKET))).unwrap().registration.unwrap();
        assert_eq!(read_plain(&|t| t.to_string()).reason.as_deref(), Some("noTexture"));
        assert_eq!(read_plain(&|t| t.replace("noTexture", "tooBlurry")).reason, None);
        // an engine number it does not know is kept as a number; one that is not a number, or is missing, is 0 (unknown)
        assert_eq!(registered_block(|t| t.replace("lcps:Engine=\"2\"", "lcps:Engine=\"7\"")).unwrap().engine, 7);
        assert_eq!(registered_block(|t| t.replace("lcps:Engine=\"2\"", "lcps:Engine=\"x\"")).unwrap().engine, 0);
        assert_eq!(registered_block(|t| t.replace("lcps:Engine=\"2\"", "")).unwrap().engine, 0);
        // a Method that is missing: no registration either (a file from before)
        assert_eq!(registered_block(|t| t.replace("lcps:Method=\"registered\"", "")).unwrap().registration, None);
        // the structural limits still apply: a shot count nobody has is no block
        assert_eq!(registered_block(|t| t.replace("lcps:ShotCount=\"16\"", "lcps:ShotCount=\"5\"")), None);
    }

    /// Hostile registration attributes: each reads as none or its safe value, whatever its size, and never changes the frames.
    #[test]
    fn hostile_registration_attributes_read_as_none_or_a_safe_value() {
        let frames = registered_block(str::to_string).unwrap().frames;
        let with = |attr: &str, value: &str| {
            let r = registered_block(|t| {
                let key = format!("lcps:{attr}=\"");
                let i = t.find(&key).unwrap_or_else(|| panic!("{attr} is written")) + key.len();
                let j = i + t[i..].find('"').unwrap();
                format!("{}{}{}", &t[..i], value, &t[j..])
            })
            .unwrap_or_else(|| panic!("{attr} = {value:?}: the block still reads"));
            assert_eq!(r.frames, frames, "{attr}: the frames are the block's own");
            r.registration.unwrap_or_else(|| panic!("{attr} = {value:?}: the registration stays"))
        };
        // Tiles: out of range, negative, huge, text
        for bad in ["100001", "-1", "4294967296", "99999999999999999999", "many", ""] {
            assert_eq!(with("Tiles", bad).tiles, 0, "Tiles {bad:?}");
        }
        assert_eq!(with("Tiles", "100000").tiles, 100_000);
        // GainsApplied: anything but true is false
        for bad in ["yes", "1", "TRUE", ""] {
            assert!(!with("GainsApplied", bad).gains, "GainsApplied {bad:?}");
        }
        // MergedAt: long or not a time
        assert_eq!(with("MergedAt", &"2".repeat(MAX_MADE_AT_BYTES + 1)).made_at, None);
        for bad in ["", "yesterday", "<script>", "2030-01-02 03:04"] {
            assert_eq!(with("MergedAt", bad).made_at, None, "MergedAt {bad:?}");
        }
        assert_eq!(with("MergedAt", "2030-01-02T03:04:05.678+02:00").made_at.as_deref(), Some("2030-01-02T03:04:05.678+02:00"));
        // Models: too many, wrong arity, not finite, out of range, not numbers: none at all
        let ok = "0 0 0 0;0.5 -0.25 0.0001 0.0001";
        assert_eq!(with("Models", ok).models.len(), 2);
        let too_many = vec!["0 0 0 0"; 17].join(";");
        let huge_list = vec!["0 0 0 0"; 100_000].join(";");
        for bad in [
            too_many.as_str(),
            huge_list.as_str(),
            "0 0 0",
            "0 0 0 0 0",
            "0 0 0 NaN",
            "inf 0 0 0",
            "-inf 0 0 0",
            "1001 0 0 0",
            "0 -1001 0 0",
            "0 0 0.11 0",
            "0 0 0 -0.11",
            "a b c d",
            "0 0 0 0;;0 0 0 0",
            ";",
            "",
        ] {
            let m = with("Models", bad).models;
            assert!(m.is_empty(), "Models {:?} gave {m:?}", &bad[..bad.len().min(40)]);
        }
        // the models of a registered merge as written (frame 1 first, 16 entries) read back whole
        let sixteen: Vec<[f32; 4]> = (0..16).map(|k| [k as f32 * 0.5, -0.25, 1.2e-6, -3.1e-6]).collect();
        let text = sixteen.iter().map(|m| m.iter().map(f32::to_string).collect::<Vec<_>>().join(" ")).collect::<Vec<_>>().join(";");
        assert_eq!(with("Models", &text).models, sixteen);
    }
}
