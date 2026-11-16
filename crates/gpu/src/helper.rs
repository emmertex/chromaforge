//! The second graphics card (Settings ▸ Performance ▸ Second graphics card): a device of its own
//! on another card, for batch work (exports, previews being built) and thumbnails, so the editing
//! card keeps the window's renders to itself.
//!
//! It is set up beside the editing card, on a thread of its own: renders never wait for it, and
//! use the editing card (or the processor) until it is ready. Its loss, or turning it off, leaves
//! the editing card alone: its devices have their own generations, and a failure on it stops only
//! it (the photo it was making is made again on the editing card's path). Which card it is follows
//! `cards::resolve_helper`.

use std::sync::{Arc, Mutex};

use crate::ctx::Gpu;

/// What the second card is now.
#[derive(Default)]
enum State {
    /// None chosen or found.
    #[default]
    Off,
    /// Being set up.
    Starting(String),
    Ready(Arc<Gpu>),
    /// Setting it up failed.
    Failed(String),
    /// It failed while in use: not used again until it is chosen again (or the app restarts).
    Lost(String),
}

struct Helper {
    state: State,
    /// Bumped by each [`restart`]: a setup that finishes after a newer one started is dropped.
    epoch: u64,
}

static HELPER: Mutex<Helper> = Mutex::new(Helper { state: State::Off, epoch: 0 });

fn helper() -> std::sync::MutexGuard<'static, Helper> {
    HELPER.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The second card's device when it is ready; never waits for one.
pub(crate) fn current() -> Option<Arc<Gpu>> {
    match &helper().state {
        State::Ready(g) => Some(g.clone()),
        _ => None,
    }
}

/// The second card's state for `app.gpu`: (`off`/`starting`/`ready`/`failed`/`lost`, its name or
/// why it isn't used).
pub(crate) fn status() -> (&'static str, Option<String>) {
    match &helper().state {
        State::Off => ("off", None),
        State::Starting(name) => ("starting", Some(name.clone())),
        State::Ready(g) => ("ready", Some(g.info.name.clone())),
        State::Failed(why) => ("failed", Some(why.clone())),
        State::Lost(why) => ("lost", Some(why.clone())),
    }
}

/// Stop using the second card (its device goes once the renders on it finish).
pub(crate) fn stop() {
    let mut h = helper();
    h.epoch = h.epoch.wrapping_add(1);
    h.state = State::Off;
}

/// Set the second card up again on `adapter` (`None`: none), on a thread of its own; the one in use
/// meanwhile is let go at once.
pub(crate) fn restart(adapter: Option<wgpu::Adapter>) {
    let epoch = {
        let mut h = helper();
        h.epoch = h.epoch.wrapping_add(1);
        h.state = match &adapter {
            Some(a) => State::Starting(a.get_info().name),
            None => State::Off,
        };
        h.epoch
    };
    let Some(adapter) = adapter else { return };
    let backends = wgpu::Backends::from(adapter.get_info().backend);
    let spawned = std::thread::Builder::new().name("lc-gpu-helper".into()).spawn(move || {
        let name = adapter.get_info().name;
        // (under the crash sentinel and the creation deadline, as the editing card's device)
        let made = crate::init::create(backends, move || Gpu::on_adapter(&adapter));
        let mut h = helper();
        if h.epoch != epoch {
            return; // another choice was made meanwhile
        }
        h.state = match made {
            Ok(g) => {
                log::info!("gpu: second graphics card ready: {name}");
                State::Ready(Arc::new(g))
            }
            Err(e) => {
                log::warn!("gpu: the second graphics card ({name}) can't be used: {e}");
                State::Failed(e)
            }
        };
    });
    if let Err(e) = spawned {
        let mut h = helper();
        if h.epoch == epoch {
            h.state = State::Failed(format!("could not start its setup ({e})"));
        }
    }
}

/// The second card's device of `generation` failed: stop using it. The editing card is not
/// touched. `false` when `generation` isn't the second card's.
pub(crate) fn lose(generation: u64, reason: &str) -> bool {
    let mut h = helper();
    let State::Ready(g) = &h.state else { return false };
    if g.generation != generation {
        return false;
    }
    let name = g.info.name.clone();
    h.state = State::Lost(reason.to_string());
    drop(h);
    log::error!("gpu: the second graphics card ({name}) failed; batch work goes to the editing card: {reason}");
    crate::slot::event(
        crate::slot::EventKind::Lost,
        format!("The second graphics card ({name}) stopped responding. Exports and previews go on with the graphics card for editing."),
    );
    true
}

/// Whether `generation` is the second card's device.
pub(crate) fn is_helper(generation: u64) -> bool {
    matches!(&helper().state, State::Ready(g) if g.generation == generation)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn without_a_card_there_is_no_second_device() {
        restart(None);
        assert!(current().is_none());
        assert_eq!(status().0, "off");
        // (a loss of a device that isn't the second card's is not its loss)
        assert!(!lose(u64::MAX, "test"));
        stop();
        assert_eq!(status().0, "off");
    }
}
