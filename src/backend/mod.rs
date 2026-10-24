//! Audio backends. An [`Engine`] lives on its own worker thread (COM objects are thread-bound), and
//! the UI talks to it through a [`Handle`]: read the latest snapshot, send commands.

use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::model::{apply_command, Command, Snapshot};

#[cfg(windows)]
mod appinfo;
pub mod mock;
#[cfg(windows)]
mod routing;
#[cfg(windows)]
pub mod wasapi;

/// How often levels are refreshed. ~30 Hz is smooth for meters without burning CPU.
const TICK: Duration = Duration::from_millis(33);
/// How often devices and sessions are re-enumerated (apps start and stop streams, devices come
/// and go). Level and volume reads in between use cached handles.
const RESCAN: Duration = Duration::from_millis(750);

pub trait Engine {
    /// Read current state. `rescan` asks for a full re-enumeration rather than just refreshing
    /// levels on what is already known.
    fn tick(&mut self, rescan: bool) -> Snapshot;
    fn apply(&mut self, cmd: &Command);
    /// Anything worth including in a bug report, such as meter read failures.
    fn diagnostics(&self) -> String {
        String::new()
    }
}

#[derive(Clone)]
pub struct Handle {
    snapshot: Arc<Mutex<Snapshot>>,
    tx: Sender<Command>,
}

impl Handle {
    pub fn snapshot(&self) -> Snapshot {
        self.snapshot.lock().map(|s| s.clone()).unwrap_or_default()
    }

    /// Send a command. The shared snapshot is patched immediately so a dragged fader doesn't
    /// snap back to the old value for the frames before the worker catches up.
    pub fn send(&self, cmd: Command) {
        if let Ok(mut s) = self.snapshot.lock() {
            apply_command(&mut s, &cmd);
        }
        let _ = self.tx.send(cmd);
    }
}

/// Start `make`'s engine on a worker thread. `repaint` is called after each fresh snapshot.
pub fn spawn<E, F>(repaint: impl Fn() + Send + 'static, make: F) -> Handle
where
    E: Engine,
    F: FnOnce() -> E + Send + 'static,
{
    let snapshot = Arc::new(Mutex::new(Snapshot::default()));
    let (tx, rx) = mpsc::channel::<Command>();
    let shared = snapshot.clone();
    thread::Builder::new()
        .name("audio-engine".into())
        .spawn(move || {
            let mut engine = make();
            let mut last_scan = Instant::now() - RESCAN;
            let mut dirty = true;
            loop {
                // Wait for a command, but never longer than a tick.
                match rx.recv_timeout(TICK) {
                    Ok(cmd) => {
                        engine.apply(&cmd);
                        // Drain anything queued behind it so a fast drag doesn't build a backlog.
                        while let Ok(more) = rx.try_recv() {
                            engine.apply(&more);
                        }
                        dirty = true;
                    }
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => break,
                }
                let rescan = dirty || last_scan.elapsed() >= RESCAN;
                if rescan {
                    last_scan = Instant::now();
                    dirty = false;
                }
                let fresh = engine.tick(rescan);
                if let Ok(mut s) = shared.lock() {
                    *s = fresh;
                }
                repaint();
            }
        })
        .expect("spawn audio engine thread");
    Handle { snapshot, tx }
}
