//! `ui.render {inline: true}` returns the PNG in the reply, so an MCP server on another machine
//! (`lightcraft-cli mcp --connect host:port`) can show a render it could never read from a path.

use serde_json::{Value, json};

use crate::control::{ControlRequest, Outcome, handle};
use crate::{LightcraftApp, Services};

fn render(app: &mut LightcraftApp, params: Value) -> Value {
    let ctx = egui::Context::default();
    let (req, _) = ControlRequest::new("ui.render", params);
    match handle(app, &ctx, &req) {
        Outcome::Done(reply) => reply,
        _ => panic!("expected a reply"),
    }
}

#[test]
fn inline_render_returns_the_png_in_the_reply() {
    let png = Services { png: Some(Box::new(|img: &lcc_raster::Rgba8| vec![img.width as u8, 1, 2])), ..Default::default() };
    let mut app = LightcraftApp::new(lcc_engine::Session::with_demo(), png);
    let reply = render(&mut app, json!({"size": 64, "inline": true}));
    assert_eq!(reply["ok"], true, "{reply}");
    let (w, h) = (reply["result"]["width"].as_u64().unwrap(), reply["result"]["height"].as_u64().unwrap());
    assert_eq!(w.max(h), 64, "{reply}");
    assert_eq!(reply["result"]["png"], lcc_engine::base64::encode(&[w as u8, 1, 2]), "{reply}");
    // without `inline` the reply stays small
    let plain = render(&mut app, json!({"size": 64}));
    assert!(plain["result"].get("png").is_none(), "{plain}");
}

#[test]
fn inline_render_without_an_encoder_is_an_error() {
    let mut app = LightcraftApp::new(lcc_engine::Session::with_demo(), Services { png: None, ..Default::default() });
    let reply = render(&mut app, json!({"size": 64, "inline": true}));
    assert_eq!(reply["ok"], false, "{reply}");
}
