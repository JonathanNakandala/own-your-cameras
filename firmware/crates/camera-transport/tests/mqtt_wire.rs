//! The shared MQTT worker against a scripted broker on loopback: device-discovery migration order,
//! retained-command rejection, events that are delivered once and never replayed, and removals that
//! precede every discovery config of an announcement, a refused setting answered with the value in
//! force, and a restarting worker that outlives a resumed session.
use camera_transport::mqtt::{self, Command, Domain, Entity, MIGRATE_MARKER, Mode, Parser};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

struct Door;
impl Parser for Door {
    type App = String;
    fn parse(&self, id: &str, text: &str) -> Result<Option<Command<String>>, String> {
        Ok((id == "door").then(|| Command::App(text.to_owned())))
    }
}

fn packet(s: &mut TcpStream) -> (u8, Vec<u8>) {
    let mut head = [0];
    s.read_exact(&mut head).unwrap();
    let (mut len, mut shift) = (0usize, 0);
    loop {
        let mut b = [0];
        s.read_exact(&mut b).unwrap();
        len |= usize::from(b[0] & 0x7f) << shift;
        shift += 7;
        if b[0] & 0x80 == 0 {
            break;
        }
    }
    let mut body = vec![0; len];
    s.read_exact(&mut body).unwrap();
    (head[0], body)
}

fn remaining(len: usize) -> Vec<u8> {
    let (mut out, mut n) = (Vec::new(), len);
    loop {
        let mut b = (n % 128) as u8;
        n /= 128;
        if n > 0 {
            b |= 0x80;
        }
        out.push(b);
        if n == 0 {
            return out;
        }
    }
}

/// Broker → client QoS 0 publish.
fn deliver(s: &mut TcpStream, topic: &str, payload: &[u8], retain: bool) {
    let mut body = vec![0, topic.len() as u8];
    body.extend_from_slice(topic.as_bytes());
    body.extend_from_slice(payload);
    let mut bytes = vec![0x30 | u8::from(retain)];
    bytes.extend(remaining(body.len()));
    bytes.extend(body);
    s.write_all(&bytes).unwrap();
}

#[derive(Debug)]
struct Seen {
    topic: String,
    payload: Vec<u8>,
    retain: bool,
}

/// Answers the client's control packets and returns its next PUBLISH.
fn next_publish(
    s: &mut TcpStream,
    on_subscribe: &mut dyn FnMut(&mut TcpStream, &[String]),
) -> Seen {
    loop {
        let (head, body) = packet(s);
        match head >> 4 {
            3 => {
                let qos = (head >> 1) & 3;
                let len = usize::from(u16::from_be_bytes([body[0], body[1]]));
                let topic = String::from_utf8(body[2..2 + len].to_vec()).unwrap();
                let mut at = 2 + len;
                if qos > 0 {
                    s.write_all(&[0x40, 2, body[at], body[at + 1]]).unwrap();
                    at += 2;
                }
                return Seen {
                    topic,
                    payload: body[at..].to_vec(),
                    retain: head & 1 == 1,
                };
            }
            8 => {
                let mut filters = Vec::new();
                let mut at = 2;
                while at < body.len() {
                    let len = usize::from(u16::from_be_bytes([body[at], body[at + 1]]));
                    filters.push(String::from_utf8(body[at + 2..at + 2 + len].to_vec()).unwrap());
                    at += 3 + len;
                }
                let mut ack = vec![0x90, 2 + filters.len() as u8, body[0], body[1]];
                ack.extend(std::iter::repeat_n(1, filters.len()));
                s.write_all(&ack).unwrap();
                on_subscribe(s, &filters);
            }
            10 => s.write_all(&[0xb0, 2, body[0], body[1]]).unwrap(),
            12 => s.write_all(&[0xd0, 0]).unwrap(),
            _ => {}
        }
    }
}

fn gate(port: u16) -> mqtt::Config {
    let mut cfg = mqtt::Config::new(
        mqtt::Broker {
            host: "127.0.0.1".into(),
            port,
            username: String::new(),
            password: String::new(),
        },
        "node",
        "gate",
        mqtt::Device {
            name: "Gate".into(),
            manufacturer: "Maker".into(),
            model: "Model".into(),
            sw_version: None,
        },
        vec![
            Entity::new("open", "Open", Domain::Button)
                .command_topic("door/command")
                .route("door"),
            Entity::new("reed", "Door", Domain::BinarySensor).state_topic("reed"),
            Entity::new(
                "doorbell",
                "Doorbell",
                Domain::Event {
                    types: vec!["press"],
                },
            )
            .state_topic("call_button"),
        ],
    );
    cfg.migrate_from_device_discovery = true;
    cfg.stop_on_disconnect = false;
    cfg
}

#[test]
fn migration_commands_and_events_on_the_wire() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let mut cfg = gate(listener.local_addr().unwrap().port());
    // An earlier firmware's separate config on a current entity's topic, and its retained state.
    cfg.superseded = vec![(Domain::Event { types: Vec::new() }, "doorbell".into())];
    cfg.superseded_states = vec!["access/last".into()];
    let (publisher, commands, worker) = mqtt::channel(cfg, Door).unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let worker_stop = stop.clone();
    let thread = std::thread::spawn(move || worker.run(&worker_stop, mqtt::Hooks::default()));
    publisher.state("reed", "closed").unwrap();

    let (mut peer, _) = listener.accept().unwrap();
    peer.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    assert_eq!(packet(&mut peer).0, 0x10);
    peer.write_all(&[0x20, 2, 0, 0]).unwrap();
    let device = "homeassistant/device/node/config";
    let mut retained_doc = |s: &mut TcpStream, filters: &[String]| {
        if filters.iter().any(|f| f == device) {
            deliver(s, device, br#"{"components":{}}"#, true);
        }
    };
    let mut seen = Vec::new();
    let mut at = Vec::new();
    while !seen
        .iter()
        .any(|p: &Seen| p.topic == device && p.payload.is_empty())
    {
        seen.push(next_publish(&mut peer, &mut retained_doc));
        at.push(Instant::now());
    }
    let index = |f: &dyn Fn(&Seen) -> bool| seen.iter().position(f).unwrap();
    let marker = index(&|p| p.topic == device && p.payload == MIGRATE_MARKER);
    let first_config = index(&|p| p.topic.ends_with("/config") && p.topic != device);
    let state = index(&|p| p.topic == "gate/reed" && p.payload == b"closed");
    let clear = seen.len() - 1;
    assert!(marker < first_config && first_config < clear, "{seen:?}");
    let doorbell = "homeassistant/event/node/doorbell/config";
    let old_config = index(&|p| p.topic == doorbell && p.payload.is_empty());
    let old_state = index(&|p| p.topic == "gate/access/last" && p.payload.is_empty());
    let new_config = index(&|p| p.topic == doorbell && !p.payload.is_empty());
    let first_config =
        index(&|p| p.topic.ends_with("/config") && p.topic != device && !p.payload.is_empty());
    assert!(
        marker < old_config && old_config < old_state && old_state < first_config,
        "the earlier configs go between the marker and the first new config: {seen:?}"
    );
    assert!(first_config <= new_config);
    assert!(
        at[first_config] - at[old_state] >= Duration::from_millis(1500),
        "Home Assistant gets time to remove the earlier entities"
    );
    assert_eq!(
        seen.iter().filter(|p| p.payload == MIGRATE_MARKER).count(),
        1,
        "one marker: {seen:?}"
    );
    assert!(
        state < clear,
        "states replayed before the device topic is cleared"
    );
    assert!(seen.iter().all(|p| p.retain));
    assert_eq!(
        seen.iter()
            .filter(|p| p.topic.ends_with("/config") && p.topic != device && !p.payload.is_empty())
            .count(),
        3
    );

    // A retained command never acts; a fresh one does, once.
    deliver(&mut peer, "gate/door/command", b"open", true);
    deliver(&mut peer, "gate/door/command", b"open", false);
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut got = Vec::new();
    while Instant::now() < deadline && got.is_empty() {
        got.extend(commands.try_next(Instant::now()));
        std::thread::sleep(Duration::from_millis(20));
    }
    std::thread::sleep(Duration::from_millis(300));
    got.extend(commands.try_next(Instant::now()));
    assert_eq!(got, [Command::App("open".to_owned())]);

    // An event goes out once, not retained.
    publisher
        .event("doorbell", r#"{"event_type":"press"}"#)
        .unwrap();
    let event = loop {
        let p = next_publish(&mut peer, &mut |_, _| {});
        if p.topic == "gate/call_button" {
            break p;
        }
    };
    assert!(!event.retain);

    // After a reconnect, states are replayed and the event is not; the migration does not run again.
    drop(peer);
    let (mut peer, _) = listener.accept().unwrap();
    peer.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    assert_eq!(packet(&mut peer).0, 0x10);
    peer.write_all(&[0x20, 2, 0, 0]).unwrap();
    let mut again = Vec::new();
    let mut subscribed_device = false;
    while !again
        .iter()
        .any(|p: &Seen| p.topic == "gate/availability" && p.payload == b"online")
    {
        again.push(next_publish(&mut peer, &mut |_, filters| {
            subscribed_device |= filters.iter().any(|f| f == device);
        }));
    }
    assert!(
        again
            .iter()
            .any(|p| p.topic == "gate/reed" && p.payload == b"closed")
    );
    assert!(
        again
            .iter()
            .all(|p| p.topic != "gate/call_button" && p.topic != device)
    );
    assert!(
        again.iter().all(|p| !p.payload.is_empty()),
        "a migrated broker is not cleared again: {again:?}"
    );
    assert!(!subscribed_device);
    stop.store(true, Ordering::Release);
    loop {
        let (head, body) = packet(&mut peer);
        if head >> 4 == 3 && (head >> 1) & 3 == 1 {
            let len = usize::from(u16::from_be_bytes([body[0], body[1]]));
            peer.write_all(&[0x40, 2, body[2 + len], body[3 + len]])
                .unwrap();
        }
        if head >> 4 == 14 {
            break;
        }
    }
    thread.join().unwrap().unwrap();
}

/// Reads one connection's announcement, up to `online` on the availability topic.
fn announcement(listener: &TcpListener) -> (TcpStream, Vec<Seen>) {
    let (mut peer, _) = listener.accept().unwrap();
    peer.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    assert_eq!(packet(&mut peer).0, 0x10);
    peer.write_all(&[0x20, 2, 0, 0]).unwrap();
    let mut seen = Vec::new();
    while !seen
        .iter()
        .any(|p: &Seen| p.topic == "gate/availability" && p.payload == b"online")
    {
        seen.push(next_publish(&mut peer, &mut |_, _| {}));
    }
    (peer, seen)
}

#[test]
fn removals_go_out_before_any_discovery_config() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let mut cfg = gate(listener.local_addr().unwrap().port());
    cfg.migrate_from_device_discovery = false;
    cfg.retired = vec![(Domain::Sensor, "old_cpu".into())];
    let (publisher, _commands, worker) = mqtt::channel(cfg, Door).unwrap();
    // A runtime removal queued before the connection, after a state: it still leads the announcement.
    publisher.state("reed", "closed").unwrap();
    publisher.remove(&Domain::Sensor, "old_wifi").unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let worker_stop = stop.clone();
    let thread = std::thread::spawn(move || worker.run(&worker_stop, mqtt::Hooks::default()));

    let expected = [
        "homeassistant/sensor/node/old_cpu/config",
        "gate/old_cpu/state",
        "homeassistant/sensor/node/old_wifi/config",
        "gate/old_wifi/state",
    ];
    let config = |p: &Seen| p.topic.ends_with("/config") && !p.payload.is_empty();
    let (peer, seen) = announcement(&listener);
    let removals: Vec<_> = seen
        .iter()
        .take_while(|p| !config(p))
        .map(|p| {
            assert!(p.retain && p.payload.is_empty(), "{p:?}");
            p.topic.as_str()
        })
        .collect();
    assert_eq!(removals, expected, "{seen:?}");
    assert_eq!(seen.iter().filter(|p| config(p)).count(), 3);
    assert!(
        seen.iter()
            .skip(expected.len())
            .all(|p| !p.payload.is_empty()),
        "nothing is removed after the first config: {seen:?}"
    );

    // Every connection retires the configured ids first again; a runtime removal was sent once.
    drop(peer);
    let (mut peer, again) = announcement(&listener);
    let removals: Vec<_> = again
        .iter()
        .take_while(|p| !config(p))
        .map(|p| p.topic.as_str())
        .collect();
    assert_eq!(removals, expected[..2], "{again:?}");
    assert!(
        again
            .iter()
            .any(|p| p.topic == "gate/reed" && p.payload == b"closed")
    );

    stop.store(true, Ordering::Release);
    loop {
        let (head, body) = packet(&mut peer);
        if head >> 4 == 3 && (head >> 1) & 3 == 1 {
            let len = usize::from(u16::from_be_bytes([body[0], body[1]]));
            peer.write_all(&[0x40, 2, body[2 + len], body[3 + len]])
                .unwrap();
        }
        if head >> 4 == 14 {
            break;
        }
    }
    thread.join().unwrap().unwrap();
}

#[test]
fn a_restarting_worker_survives_a_resumed_session_with_the_same_handles() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let mut cfg = gate(listener.local_addr().unwrap().port());
    cfg.migrate_from_device_discovery = false;
    let (publisher, commands, worker) = mqtt::channel(cfg, Door).unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let worker_stop = stop.clone();
    let thread = std::thread::spawn(move || {
        worker.run_restarting(
            &worker_stop,
            mqtt::Hooks::default(),
            Duration::from_millis(200),
        )
    });

    // A broker that resumes a session ends that session, not the worker.
    let (mut first, _) = listener.accept().unwrap();
    first
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    assert_eq!(packet(&mut first).0, 0x10);
    first.write_all(&[0x20, 2, 1, 0]).unwrap();
    loop {
        let (head, _) = packet(&mut first);
        if head >> 4 == 14 {
            break;
        }
    }
    drop(first);

    publisher.state("reed", "open").unwrap();
    let (mut peer, seen) = announcement(&listener);
    assert!(
        seen.iter()
            .any(|p| p.topic == "gate/reed" && p.payload == b"open"),
        "{seen:?}"
    );
    deliver(&mut peer, "gate/door/command", b"open", false);
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut got = None;
    while Instant::now() < deadline && got.is_none() {
        got = commands.try_next(Instant::now());
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(got, Some(Command::App("open".to_owned())));

    stop.store(true, Ordering::Release);
    loop {
        let (head, _) = packet(&mut peer);
        if head >> 4 == 14 {
            break;
        }
    }
    thread.join().unwrap();
}

#[test]
fn a_refused_setting_publishes_the_value_in_force_again() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let mut cfg = gate(listener.local_addr().unwrap().port());
    cfg.migrate_from_device_discovery = false;
    cfg.entities
        .push(Entity::new("volume", "Volume", Domain::number(0, 10, Mode::Box)).slot(1));
    let (publisher, commands, worker) = mqtt::channel(cfg, Door).unwrap();
    publisher.state("volume", "3").unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let worker_stop = stop.clone();
    let thread = std::thread::spawn(move || worker.run(&worker_stop, mqtt::Hooks::default()));
    let (mut peer, seen) = announcement(&listener);
    assert!(
        seen.iter()
            .any(|p| p.topic == "gate/volume/state" && p.payload == b"3")
    );

    for refused in [&b"11"[..], b"loud", &[0xff]] {
        deliver(&mut peer, "gate/volume/set", refused, false);
        let again = next_publish(&mut peer, &mut |_, _| {});
        assert_eq!(
            (again.topic.as_str(), again.payload.as_slice(), again.retain),
            ("gate/volume/state", &b"3"[..], true),
            "refused {refused:?}"
        );
    }
    assert_eq!(commands.try_next(Instant::now()), None);
    // A valid value reaches the camera.
    deliver(&mut peer, "gate/volume/set", b"7", false);
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut got = None;
    while Instant::now() < deadline && got.is_none() {
        got = commands.try_next(Instant::now());
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(got, Some(Command::Setting { slot: 1, value: 7 }));
    // The camera's owner refuses it and publishes the unchanged value in force: it goes out.
    publisher.state("volume", "3").unwrap();
    let answer = next_publish(&mut peer, &mut |_, _| {});
    assert_eq!(
        (answer.topic.as_str(), answer.payload.as_slice()),
        ("gate/volume/state", &b"3"[..])
    );

    stop.store(true, Ordering::Release);
    loop {
        let (head, _) = packet(&mut peer);
        if head >> 4 == 14 {
            break;
        }
    }
    thread.join().unwrap().unwrap();
}
