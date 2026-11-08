//! MQTT on the shared layer: the network thread, and the consumer that turns input edges into states
//! and events. Door and setting commands come back through the returned queue; the returned
//! publisher carries the settings' states and telemetry.
use super::Device;
use camera_transport::mqtt;
use ds_kd8003_ime1_cam::config::Config;
use ds_kd8003_ime1_cam::home::{self, Command, Out};
use ds_kd8003_ime1_cam::inputs::Edges;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

/// Wait before the worker connects again after a session it could not continue.
const RESTART: Duration = Duration::from_secs(5);

pub fn start(
    g: Arc<Device>,
    cfg: &Config,
    edges: &Edges,
) -> std::io::Result<(mqtt::Publisher, mqtt::Commands<Command>)> {
    let (publisher, commands, worker) = mqtt::channel(
        home::transport(cfg, env!("CARGO_PKG_VERSION")),
        home::Parser,
    )?;
    let rx = edges.subscribe(256);
    let returned = publisher.clone();
    let (g2, consumer) = (g.clone(), publisher.clone());
    super::spawn_named("mqtt-edges", move || {
        while !g2.stop.load(Ordering::Relaxed) {
            let Ok(edge) = rx.recv_timeout(Duration::from_millis(500)) else {
                continue;
            };
            let result = match home::publication(&edge) {
                Some(Out::State(id, value)) => consumer.state(id, value),
                Some(Out::Event(id, payload)) => consumer.event(id, payload),
                None => Ok(()),
            };
            if let Err(e) = result {
                g2.log(format!("mqtt: {e}"));
            }
        }
    });
    super::spawn_named("mqtt", move || {
        let progress = || {
            let mut s = g.st.lock().unwrap();
            s.mqtt_online = publisher.is_connected();
            s.mqtt_drops = u64::from(publisher.drops());
        };
        let log = |line: &str| g.log(format!("mqtt: {line}"));
        let hooks = mqtt::Hooks {
            on_progress: Some(&progress),
            log: Some(&log),
            ..mqtt::Hooks::default()
        };
        // A session the worker cannot continue (a resumed session, no announcement) ends in a
        // reconnect after the back-off, so remote open never stays down until reboot; the
        // publisher and command queue returned here stay valid across it.
        worker.run_restarting(&g.stop, hooks, RESTART);
        g.st.lock().unwrap().mqtt_online = false;
    });
    Ok((returned, commands))
}
