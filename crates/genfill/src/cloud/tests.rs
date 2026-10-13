//! Every provider client against the local mock server (never the internet).

use super::mock::{MockServer, mean_filler};
use super::*;

const KEY: &str = "test-key-not-real-0001";

fn request(w: usize, h: usize, prompt: &str, n: u32) -> Request {
    let rgb: Vec<u8> = (0..w * h).flat_map(|i| [(i % w * 255 / w) as u8, 120, 60]).collect();
    let mask: Vec<u8> = (0..w * h).map(|i| if (i % w).abs_diff(w / 2) < w / 8 && (i / w).abs_diff(h / 2) < h / 8 { 255 } else { 0 }).collect();
    Request { width: w, height: h, rgb, mask, prompt: prompt.into(), variations: n, seed: 3 }
}

fn ctl(cancel: &AtomicBool) -> Control<'_> {
    Control { cancel, stall: Duration::from_secs(10), deadline: Duration::from_secs(30) }
}

#[test]
fn every_provider_round_trips_through_the_mock() {
    let mock = MockServer::start(0, Some(KEY), mean_filler()).unwrap();
    let at = Endpoint::mock(&mock.base).unwrap();
    let cancel = AtomicBool::new(false);
    for p in ALL {
        for prompt in ["", "green lawn"] {
            let r = request(96, 64, prompt, 2);
            let out = generate(p, KEY, &at, &r, &ctl(&cancel)).unwrap_or_else(|e| panic!("{p:?} {prompt:?}: {e}"));
            assert_eq!(out.len(), 2, "{p:?}");
            for img in &out {
                assert_eq!(img.rgb.len(), img.width * img.height * 3);
                if p == Provider::OpenAi {
                    assert_eq!((img.width, img.height), (1536, 1024), "OpenAI answers in the requested size");
                } else {
                    assert_eq!((img.width, img.height), (96, 64));
                    // the hole was filled (the mean of the kept pixels), the rest kept
                    let c = (32 * 96 + 48) * 3;
                    assert_ne!(img.rgb[c..c + 3], r.rgb[c..c + 3]);
                    assert_eq!(img.rgb[..3], r.rgb[..3]);
                }
            }
        }
    }
    let seen = mock.seen();
    assert!(seen.iter().all(|s| s.authorized || s.path.starts_with("/mock/sample/")));
    let openai = seen.iter().find(|s| s.path == "/v1/images/edits").unwrap();
    for f in ["model", "prompt", "n", "size", "quality", "input_fidelity", "output_format", "image", "mask"] {
        assert!(openai.fields.iter().any(|x| x == f), "OpenAI field {f}: {:?}", openai.fields);
    }
    assert!(seen.iter().any(|s| s.path == "/v2beta/stable-image/edit/erase"));
    assert!(seen.iter().any(|s| s.path == "/v2beta/stable-image/edit/inpaint" && s.fields.iter().any(|f| f == "prompt")));
    assert!(seen.iter().any(|s| s.path == "/v1/flux-tools/erase-v1"));
    assert!(seen.iter().any(|s| s.path == "/v1/flux-pro-1.0-fill" && s.fields.iter().any(|f| f == "guidance")));
    assert!(seen.iter().any(|s| s.path == "/fal-ai/object-removal/mask"));
    // fal is asked not to store the request
    assert!(seen.iter().filter(|s| s.method == "POST" && s.path.starts_with("/fal-ai/")).all(|s| s.store_io.as_deref() == Some("0")));
}

#[test]
fn a_wrong_key_is_refused_and_never_echoed() {
    let mock = MockServer::start(0, Some(KEY), mean_filler()).unwrap();
    let at = Endpoint::mock(&mock.base).unwrap();
    let cancel = AtomicBool::new(false);
    let wrong = "wrong-key-abcdefghijkl";
    for p in ALL {
        let e = generate(p, wrong, &at, &request(64, 64, "", 1), &ctl(&cancel)).unwrap_err();
        assert!(matches!(e, GenError::Unauthorized(_)), "{p:?}: {e}");
        assert!(!e.to_string().contains(wrong));
    }
    assert_eq!(generate(Provider::OpenAi, " ", &at, &request(64, 64, "", 1), &ctl(&cancel)), Err(GenError::NoKey));
}

#[test]
fn cancel_and_bad_input_end_cleanly() {
    let mock = MockServer::start(0, Some(KEY), mean_filler()).unwrap();
    let at = Endpoint::mock(&mock.base).unwrap();
    let cancel = AtomicBool::new(true);
    assert_eq!(generate(Provider::Bfl, KEY, &at, &request(64, 64, "x", 1), &ctl(&cancel)), Err(GenError::Cancelled));
    let cancel = AtomicBool::new(false);
    let mut r = request(64, 64, "x", 1);
    r.mask.pop();
    assert!(matches!(generate(Provider::Fal, KEY, &at, &r, &ctl(&cancel)), Err(GenError::Provider(_))));
    // nothing listens here
    let dead = Endpoint::mock("http://127.0.0.1:9").unwrap();
    assert!(matches!(generate(Provider::Stability, KEY, &dead, &request(64, 64, "", 1), &ctl(&cancel)), Err(GenError::Network(_))));
}

#[test]
fn the_mock_endpoint_must_be_local_and_follow_ups_must_stay_with_the_provider() {
    assert!(Endpoint::mock("http://example.com:8080").is_err());
    assert!(Endpoint::mock("http://127.0.0.1:18510").is_ok());
    let real = Endpoint::real(Provider::Bfl);
    assert!(real.follow(Provider::Bfl, "https://api.eu.bfl.ai/v1/get_result?id=1").is_ok());
    assert!(real.follow(Provider::Bfl, "https://evil.example/steal").is_err());
    assert!(real.follow(Provider::Bfl, "http://api.bfl.ai/v1/get_result").is_err(), "plain http is not followed");
    assert!(real.follow(Provider::Fal, "https://v3.fal.media/files/x.png").is_ok());
    assert!(Endpoint::real(Provider::Fal).follow(Provider::Fal, "https://notfal.run.example/x").is_err());
}

#[test]
fn provider_facts_are_complete() {
    for p in ALL {
        let i = p.info();
        assert_eq!(Provider::from_id(i.id), Some(p));
        assert!(i.max_side >= 1024 && i.usd_per_image > 0.0 && i.policy.url.starts_with("https://") && i.key_url.starts_with("https://"));
        assert!(!i.policy.summary.is_empty());
    }
    assert_eq!(requests_for(Provider::OpenAi, 3, true), 1);
    assert_eq!(requests_for(Provider::Stability, 3, true), 3);
    assert_eq!(openai_size(300, 100), "1536x1024");
    assert_eq!(openai_size(100, 300), "1024x1536");
    assert_eq!(openai_size(100, 100), "1024x1024");
}
