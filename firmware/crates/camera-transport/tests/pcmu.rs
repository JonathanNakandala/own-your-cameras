//! Actual loopback wire check; isolated protocol fixtures never reach a camera.
use camera_transport::{
    AudioHub, Config, Hub, ServerLimits, microphone::Format, serve_with_limits,
};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

fn response(reader: &mut BufReader<TcpStream>) -> (String, String) {
    let mut head = String::new();
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        assert!(!line.is_empty());
        head.push_str(&line);
        if line == "\r\n" {
            break;
        }
    }
    let bytes = head
        .lines()
        .find_map(|line| line.strip_prefix("Content-Length: "))
        .map_or(0, |length| length.trim().parse().unwrap());
    let mut body = vec![0; bytes];
    reader.read_exact(&mut body).unwrap();
    (head, String::from_utf8(body).unwrap())
}

#[test]
fn configured_pcmu_microphone_is_advertised_and_transmitted_with_its_real_codec() {
    let hub = Hub::new();
    hub.publish(
        true,
        0,
        vec![
            0, 0, 0, 1, 0x67, 0x4d, 0, 0x1f, 0xac, 0, 0, 0, 1, 0x68, 0xee, 0x3c, 0, 0, 0, 1, 0x65,
        ],
    );
    let audio = AudioHub::with_format(Format::Pcmu8k);
    let cfg = Arc::new(Config {
        streams: vec![("Preview_01_main".into(), hub)],
        creds: None,
        audio: Some(audio.clone()),
        talk: None,
    });
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let stopped = stop.clone();
    let worker = std::thread::spawn(move || {
        serve_with_limits(&listener, cfg, &stopped, None, ServerLimits::MI360).unwrap()
    });
    let mut socket = TcpStream::connect(addr).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let mut reader = BufReader::new(socket.try_clone().unwrap());
    let uri = format!("rtsp://{addr}/Preview_01_main");
    write!(socket, "DESCRIBE {uri} RTSP/1.0\r\nCSeq: 1\r\n\r\n").unwrap();
    let (header, sdp) = response(&mut reader);
    assert!(header.starts_with("RTSP/1.0 200"));
    assert!(sdp.contains("a=rtpmap:0 PCMU/8000/1"));
    assert!(!sdp.contains("MPEG4"));
    write!(socket, "SETUP {uri}/trackID=1 RTSP/1.0\r\nCSeq: 2\r\nTransport: RTP/AVP/TCP;unicast;interleaved=2-3\r\n\r\n").unwrap();
    assert!(response(&mut reader).0.starts_with("RTSP/1.0 200"));
    write!(socket, "PLAY {uri} RTSP/1.0\r\nCSeq: 3\r\n\r\n").unwrap();
    assert!(response(&mut reader).0.starts_with("RTSP/1.0 200"));
    audio.publish(1_000_000, vec![0xff, 0x80, 0]);
    let mut prefix = [0; 4];
    reader.read_exact(&mut prefix).unwrap();
    assert_eq!(prefix, [b'$', 2, 0, 15]);
    let mut rtp = [0; 15];
    reader.read_exact(&mut rtp).unwrap();
    assert_eq!(rtp[1], 0x80);
    assert_eq!(u32::from_be_bytes(rtp[4..8].try_into().unwrap()), 8000);
    assert_eq!(&rtp[12..], &[0xff, 0x80, 0]);
    stop.store(true, Ordering::Relaxed);
    worker.join().unwrap();
}
