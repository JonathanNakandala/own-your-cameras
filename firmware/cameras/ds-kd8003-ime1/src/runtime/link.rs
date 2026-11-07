//! The MQTT thread: keeps one broker connection alive, replays retained state after each reconnect,
//! forwards `door/command` to the main loop.
use super::Device;
use ds_kd8003_ime1_cam::config::Config;
use ds_kd8003_ime1_cam::discovery::{self, DoorCommand, Topics};
use ds_kd8003_ime1_cam::mqtt::{Client, Incoming, Options};
use std::collections::BTreeMap;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

/// A doorbell press or access event older than this is not worth delivering after an outage.
const EVENT_TTL: Duration = Duration::from_secs(10);

pub struct Outgoing {
    pub topic: String,
    pub payload: Vec<u8>,
    pub retain: bool,
    pub queued: Instant,
}

/// Handle the main loop publishes through. Retained topics are remembered for replay; the channel
/// never blocks the main loop.
pub struct Publisher {
    tx: mpsc::Sender<Outgoing>,
    retained: Arc<Mutex<BTreeMap<String, Vec<u8>>>>,
}

impl Publisher {
    pub fn publish(&self, topic: &str, payload: impl Into<Vec<u8>>, retain: bool) {
        let payload = payload.into();
        if retain {
            self.retained
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(topic.to_string(), payload.clone());
        }
        let _ = self.tx.send(Outgoing {
            topic: topic.into(),
            payload,
            retain,
            queued: Instant::now(),
        });
    }
}

pub fn start(
    g: Arc<Device>,
    cfg: &Config,
    commands: mpsc::Sender<DoorCommand>,
) -> (Publisher, std::thread::JoinHandle<()>) {
    let (tx, rx) = mpsc::channel::<Outgoing>();
    let retained = Arc::new(Mutex::new(BTreeMap::new()));
    let publisher = Publisher {
        tx,
        retained: retained.clone(),
    };
    let topics = Topics::new(&cfg.mqtt_prefix, &cfg.mqtt_node);
    let discovery = discovery::payload(
        &cfg.mqtt_node,
        &cfg.mqtt_prefix,
        &cfg.device_name,
        env!("CARGO_PKG_VERSION"),
    );
    let options = Options {
        client_id: cfg.mqtt_node.clone(),
        username: cfg.mqtt_user.clone(),
        password: cfg.mqtt_password.clone(),
        keep_alive: Duration::from_secs(30),
        will_topic: topics.availability.clone(),
        will_payload: discovery::OFFLINE.as_bytes().to_vec(),
    };
    let addr = (cfg.mqtt_host.clone(), cfg.mqtt_port);
    let handle = super::spawn_named("mqtt", move || {
        let mut backoff = 2u64;
        while !g.stop.load(Ordering::Relaxed) {
            let mut client = match Client::connect(&addr, &options) {
                Ok(c) => c,
                Err(e) => {
                    g.log(format!("mqtt: connect failed: {e}"));
                    g.st.lock().unwrap().mqtt_online = false;
                    sleep_while_running(&g, Duration::from_secs(backoff));
                    backoff = (backoff * 2).min(60);
                    continue;
                }
            };
            backoff = 2;
            let connected_at = Instant::now();
            let result = (|| -> std::io::Result<()> {
                client.subscribe(&[&topics.door_command, "homeassistant/status"])?;
                announce(&mut client, &topics, &discovery, &retained)?;
                g.st.lock().unwrap().mqtt_online = true;
                g.log("mqtt: connected");
                while !g.stop.load(Ordering::Relaxed) {
                    while let Ok(out) = rx.try_recv() {
                        if !out.retain && out.queued.elapsed() > EVENT_TTL {
                            continue;
                        }
                        client.publish(&out.topic, &out.payload, out.retain)?;
                    }
                    match client.poll(Duration::from_millis(100))? {
                        Some(Incoming::Publish {
                            topic,
                            payload,
                            retain,
                        }) if topic == topics.door_command => {
                            if retain {
                                // A retained command would open the door on every reconnect and boot.
                                g.log("mqtt: ignored retained door command");
                            } else {
                                match discovery::parse_command(&payload) {
                                    Some(cmd) => {
                                        let _ = commands.send(cmd);
                                    }
                                    None => g.log("mqtt: ignored unknown door command"),
                                }
                            }
                        }
                        Some(Incoming::Publish { topic, payload, .. })
                            if topic == "homeassistant/status" && payload == b"online" =>
                        {
                            announce(&mut client, &topics, &discovery, &retained)?;
                        }
                        _ => {}
                    }
                }
                Ok(())
            })();
            g.st.lock().unwrap().mqtt_online = false;
            match result {
                Ok(()) => {
                    let _ =
                        client.publish(&topics.availability, discovery::OFFLINE.as_bytes(), true);
                    client.disconnect();
                    return;
                }
                Err(e) => {
                    g.log(format!(
                        "mqtt: connection lost after {} s: {e}",
                        connected_at.elapsed().as_secs()
                    ));
                    g.st.lock().unwrap().mqtt_drops += 1;
                }
            }
            sleep_while_running(&g, Duration::from_secs(2));
        }
    });
    (publisher, handle)
}

/// Discovery, every remembered retained state, then availability. The map is copied first so the
/// main loop never waits on a slow broker.
fn announce(
    client: &mut Client,
    topics: &Topics,
    discovery: &str,
    retained: &Mutex<BTreeMap<String, Vec<u8>>>,
) -> std::io::Result<()> {
    client.publish(&topics.discovery, discovery.as_bytes(), true)?;
    let snapshot = retained.lock().unwrap_or_else(|e| e.into_inner()).clone();
    for (topic, payload) in &snapshot {
        client.publish(topic, payload, true)?;
    }
    client.publish(&topics.availability, discovery::ONLINE.as_bytes(), true)
}

fn sleep_while_running(g: &Device, d: Duration) {
    let end = Instant::now() + d;
    while Instant::now() < end && !g.stop.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(100));
    }
}
