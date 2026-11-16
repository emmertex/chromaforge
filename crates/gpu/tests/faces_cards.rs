//! Face models and two graphics cards: a new model goes to the second card first, each card has a device of its own
//! that gives the same answers, and a card that stops working leaves new models to the other card. On a machine with
//! one card the second device is made on that same card (`same`: the two-device logic, not two cards' speed). Its own
//! process: the face devices and the second card are once per process.

use lcc_faces::net::{Accelerator, ConvJob};
use lcc_gpu::faces::{self, Card};

/// A convolution big enough for the GPU (64 → 64 channels, 3 × 3, on 40 × 40).
fn conv(gpu: &dyn Accelerator, weights: &[f32], bias: &[f32], input: &[f32]) -> Result<Option<Vec<f32>>, String> {
    gpu.conv(&ConvJob {
        layer: 0,
        cin: 64,
        h: 40,
        w: 40,
        cout: 64,
        groups: 1,
        kh: 3,
        kw: 3,
        sh: 1,
        sw: 1,
        pt: 1,
        pl: 1,
        oh: 40,
        ow: 40,
        weights,
        bias: Some(bias),
        input,
    })
}

#[test]
fn face_models_take_the_second_card_first_and_leave_a_card_that_stopped() {
    if !lcc_gpu::available() {
        lcc_testkit::skip!(Gpu, "no GPU adapter");
        return;
    }
    // a second device on the same card (this machine may have one card)
    assert!(lcc_gpu::set_helper("same"));
    let weights: Vec<f32> = (0..64 * 64 * 9).map(|i| ((i * 37 % 23) as f32 - 11.0) * 0.03).collect();
    let bias: Vec<f32> = (0..64).map(|i| i as f32 * 0.01 - 0.3).collect();
    let input: Vec<f32> = (0..64 * 40 * 40).map(|i| ((i * 13 % 29) as f32 - 14.0) * 0.07).collect();

    let first = match faces::accelerator() {
        Ok(a) => a,
        Err(e) => {
            lcc_testkit::skip!(Gpu, "no GPU for the face kernels: {e}");
            return;
        }
    };
    assert_eq!(first.card(), Card::Second, "a new model goes to the second card");
    let editing = faces::accelerator_on(Card::Editing).expect("the card for editing's own face device");
    assert_eq!(editing.card(), Card::Editing);
    let on_second = conv(&*first, &weights, &bias, &input).unwrap().expect("run on the GPU");
    let on_editing = conv(&*editing, &weights, &bias, &input).unwrap().expect("run on the GPU");
    assert_eq!(on_second, on_editing, "both cards' devices give the same answer");
    eprintln!("second: {}; editing: {}", faces::adapter_on(Card::Second).unwrap(), faces::adapter_on(Card::Editing).unwrap());

    // the second card stops working: its model fails (and so drops it), the card for editing carries on
    faces::give_up_on(Card::Second, "device lost (test)");
    assert!(conv(&*first, &weights, &bias, &input).is_err(), "a model on the lost card fails");
    assert_eq!(conv(&*editing, &weights, &bias, &input).unwrap(), Some(on_editing.clone()), "the card for editing is not touched");
    assert!(faces::broken_reason_on(Card::Second).is_some_and(|w| w.contains("device lost")));
    assert_eq!(faces::broken_reason_on(Card::Editing), None);
    assert_eq!(faces::broken_reason(), None, "a new model still has a card");
    assert!(faces::accelerator_on(Card::Second).is_err());
    let next = faces::accelerator().expect("the next model goes to the card for editing");
    assert_eq!(next.card(), Card::Editing);
    assert_eq!(conv(&*next, &weights, &bias, &input).unwrap(), Some(on_editing));
    lcc_gpu::set_helper("");
}
