//! Graph order, clean-up and the Annex B dump, against `hdal::fake::Fake`.
use super::*;
use crate::annexb::{self, AnnexB};
use hdal::Step;
use hdal::enc::EncFrame;
use hdal::fake::Fake;

fn run(hw: &mut Fake, seconds: u32) -> Result<(DumpReport, Vec<Vec<u8>>), PipelineError> {
    let (mut main, mut sub) = (Vec::new(), Vec::new());
    let report = {
        let mut sinks = [AnnexB::new(&mut main), AnnexB::new(&mut sub)];
        dump(hw, &PipelineConfig::e1_zoom(), seconds, &mut sinks)?
    };
    Ok((report, vec![main, sub]))
}

#[test]
fn builds_the_graph_in_order_starts_downstream_last_and_tears_down_in_reverse() {
    let mut hw = Fake::default();
    run(&mut hw, 1).expect("dump");
    let calls: Vec<&str> = hw
        .calls
        .iter()
        .copied()
        .filter(|c| *c != "videoenc_recv")
        .collect();
    assert_eq!(
        calls,
        [
            "common_init",
            "common_mem_init",
            "videocap_init",
            "videoproc_init",
            "videoenc_init",
            "videocap_open",
            "videocap_set_sensor",
            "videocap_open_io",
            "videocap_set_in",
            "videocap_set_out",
            "videoproc_open",
            "videoproc_set_dev_config",
            "videoproc_open",
            "videoproc_set_out",
            "videoproc_open",
            "videoproc_set_out",
            "videoenc_open",
            "videoenc_configure",
            "videoenc_open",
            "videoenc_configure",
            "bind_cap_proc",
            "bind_proc_enc",
            "bind_proc_enc",
            "videoenc_start",
            "videoenc_start",
            "videoproc_start",
            "videoproc_start",
            "videocap_start",
            "videocap_stop",
            "videoproc_stop",
            "videoproc_stop",
            "videoenc_stop",
            "videoenc_stop",
            "unbind",
            "unbind",
            "unbind",
            "videoenc_close",
            "videoenc_close",
            "videoproc_close",
            "videoproc_close",
            "videoproc_close",
            "videocap_close",
            "videocap_close",
            "videoenc_uninit",
            "videoproc_uninit",
            "videocap_uninit",
            "common_mem_uninit",
            "common_uninit",
        ]
    );
}

#[test]
fn main_is_the_full_4_3_frame_and_sub_keeps_the_vendor_settings() {
    let mut hw = Fake::default();
    run(&mut hw, 1).expect("dump");
    let dim = |w, h| hdal::Dim { w, h };
    assert_eq!(hw.input, Some((dim(2592, 1944), 30)));
    assert_eq!(hw.proc_dev_dim, Some(dim(2592, 1944)));
    assert_eq!(
        hw.proc_out,
        [(0, dim(1920, 1440), 15), (1, dim(640, 480), 10)]
    );
    let specs: Vec<_> = hw
        .enc_specs
        .iter()
        .map(|(i, s)| (*i, s.dim, s.fps, s.gop))
        .collect();
    // gop 15 frames = 1 s at 15 fps; sub: 10 frames = 1 s at 10 fps.
    assert_eq!(
        specs,
        [(0, dim(1920, 1440), 15, 15), (1, dim(640, 480), 10, 10)]
    );
    // Every main size is 4:3, and the main ring is sized for the largest size and bitrate.
    assert!(MAIN_SIZES.iter().all(|d| d.w * 3 == d.h * 4));
    assert_eq!(
        hw.envelopes[0].1,
        EncSpec {
            dim: dim(2592, 1944),
            bitrate_bps: MAIN_MAX_BITRATE,
            fps: 15,
            gop: 15
        }
    );
    assert_eq!(hw.binds, [(0, 0), (1, 1)]);
    // Bound to videoproc: frames must not queue for a user pull.
    assert_eq!(hw.cap_out_depth, Some(0));
}

#[test]
fn the_dump_runs_for_the_requested_seconds_and_counts_frames_per_stream() {
    let mut hw = Fake::default();
    let (report, _) = run(&mut hw, 4).expect("dump");
    assert!((4000..4200).contains(&report.elapsed_ms), "{report:?}");
    // 15 and 10 fps over 4 s, give or take the edge frame.
    assert!((58..=62).contains(&report.streams[0].frames), "{report:?}");
    assert!((38..=42).contains(&report.streams[1].frames), "{report:?}");
}

#[test]
fn each_dump_starts_with_sps_pps_then_an_idr_and_is_annex_b() {
    let mut hw = Fake::default();
    let (_, files) = run(&mut hw, 2).expect("dump");
    for f in &files {
        assert_eq!(&f[..4], [0, 0, 0, 1]);
        let types = annexb::nal_types(f);
        assert_eq!(&types[..3], [7, 8, 5], "{types:?}");
    }
}

#[test]
fn a_failing_step_unwinds_everything_that_was_done() {
    for step in [
        Step::VideoprocSetOut,
        Step::VideoencConfigure,
        Step::BindProcEnc,
        Step::VideocapStart,
        Step::VideoencRecv,
    ] {
        let mut hw = Fake::failing(step, -7);
        let err = run(&mut hw, 1).expect_err("must fail");
        assert_eq!(
            err.to_string(),
            format!("hd_{} failed: HD_RESULT -7", step.name())
        );
        let last = hw.calls.last().copied();
        assert_eq!(last, Some("common_uninit"), "{step:?}: {:?}", hw.calls);
        let n = |name: &str| hw.calls.iter().filter(|c| **c == name).count();
        assert_eq!(n("common_init"), n("common_uninit"));
        assert_eq!(n("videoenc_open"), n("videoenc_close"));
        assert_eq!(n("videoproc_open"), n("videoproc_close"));
        assert_eq!(
            n("videocap_open") + n("videocap_open_io"),
            n("videocap_close")
        );
    }
}

#[test]
fn frames_before_the_first_key_frame_are_dropped_and_sps_pps_are_repeated_if_missing() {
    let mut out = Vec::new();
    let mut w = AnnexB::new(&mut out);
    let p = |data: &[u8]| EncFrame {
        stream: 0,
        data: data.to_vec(),
        key: false,
        pts_us: 0,
    };
    let sps = [0, 0, 0, 1, 0x67, 1];
    let pps = [0, 0, 0, 1, 0x68, 2];
    let idr_only = [0, 0, 1, 0x65, 3];
    w.write(&p(&[0, 0, 0, 1, 0x41, 9])).unwrap();
    w.write(&EncFrame {
        key: true,
        data: [&sps[..], &pps[..], &[0, 0, 0, 1, 0x65, 3][..]].concat(),
        ..p(&[])
    })
    .unwrap();
    w.write(&p(&[0, 0, 0, 1, 0x41, 9])).unwrap();
    w.write(&EncFrame {
        key: true,
        data: idr_only.to_vec(),
        ..p(&[])
    })
    .unwrap();
    assert_eq!(w.stats().frames, 3);
    assert_eq!(w.stats().key_frames, 2);
    drop(w);
    let types = annexb::nal_types(&out);
    assert_eq!(types, [7, 8, 5, 1, 7, 8, 5]);
}

#[test]
fn stream_delivers_frames_until_told_to_stop_then_tears_down() {
    let mut hw = Fake::default();
    let trace = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    hw.trace = Some(std::sync::Arc::clone(&trace));
    let mut got = [0usize; 2];
    let mut polls = 0;
    stream(
        &mut hw,
        &PipelineConfig::e1_zoom(),
        &mut || {
            if !trace.lock().unwrap().contains(&"videocap_start") {
                return true;
            }
            polls += 1;
            polls <= 20
        },
        &mut |f: EncFrame| got[f.stream] += 1,
    )
    .expect("stream");
    assert!(got[0] > 0 && got[1] > 0, "both streams deliver: {got:?}");
    assert_eq!(hw.calls.last(), Some(&"common_uninit"));
}

#[test]
fn stream_tears_down_when_a_receive_fails() {
    let mut hw = Fake::failing(Step::VideoencRecv, -3);
    let err = stream(
        &mut hw,
        &PipelineConfig::e1_zoom(),
        &mut || true,
        &mut |_| {},
    )
    .unwrap_err();
    assert_eq!(err.primary.step, Step::VideoencRecv);
    assert_eq!(hw.calls.last(), Some(&"common_uninit"));
}

#[test]
fn audio_is_ready_before_advertising_and_delivers_real_capture_pts() {
    use hdal::audio::PcmFrame;
    let mut hw = Fake::default();
    hw.audio_frames.push_back(PcmFrame {
        pts_us: 64_000,
        samples: vec![7; 1024],
    });
    let ready = std::cell::Cell::new(false);
    let mut polls = 0;
    let mut seen = Vec::new();
    let report = stream_with_audio(
        &mut hw,
        &PipelineConfig::e1_zoom(),
        &mut || ready.set(true),
        &mut || {},
        &mut || {
            if !ready.get() {
                return true;
            }
            polls += 1;
            polls <= 2
        },
        &mut |_| {},
        &mut |f| seen.push((f.pts_us, f.samples.len())),
        &mut || None,
        &mut Untuned,
    )
    .unwrap();
    assert!(ready.get());
    assert_eq!(seen, [(64_000, 1024)]);
    assert_eq!((report.mic_frames, report.mic_samples), (1, 1024));
    assert_eq!(hw.calls.last(), Some(&"common_uninit"));
}

#[test]
fn the_speaker_gets_whole_driver_frames_and_a_full_buffer_drops_only_old_samples() {
    // Four 320-sample mixer blocks in one turn: one whole 1,024-sample frame goes out, 256 wait.
    let mut hw = Fake::default();
    let ready = std::cell::Cell::new(false);
    let mut polls = 0;
    let mut blocks = 4;
    let report = stream_with_audio(
        &mut hw,
        &PipelineConfig::e1_zoom(),
        &mut || ready.set(true),
        &mut || {},
        &mut || {
            if !ready.get() {
                return true;
            }
            polls += 1;
            polls == 1
        },
        &mut |_| {},
        &mut |_| {},
        &mut || {
            (blocks > 0).then(|| {
                blocks -= 1;
                vec![1i16; 320]
            })
        },
        &mut Untuned,
    )
    .unwrap();
    assert_eq!(report.speaker_samples, 1024);
    assert_eq!(hw.speaker_pcm.len(), 1024);
    // A refusing driver: what waits is capped at 200 ms, the oldest samples are dropped.
    let mut hw = Fake::default();
    hw.speaker_full = true;
    ready.set(false);
    let mut polls = 0;
    let report = stream_with_audio(
        &mut hw,
        &PipelineConfig::e1_zoom(),
        &mut || ready.set(true),
        &mut || {},
        &mut || {
            if !ready.get() {
                return true;
            }
            polls += 1;
            polls <= 5
        },
        &mut |_| {},
        &mut |_| {},
        &mut || Some(vec![1; 960]),
        &mut Untuned,
    )
    .unwrap();
    assert_eq!(report.speaker_samples, 0);
    assert_eq!(report.speaker_dropped_samples, 5 * 10 * 960 - 9600);
    assert!(hw.speaker_pcm.is_empty());
}

#[test]
fn audio_startup_failure_never_advertises_and_runtime_failures_release_the_graph() {
    for step in [
        Step::AudioOutStart,
        Step::AudioCapStart,
        Step::AudioCapRecv,
        Step::AudioOutPush,
    ] {
        let mut hw = Fake::failing(step, -7);
        let ready = std::cell::Cell::new(false);
        let err = stream_with_audio(
            &mut hw,
            &PipelineConfig::e1_zoom(),
            &mut || ready.set(true),
            &mut || {},
            &mut || true,
            &mut |_| {},
            &mut |_| {},
            &mut || Some(vec![1; 320]),
            &mut Untuned,
        )
        .unwrap_err();
        assert_eq!(err.primary.step, step);
        assert_eq!(
            ready.get(),
            matches!(step, Step::AudioCapRecv | Step::AudioOutPush)
        );
        assert_eq!(hw.calls.last(), Some(&"common_uninit"));
        if ready.get() {
            let index = |n| hw.calls.iter().position(|c| *c == n).unwrap();
            assert!(index("audio_cap_stop") < index("common_mem_uninit"));
            assert!(index("audio_out_stop") < index("common_mem_uninit"));
        }
    }
}

#[test]
fn dump_reports_buffered_output_failure_and_flushes_both_sinks_after_teardown() {
    use std::cell::Cell;
    use std::io::{self, BufWriter};
    use std::rc::Rc;
    use std::sync::{Arc, Mutex};

    struct Sink {
        flushed: Rc<Cell<bool>>,
        trace: Arc<Mutex<Vec<&'static str>>>,
        fail: bool,
    }
    impl Write for Sink {
        fn write(&mut self, data: &[u8]) -> io::Result<usize> {
            assert!(self.trace.lock().unwrap().contains(&"common_uninit"));
            self.flushed.set(true);
            if self.fail {
                Err(io::Error::other("disk full"))
            } else {
                Ok(data.len())
            }
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let trace = Arc::new(Mutex::new(Vec::new()));
    let flushed = [Rc::new(Cell::new(false)), Rc::new(Cell::new(false))];
    let mut sinks = std::array::from_fn(|i| {
        AnnexB::new(BufWriter::with_capacity(
            64 * 1024,
            Sink {
                flushed: Rc::clone(&flushed[i]),
                trace: Arc::clone(&trace),
                fail: i == 0,
            },
        ))
    });
    let mut hw = Fake::default();
    hw.trace = Some(trace);
    let result = dump(&mut hw, &PipelineConfig::e1_zoom(), 1, &mut sinks);
    assert!(result.is_err(), "buffered disk failure must not be success");
    assert!(
        flushed.iter().all(|sink| sink.get()),
        "both outputs must be flushed"
    );
    assert_eq!(hw.calls.last(), Some(&"common_uninit"));
}

#[test]
fn video_only_cancellation_stops_startup_at_the_observed_hardware_boundary() {
    use std::sync::{Arc, Mutex};
    for last_call in [
        None,
        Some("common_init"),
        Some("videoproc_set_out"),
        Some("videocap_start"),
    ] {
        let trace = Arc::new(Mutex::new(Vec::new()));
        let mut hw = Fake::default();
        hw.trace = Some(Arc::clone(&trace));
        stream(
            &mut hw,
            &PipelineConfig::e1_zoom(),
            &mut || last_call.is_some_and(|name| !trace.lock().unwrap().contains(&name)),
            &mut |_| panic!("no frame after cancellation"),
        )
        .unwrap();
        let forward: Vec<_> = hw
            .calls
            .iter()
            .copied()
            .take_while(|name| {
                !name.ends_with("stop")
                    && !name.ends_with("uninit")
                    && !name.ends_with("close")
                    && *name != "unbind"
            })
            .collect();
        assert_eq!(forward.last().copied(), last_call, "{:?}", hw.calls);
        if last_call.is_some() {
            assert_eq!(hw.calls.last(), Some(&"common_uninit"));
        }
    }
}

/// A tuner that asks for `requests[i]` on the `at[i]`-th loop poll, and keeps every report.
struct Script {
    initial: MediaTuning,
    requests: Vec<(usize, MediaTuning)>,
    polls: usize,
    reports: Vec<Tuned>,
}

impl Script {
    fn new(requests: Vec<(usize, MediaTuning)>) -> Self {
        Self {
            initial: MediaTuning::default(),
            requests,
            polls: 0,
            reports: Vec::new(),
        }
    }
}

impl Tuner for Script {
    fn initial(&mut self) -> MediaTuning {
        self.initial
    }
    fn next(&mut self) -> Option<MediaTuning> {
        self.polls += 1;
        let i = self.requests.iter().position(|(at, _)| *at == self.polls)?;
        Some(self.requests.remove(i).1)
    }
    fn report(&mut self, tuned: Tuned) {
        self.reports.push(tuned);
    }
}

fn run_tuned(
    hw: &mut Fake,
    tuner: &mut Script,
    polls: usize,
) -> Result<Vec<EncFrame>, PipelineError> {
    let mut left = polls;
    let mut frames = Vec::new();
    let ready = std::cell::Cell::new(false);
    stream_with_audio(
        hw,
        &PipelineConfig::e1_zoom(),
        &mut || ready.set(true),
        &mut || {},
        &mut || {
            if !ready.get() {
                return true;
            }
            left = left.saturating_sub(1);
            left > 0
        },
        &mut |f| frames.push(f),
        &mut |_| {},
        &mut || None,
        tuner,
    )?;
    Ok(frames)
}

fn quiet() -> Tuning {
    let mut t = Tuning::stock();
    t.aec.enabled = false;
    t.anr.enabled = false;
    t.mic_volume = 100;
    t
}

#[test]
fn persisted_tuning_starts_the_audio_and_is_reported() {
    let mut hw = Fake::default();
    let mut tuner = Script::new(vec![]);
    tuner.initial.audio = Some(Tuning {
        speaker_volume: 70,
        ..quiet()
    });
    run_tuned(&mut hw, &mut tuner, 5).unwrap();
    assert_eq!(hw.speaker_volumes, [70]);
    assert_eq!(hw.tunings[0].mic_volume, 100);
    assert!(tuner.reports.iter().any(|r| matches!(
        r,
        Tuned::Audio { applied, refused: None } if applied.speaker_volume == 70 && !applied.aec.enabled
    )));
}

#[test]
fn a_failed_tuned_audio_start_retries_once_with_stock() {
    let mut hw = Fake::default().failing_nth(Step::AudioCapStart, 1, -7);
    let mut tuner = Script::new(vec![]);
    tuner.initial.audio = Some(quiet());
    run_tuned(&mut hw, &mut tuner, 5).expect("never fails the start on a tuning value");
    assert_eq!(hw.tunings, [Tuning::stock()]);
    assert!(tuner.reports.iter().any(|r| matches!(
        r,
        Tuned::Audio { applied, refused: Some(_) } if *applied == Tuning::stock()
    )));
}

#[test]
fn audio_tuning_is_applied_between_frames_on_the_media_thread() {
    let mut hw = Fake::default();
    let want = quiet();
    let mut tuner = Script::new(vec![(
        20,
        MediaTuning {
            audio: Some(want),
            main: None,
        },
    )]);
    run_tuned(&mut hw, &mut tuner, 40).unwrap();
    // The capture path alone restarts: stop, start with the new tuning, while video keeps flowing.
    let stop = hw
        .calls
        .iter()
        .position(|c| *c == "audio_cap_stop")
        .unwrap();
    assert_eq!(hw.calls[stop + 1], "audio_cap_start");
    assert!(hw.calls[..stop].contains(&"videoenc_recv"));
    assert!(
        hw.calls[stop..].contains(&"videoenc_recv"),
        "keeps streaming"
    );
    for (call, n) in [
        ("audio_out_start", 1),
        ("videoenc_stop", 2),
        ("videocap_start", 1),
    ] {
        assert_eq!(hw.calls.iter().filter(|c| **c == call).count(), n, "{call}");
    }
    assert_eq!(hw.tunings.last(), Some(&want));
    assert_eq!(
        tuner.reports.last(),
        Some(&Tuned::Audio {
            applied: want,
            refused: None
        })
    );
}

#[test]
fn a_refused_audio_tuning_keeps_the_previous_one() {
    // audio_cap_start: the first is startup, the second the retune.
    let mut hw = Fake::default().failing_nth(Step::AudioCapStart, 2, -22);
    let mut tuner = Script::new(vec![(
        3,
        MediaTuning {
            audio: Some(quiet()),
            main: None,
        },
    )]);
    run_tuned(&mut hw, &mut tuner, 10).unwrap();
    assert_eq!(
        tuner.reports.last(),
        Some(&Tuned::Audio {
            applied: Tuning::stock(),
            refused: Some(hdal::Error {
                step: Step::AudioCapStart,
                code: -22
            })
        })
    );
    assert_eq!(
        hw.tunings.last(),
        Some(&Tuning::stock()),
        "restarted as before"
    );
}

#[test]
fn a_speaker_volume_change_touches_only_the_speaker() {
    let mut hw = Fake::default();
    let mut tuner = Script::new(vec![(
        3,
        MediaTuning {
            audio: Some(Tuning {
                speaker_volume: 30,
                ..Tuning::stock()
            }),
            main: None,
        },
    )]);
    run_tuned(&mut hw, &mut tuner, 10).unwrap();
    assert_eq!(hw.speaker_volumes, [100, 30]);
    assert!(!hw.calls.contains(&"audio_cap_stop") || hw.calls.last() == Some(&"common_uninit"));
    assert_eq!(
        hw.calls.iter().filter(|c| **c == "audio_cap_start").count(),
        1
    );
}

fn small() -> MainStream {
    MainStream {
        dim: hdal::Dim { w: 1280, h: 960 },
        bitrate_bps: 512_000,
        fps: 10,
    }
}

#[test]
fn a_main_retune_restarts_only_the_main_path_and_opens_with_a_key_frame() {
    let mut hw = Fake::default();
    let mut tuner = Script::new(vec![(
        200,
        MediaTuning {
            audio: None,
            main: Some(small()),
        },
    )]);
    let frames = run_tuned(&mut hw, &mut tuner, 400).unwrap();
    let at = hw
        .calls
        .iter()
        .position(|c| *c == "videoenc_retune")
        .unwrap();
    let window: Vec<_> = hw.calls[at - 3..at + 3].to_vec();
    assert_eq!(
        window,
        [
            "videoproc_stop",
            "videoenc_stop",
            "videoproc_set_out",
            "videoenc_retune",
            "videoenc_start",
            "videoproc_start"
        ]
    );
    assert_eq!(
        hw.proc_out.last(),
        Some(&(0, hdal::Dim { w: 1280, h: 960 }, 10))
    );
    assert_eq!(hw.enc_specs.last().unwrap(), &(0, small().spec()));
    // Nothing else restarted: one start each for capture, the sub path and audio.
    for c in ["videocap_start", "audio_cap_start", "audio_out_start"] {
        assert_eq!(hw.calls.iter().filter(|x| **x == c).count(), 1, "{c}");
    }
    assert_eq!(
        hw.calls.iter().filter(|c| **c == "videoenc_start").count(),
        3
    );
    // First main frame after the restart is a key frame; the sub stream continued meanwhile.
    let recv_before = hw.calls[..at]
        .iter()
        .filter(|c| **c == "videoenc_recv")
        .count();
    let main_after: Vec<_> = frames
        .iter()
        .filter(|f| f.stream == 0 && f.pts_us > 0)
        .skip_while(|f| f.pts_us / 1000 < (recv_before as u64) * 10)
        .collect();
    assert!(main_after.first().is_some_and(|f| f.key));
    assert!(matches!(
        tuner.reports.last(),
        Some(Tuned::Main { applied, refused: None, .. }) if *applied == small()
    ));
}

#[test]
fn a_refused_main_retune_restores_the_previous_stream() {
    // videoproc_set_out: two at build, the third is the retune.
    let mut hw = Fake::default().failing_nth(Step::VideoprocSetOut, 3, -22);
    let mut tuner = Script::new(vec![(
        5,
        MediaTuning {
            audio: None,
            main: Some(small()),
        },
    )]);
    run_tuned(&mut hw, &mut tuner, 20).expect("a refusal is not a media error");
    let stock = MainStream::of(&PipelineConfig::e1_zoom().streams[0]);
    assert!(matches!(
        tuner.reports.last(),
        Some(Tuned::Main { applied, refused: Some(_), .. }) if *applied == stock
    ));
    assert_eq!(hw.enc_specs.last().unwrap().1, stock.spec());
}

#[test]
fn a_main_path_left_stopped_is_a_media_error_with_exact_cleanup() {
    // videoenc_start: two at build, the third restarts main.
    let mut hw = Fake::default().failing_nth(Step::VideoencStart, 3, -5);
    let mut tuner = Script::new(vec![(
        5,
        MediaTuning {
            audio: None,
            main: Some(small()),
        },
    )]);
    let err = run_tuned(&mut hw, &mut tuner, 20).unwrap_err();
    assert_eq!(err.primary.step, Step::VideoencStart);
    assert!(err.cleanup.is_empty(), "{:?}", err.cleanup);
    // main was stopped by the retune and is not stopped again; sub is stopped once by the teardown.
    assert_eq!(
        hw.calls.iter().filter(|c| **c == "videoenc_stop").count(),
        2
    );
    assert_eq!(
        hw.calls.iter().filter(|c| **c == "videoproc_stop").count(),
        2
    );
    assert_eq!(hw.calls.last(), Some(&"common_uninit"));
}

#[test]
fn a_persisted_main_stream_the_driver_refuses_falls_back_to_stock_at_build() {
    let mut hw = Fake::default().failing_nth(Step::VideoencConfigure, 1, -22);
    let mut tuner = Script::new(vec![]);
    tuner.initial.main = Some(MainStream {
        dim: hdal::Dim { w: 2592, h: 1944 },
        bitrate_bps: 8_000_000,
        fps: 30,
    });
    run_tuned(&mut hw, &mut tuner, 5).expect("boot never fails on a setting");
    let stock = PipelineConfig::e1_zoom().streams[0];
    assert_eq!(
        hw.enc_specs.iter().rfind(|(i, _)| *i == 0).unwrap().1,
        stock
    );
    assert!(matches!(
        tuner.reports.first(),
        Some(Tuned::Main { applied, refused: Some(_), .. }) if *applied == MainStream::of(&stock)
    ));
}

#[test]
fn an_unchanged_main_request_does_nothing() {
    let mut hw = Fake::default();
    let stock = MainStream::of(&PipelineConfig::e1_zoom().streams[0]);
    let mut tuner = Script::new(vec![(
        3,
        MediaTuning {
            audio: None,
            main: Some(stock),
        },
    )]);
    run_tuned(&mut hw, &mut tuner, 10).unwrap();
    assert!(!hw.calls.contains(&"videoenc_retune"));
}

#[test]
fn a_capture_that_cannot_restart_at_all_is_a_media_error_without_a_second_stop() {
    let mut hw = Fake::default()
        .failing_nth(Step::AudioCapStart, 2, -22)
        .failing_nth(Step::AudioCapStart, 3, -22);
    let mut tuner = Script::new(vec![(
        3,
        MediaTuning {
            audio: Some(quiet()),
            main: None,
        },
    )]);
    let err = run_tuned(&mut hw, &mut tuner, 10).unwrap_err();
    assert_eq!(err.primary.step, Step::AudioCapStart);
    assert_eq!(
        hw.calls.iter().filter(|c| **c == "audio_cap_stop").count(),
        1
    );
    assert_eq!(hw.calls.last(), Some(&"common_uninit"));
}

fn fast_main() -> MainStream {
    MainStream {
        dim: hdal::Dim { w: 2592, h: 1944 },
        bitrate_bps: 4_096_000,
        fps: 30,
    }
}

#[test]
fn a_main_change_that_starves_the_sub_stream_is_reverted_before_the_watchdog_budget() {
    let mut hw = Fake::default();
    hw.starve_sub_above_fps = Some(15);
    let mut tuner = Script::new(vec![(
        100,
        MediaTuning {
            audio: None,
            main: Some(fast_main()),
        },
    )]);
    let frames = run_tuned(&mut hw, &mut tuner, 1500).unwrap();
    let stock = MainStream::of(&PipelineConfig::e1_zoom().streams[0]);
    assert!(matches!(
        tuner.reports.last(),
        Some(Tuned::Main { applied, refused: Some(e), .. }) if *applied == stock && e.code == STALLED
    ));
    assert_eq!(hw.enc_specs.last().unwrap(), &(0, stock.spec()));
    // The longest sub-stream silence stayed under the revert threshold plus one frame period.
    let sub: Vec<u64> = frames
        .iter()
        .filter(|f| f.stream == 1)
        .map(|f| f.pts_us / 1000)
        .collect();
    let gap = sub.windows(2).map(|w| w[1] - w[0]).max().unwrap();
    assert!(gap <= STALL_REVERT_MS + 200, "sub silent {gap} ms");
}

#[test]
fn a_persisted_main_stream_that_starves_the_sub_stream_reverts_to_stock_at_startup() {
    let mut hw = Fake::default();
    hw.starve_sub_above_fps = Some(15);
    let mut tuner = Script::new(vec![]);
    tuner.initial.main = Some(fast_main());
    run_tuned(&mut hw, &mut tuner, 1500).unwrap();
    let stock = MainStream::of(&PipelineConfig::e1_zoom().streams[0]);
    assert!(matches!(
        tuner.reports.last(),
        Some(Tuned::Main { applied, refused: Some(_), .. }) if *applied == stock
    ));
}

#[test]
fn a_sustainable_main_change_survives_probation() {
    let mut hw = Fake::default();
    hw.starve_sub_above_fps = Some(15);
    let mut tuner = Script::new(vec![(
        100,
        MediaTuning {
            audio: None,
            main: Some(MainStream {
                fps: 15,
                ..fast_main()
            }),
        },
    )]);
    run_tuned(&mut hw, &mut tuner, 2500).unwrap();
    assert!(matches!(
        tuner.reports.last(),
        Some(Tuned::Main { applied, refused: None, .. }) if applied.fps == 15 && applied.dim.w == 2592
    ));
}
