// Main event loop — select! over 4 sources, frame-budgeted render.
//
// PR-4 wires the loop skeleton; the reducer is a no-op and the render
// is a no-op. PR-5 fills in the real reduce body and PR-7 the actual
// widget rendering.

use std::time::{Duration, Instant};

use crate::emitter::PagerEvent;
use crate::reducer::{PagerAction, ExitReason};
use crate::state::{PagerState, SharedState};
use parking_lot::RwLock;
use std::sync::Arc;

const FRAME_BUDGET: Duration = Duration::from_millis(16);
#[allow(dead_code)]
const TICK_PERIOD: Duration = Duration::from_millis(50);

/// Run the pager main loop.
///
/// PR-4 uses a dummy sleep-based loop. PR-5 replaces this with a real
/// select! over agent event, input, tick, and background channels.
pub async fn run<A>(_app: A) -> anyhow::Result<()>
where
    A: Send + 'static,
{
    let state: SharedState = Arc::new(RwLock::new(PagerState::default()));
    let mut running = true;
    let mut last_render = Instant::now();

    while running {
        tokio::time::sleep(Duration::from_millis(10)).await;

        let event = PagerEvent::Tick;
        let actions: Vec<PagerAction>;
        {
            let mut guard = state.write();
            actions = crate::reducer::reduce(&mut guard, event);
        }

        for action in actions {
            match action {
                PagerAction::Quit(ExitReason::UserQuit) => {
                    running = false;
                }
                PagerAction::Render => {
                    let snapshot = state.read();
                    let _ = crate::render::render(&snapshot);
                    last_render = Instant::now();
                }
                PagerAction::SendToAgent(_) => {}
                _ => {}
            }
        }

        if last_render.elapsed() >= FRAME_BUDGET {
            let snapshot = state.read();
            let _ = crate::render::render(&snapshot);
            last_render = Instant::now();
        }
    }

    Ok(())
}
