//! Write a procedural ColorChecker photo as a DNG (no photograph involved), for trying the Create
//! Camera Profile dialog: `cargo run -p lcc-engine --example chart_dng -- OUT.dng [light] [serial]`.

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().collect();
    let out = args.get(1).ok_or("usage: chart_dng OUT.dng [D55|A|D65] [serial]")?;
    let light = args.get(2).map_or("D55", String::as_str);
    let serial = args.get(3).map(String::as_str);
    let corners = lcc_engine::chart_profiling::camprof::ChartCorners::from_rect(300.0, 200.0, 600.0, 400.0);
    let dng = lcc_engine::chart_profiling::demo_chart_dng_of_body(1200, 800, &corners, light, false, serial)?;
    std::fs::write(out, dng).map_err(|e| format!("{out}: {e}"))
}
