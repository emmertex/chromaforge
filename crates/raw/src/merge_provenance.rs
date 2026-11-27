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

    /// Read the block from an XMP packet; `None` when the packet has none or it is unusable.
    pub fn from_packet(packet: &str) -> Option<Provenance> {
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
        // a file from before registration existed has no Method
        let registration = attr("Method").map(|m| Registration {
            registered: m == "registered",
            verdict: attr("GateVerdict"),
            reason: attr("Reason"),
            gains: attr("GainsApplied").is_some_and(|g| g == "true"),
            tiles: num("Tiles").and_then(|t| u32::try_from(t).ok()).unwrap_or(0),
            made_at: attr("MergedAt"),
            models: attr("Models")
                .map(|t| {
                    t.split(';')
                        .filter_map(|m| {
                            let v: Vec<f32> = m.split_whitespace().filter_map(|x| x.parse().ok()).collect();
                            <[f32; 4]>::try_from(v).ok()
                        })
                        .collect()
                })
                .unwrap_or_default(),
        });
        Some(Provenance {
            engine: num("Engine")? as u32,
            shot_count: u16::try_from(num("ShotCount")?).ok()?,
            reference: u16::try_from(num("Reference")?).ok()?,
            scale: u16::try_from(num("Scale")?).ok()?,
            window,
            frames,
            registration,
        })
    }
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
    Provenance::from_packet(&String::from_utf8_lossy(packet))
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
            reason: Some("no <texture> & \"such\"".into()),
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
}
