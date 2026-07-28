use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use aravis_port_core::bootstrap::{control_channel_privilege, offset};

use crate::net::GvcpTransaction;

pub(crate) struct HeartbeatHandle {
    stop_tx: mpsc::Sender<()>,
    join: Option<JoinHandle<()>>,
}

impl HeartbeatHandle {
    pub fn spawn(conn: Arc<Mutex<GvcpTransaction>>, period: Duration, control_lost: Arc<AtomicBool>) -> Self {
        let (stop_tx, stop_rx) = mpsc::channel();
        let join = thread::spawn(move || run(conn, period, control_lost, stop_rx));
        Self {
            stop_tx,
            join: Some(join),
        }
    }

    pub fn stop(&mut self) {
        let _ = self.stop_tx.send(());
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

impl Drop for HeartbeatHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

fn run(conn: Arc<Mutex<GvcpTransaction>>, period: Duration, control_lost: Arc<AtomicBool>, stop_rx: mpsc::Receiver<()>) {
    loop {
        match stop_rx.recv_timeout(period) {
            Ok(()) | Err(RecvTimeoutError::Disconnected) => return,
            Err(RecvTimeoutError::Timeout) => {}
        }

        let value = conn.lock().unwrap().read_register(offset::CONTROL_CHANNEL_PRIVILEGE);
        match value {
            Ok(v) => {
                let has_control =
                    v & (control_channel_privilege::EXCLUSIVE | control_channel_privilege::CONTROL) != 0;
                control_lost.store(!has_control, Ordering::Relaxed);
                if !has_control {
                    log::warn!("control-channel privilege lost (register value=0x{v:08x})");
                }
            }
            Err(e) => {
                log::warn!("heartbeat read failed, treating control as lost: {e}");
                control_lost.store(true, Ordering::Relaxed);
            }
        }
    }
}
