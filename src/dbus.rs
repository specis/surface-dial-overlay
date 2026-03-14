use futures_util::StreamExt;
use zbus::proxy;

use crate::DialEvent;

/// zbus proxy for the dial daemon D-Bus interface.
/// The `#[zbus(signal)]` attribute tells the macro to generate
/// `receive_<name>()` methods returning a signal stream.
#[proxy(
    interface = "com.dialmenu.Daemon",
    default_service = "com.dialmenu.Daemon",
    default_path = "/com/dialmenu/Daemon"
)]
trait DialDaemon {
    #[zbus(signal)]
    fn dial_rotated(&self, delta: i32) -> zbus::Result<()>;

    #[zbus(signal)]
    fn dial_pressed(&self) -> zbus::Result<()>;

    #[zbus(signal)]
    fn dial_released(&self) -> zbus::Result<()>;
}

/// Async task: connects to the session bus, subscribes to all three signals,
/// and forwards them as `DialEvent` values through the calloop channel sender.
/// Runs for the lifetime of the application.
pub async fn run(tx: calloop::channel::Sender<DialEvent>) {
    log::info!("Connecting to D-Bus session bus");

    let conn = match zbus::Connection::session().await {
        Ok(c) => c,
        Err(e) => {
            log::error!("Failed to connect to D-Bus: {e}");
            return;
        }
    };

    let proxy = match DialDaemonProxy::new(&conn).await {
        Ok(p) => p,
        Err(e) => {
            log::error!("Failed to create proxy (is com.dialmenu.Daemon running?): {e}");
            return;
        }
    };

    let mut rotated = match proxy.receive_dial_rotated().await {
        Ok(s) => s,
        Err(e) => { log::error!("receive_dial_rotated: {e}"); return; }
    };
    let mut pressed = match proxy.receive_dial_pressed().await {
        Ok(s) => s,
        Err(e) => { log::error!("receive_dial_pressed: {e}"); return; }
    };
    let mut released = match proxy.receive_dial_released().await {
        Ok(s) => s,
        Err(e) => { log::error!("receive_dial_released: {e}"); return; }
    };

    log::info!("Listening for Surface Dial events");

    loop {
        tokio::select! {
            Some(signal) = rotated.next() => {
                if let Ok(args) = signal.args() {
                    log::debug!("DialRotated({})", args.delta);
                    let _ = tx.send(DialEvent::Rotated(args.delta));
                }
            }
            Some(_) = pressed.next() => {
                log::debug!("DialPressed");
                let _ = tx.send(DialEvent::Pressed);
            }
            Some(_) = released.next() => {
                log::debug!("DialReleased");
                let _ = tx.send(DialEvent::Released);
            }
            else => {
                log::warn!("All D-Bus signal streams closed");
                break;
            }
        }
    }
}
