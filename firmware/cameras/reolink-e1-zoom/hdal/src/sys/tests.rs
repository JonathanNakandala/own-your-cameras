use crate::AudioCap;

use super::*;
use h::DRIVER;
fn fixture() -> Sys {
    DRIVER.with_borrow_mut(|d| {
        *d = h::Driver::default();
        d.bytes = vec![1, 2, 3, 4];
        d.phy = 0x1000;
        d.size = 4;
        d.video.pack_num = 1;
        d.video.video_pack[0].phy_addr = 0x1000;
        d.video.video_pack[0].size = 4;
        d.audio.phy_addr[0] = 0x1000;
        d.audio.size = 4;
        d.audio.sample_rate = 48_000;
        d.audio.bit_width = 16;
        d.audio.sound_mode = 1;
    });
    let mut sys = Sys::new().with_speaker_enable(|_| Ok(()));
    sys.common_init().unwrap();
    sys
}
#[test]
fn cache_failure_is_returned_and_the_video_descriptor_is_released() {
    let mut sys = fixture();
    DRIVER.with_borrow_mut(|d| {
        d.errors.insert("hd_common_mem_flush_cache", -77);
    });
    assert_eq!(
        sys.videoenc_recv(&[PathId(1)], 0),
        Err(Error {
            step: Step::VideoencRecv,
            code: -77
        })
    );
    DRIVER.with_borrow(|d| assert_eq!(d.calls.last(), Some(&"hd_videoenc_release_out_buf")));
}

#[test]
fn malformed_video_never_returns_a_partial_frame_or_reads_any_pack() {
    for case in 0..5 {
        let mut sys = fixture();
        DRIVER.with_borrow_mut(|d| match case {
            0 => d.video.pack_num = d.video.video_pack.len() as u32 + 1,
            1 => d.video.pack_num = 0,
            _ => {
                d.video.pack_num = 2;
                let p = &mut d.video.video_pack[1];
                (p.phy_addr, p.size) = match case {
                    2 => (0xfff, 1),
                    3 => (0x1003, 2),
                    _ => (u32::MAX as usize, 2),
                };
            }
        });
        assert!(sys.videoenc_recv(&[PathId(1)], 0).is_err(), "case {case}");
        DRIVER.with_borrow(|d| {
            assert!(
                !d.calls.contains(&"hd_common_mem_flush_cache"),
                "case {case}"
            );
            assert_eq!(d.calls.last(), Some(&"hd_videoenc_release_out_buf"));
        });
    }
}

#[test]
fn unmap_failure_is_returned_but_encoder_close_still_runs() {
    let mut sys = fixture();
    sys.videoenc_recv(&[PathId(1)], 0).unwrap();
    DRIVER.with_borrow_mut(|d| {
        d.errors.insert("hd_common_mem_munmap", -78);
        d.errors.insert("hd_videoenc_close", -79);
    });
    assert_eq!(
        sys.videoenc_close(PathId(1)),
        Err(Error {
            step: Step::VideoencClose,
            code: -78
        })
    );
    DRIVER.with_borrow(|d| assert_eq!(d.calls.last(), Some(&"hd_videoenc_close")));
}

#[test]
fn invalid_mappings_are_rejected_before_pull_or_audio_start() {
    for case in 0..7 {
        let mut sys = fixture();
        DRIVER.with_borrow_mut(|d| {
            d.video.pack_num = 0; // invalid pointer fixtures must never be dereferenced, even before the fix
            match case {
                0 => d.map_result = Some(0),
                1 => d.map_result = Some(usize::MAX),
                2 => d.size = 0,
                3 => d.phy = 0,
                4 => {
                    d.phy = 0xffff_f000;
                    d.size = 8192;
                }
                5 => d.size = 0x8000_0000,
                _ => d.map_result = Some(usize::MAX - 2),
            }
        });
        assert!(sys.videoenc_recv(&[PathId(1)], 0).is_err());
        DRIVER.with_borrow(|d| {
            assert!(
                !d.calls.contains(&"hd_videoenc_pull_out_buf"),
                "case {case}"
            )
        });
        assert!(
            sys.audio_cap_start(&crate::audio::Tuning::stock()).is_err(),
            "audio case {case}"
        );
        assert!(sys.audio_cap_recv().is_err());
        sys.audio_cap_stop().unwrap();
    }
}

#[test]
fn audio_cache_error_survives_a_release_error_and_shutdown_completes() {
    let mut sys = fixture();
    sys.audio_cap_start(&crate::audio::Tuning::stock()).unwrap();
    DRIVER.with_borrow_mut(|d| {
        d.errors.insert("hd_common_mem_flush_cache", -77);
        d.errors.insert("hd_audiocap_release_out_buf", -78);
        d.errors.insert("hd_common_mem_munmap", -79);
        d.errors.insert("hd_audiocap_stop", -80);
    });
    assert_eq!(
        sys.audio_cap_recv(),
        Err(Error {
            step: Step::AudioCapRecv,
            code: -77
        })
    );
    DRIVER.with_borrow(|d| assert_eq!(d.calls.last(), Some(&"hd_audiocap_release_out_buf")));
    assert_eq!(
        sys.audio_cap_stop(),
        Err(Error {
            step: Step::AudioCapStop,
            code: -79
        })
    );
    DRIVER.with_borrow(|d| assert_eq!(d.calls.last(), Some(&"hd_audiocap_uninit")));
    sys.audio_cap_stop().unwrap();
}

#[test]
fn repeated_audio_start_is_rejected_without_replacing_live_resources() {
    use crate::AudioOut;
    let mut sys = fixture();
    DRIVER.with_borrow_mut(|d| d.bytes.resize(2048, 0));
    sys.audio_cap_start(&crate::audio::Tuning::stock()).unwrap();
    sys.audio_out_start(100).unwrap();
    let before = DRIVER.with_borrow(|d| d.calls.len());
    assert!(sys.audio_cap_start(&crate::audio::Tuning::stock()).is_err());
    assert!(sys.audio_out_start(100).is_err());
    assert_eq!(DRIVER.with_borrow(|d| d.calls.len()), before);
    // Little-endian 16-bit samples from the mapped raw ring.
    assert_eq!(
        sys.audio_cap_recv().unwrap().unwrap().samples,
        [0x0201, 0x0403]
    );
    assert!(sys.audio_out_push(&[123, -456]).unwrap());
    sys.audio_out_stop().unwrap();
    sys.audio_cap_stop().unwrap();
    sys.audio_cap_start(&crate::audio::Tuning::stock()).unwrap();
    sys.audio_out_start(100).unwrap();
    sys.audio_out_stop().unwrap();
    sys.audio_cap_stop().unwrap();
}

#[test]
fn invalid_speaker_allocation_never_becomes_a_writable_buffer() {
    use crate::AudioOut;
    for case in 0..5 {
        let mut sys = fixture();
        DRIVER.with_borrow_mut(|d| {
            d.bytes.resize(2048, 0);
            match case {
                0 => d.phy = 0,
                1 => d.phy = 0xffff_ff00,
                2 => d.map_result = Some(usize::MAX),
                3 => d.map_result = Some(0),
                _ => d.map_result = Some(d.bytes.as_mut_ptr() as usize + 1),
            }
        });
        assert!(sys.audio_out_start(100).is_err(), "case {case}");
        assert!(sys.audio_out_push(&[1]).is_err());
        DRIVER.with_borrow(|d| {
            assert_eq!(d.calls.last(), Some(&"hd_audioout_uninit"));
            if case == 2 || case == 3 {
                assert!(!d.calls.contains(&"hd_common_mem_free"));
            }
        });
    }
}

#[test]
fn failed_audio_cleanup_blocks_restart_until_recovery() {
    use crate::AudioOut;
    for startup in [false, true] {
        let mut sys = fixture();
        DRIVER.with_borrow_mut(|d| {
            d.bytes.resize(2048, 0);
            if startup {
                d.errors.insert("hd_common_mem_alloc", -77);
            }
            d.errors.insert("hd_audioout_close", -78);
        });
        if startup {
            assert_eq!(
                sys.audio_out_start(100),
                Err(Error {
                    step: Step::AudioOutStart,
                    code: -77
                })
            );
        } else {
            sys.audio_out_start(100).unwrap();
            assert!(sys.audio_out_stop().is_err());
        }
        DRIVER.with_borrow_mut(|d| d.errors.clear());
        let before = DRIVER.with_borrow(|d| d.calls.len());
        assert!(sys.audio_out_start(100).is_err());
        assert_eq!(DRIVER.with_borrow(|d| d.calls.len()), before);
    }
}

#[test]
fn audio_rejects_invalid_descriptors_and_always_releases_them() {
    for case in 0..4 {
        let mut sys = fixture();
        sys.audio_cap_start(&crate::audio::Tuning::stock()).unwrap();
        DRIVER.with_borrow_mut(|d| match case {
            0 => d.audio.size = 0,
            1 => d.audio.phy_addr[0] = 0xfff,
            2 => d.audio.size = 3,
            _ => d.audio.sample_rate = 16_000,
        });
        assert!(sys.audio_cap_recv().is_err());
        DRIVER.with_borrow(|d| {
            assert!(!d.calls.contains(&"hd_common_mem_flush_cache"));
            assert_eq!(d.calls.last(), Some(&"hd_audiocap_release_out_buf"));
        });
        sys.audio_cap_stop().unwrap();
    }
}

#[test]
fn valid_packs_are_owned_in_descriptor_order_with_original_metadata() {
    let mut sys = fixture();
    DRIVER.with_borrow_mut(|d| {
        d.video.pack_num = 2;
        d.video.video_pack[0].phy_addr = 0x1002;
        d.video.video_pack[0].size = 2;
        d.video.video_pack[1].phy_addr = 0x1000;
        d.video.video_pack[1].size = 2;
        d.video.frame_type = h::HD_FRAME_TYPE_IDR;
        d.video.timestamp = 12345;
    });
    let frames = sys.videoenc_recv(&[PathId(1)], 0).unwrap();
    DRIVER.with_borrow_mut(|d| d.bytes.fill(0));
    assert_eq!(
        frames,
        [EncFrame {
            stream: 0,
            data: vec![3, 4, 1, 2],
            key: true,
            pts_us: 12345
        }]
    );
    sys.videoenc_close(PathId(1)).unwrap();
}

#[test]
fn common_context_is_exclusive_and_cannot_be_recreated_without_process_recovery() {
    let mut first = fixture();
    let mut second = Sys::new();
    assert!(
        first.common_init().is_err(),
        "repeat init must not reset a live graph"
    );
    assert!(
        second.common_init().is_err(),
        "a second owner must not enter HDAL"
    );
    assert!(
        second.common_uninit().is_err(),
        "a rejected owner must not tear down HDAL"
    );
    assert!(
        second.videocap_init().is_err(),
        "unit calls must require ownership"
    );
    first.common_uninit().unwrap();
    assert!(
        first.common_init().is_err(),
        "teardown does not establish kernel recovery"
    );
    assert!(
        Sys::new().common_init().is_err(),
        "a fresh object is not recovery"
    );
    DRIVER.with_borrow(|d| assert_eq!(d.calls, ["hd_common_init", "hd_common_uninit"]));
}

#[test]
fn cleanup_failure_is_terminal_for_all_forward_calls_but_remaining_cleanup_runs() {
    use crate::AudioOut;
    let mut sys = fixture();
    sys.videoenc_recv(&[PathId(1)], 0).unwrap();
    DRIVER.with_borrow_mut(|d| {
        d.errors.insert("hd_common_mem_munmap", -78);
    });
    assert!(sys.videoenc_close(PathId(1)).is_err());
    DRIVER.with_borrow_mut(|d| d.errors.clear());
    let before = DRIVER.with_borrow(|d| d.calls.len());
    assert!(sys.videoenc_recv(&[PathId(1)], 0).is_err());
    assert!(sys.audio_out_start(100).is_err());
    assert!(sys.videocap_init().is_err());
    assert_eq!(DRIVER.with_borrow(|d| d.calls.len()), before);
    sys.videoenc_uninit().unwrap();
    sys.common_uninit().unwrap();
    assert!(Sys::new().common_init().is_err());
    DRIVER.with_borrow(|d| assert_eq!(d.calls.last(), Some(&"hd_common_uninit")));
}

#[test]
fn common_teardown_cannot_invalidate_live_audio_or_encoder_mappings() {
    use crate::AudioOut;
    let mut sys = fixture();
    DRIVER.with_borrow_mut(|d| d.bytes.resize(2048, 0));
    sys.audio_out_start(100).unwrap();
    sys.audio_cap_start(&crate::audio::Tuning::stock()).unwrap();
    sys.videoenc_recv(&[PathId(1)], 0).unwrap();
    let before = DRIVER.with_borrow(|d| d.calls.len());
    assert!(sys.common_mem_uninit().is_err());
    assert!(sys.common_uninit().is_err());
    assert!(sys.videoenc_uninit().is_err());
    assert_eq!(DRIVER.with_borrow(|d| d.calls.len()), before);
    assert!(sys.audio_out_push(&[1, -2]).unwrap());
    assert!(sys.audio_cap_recv().unwrap().is_some());
    sys.audio_out_stop().unwrap();
    sys.audio_cap_stop().unwrap();
    sys.videoenc_close(PathId(1)).unwrap();
    sys.common_mem_uninit().unwrap();
    sys.common_uninit().unwrap();
}

#[test]
fn initialized_units_hold_common_memory_until_their_cleanup() {
    let mut sys = fixture();
    let pool = crate::ProbeConfig::os05a10().pool;
    sys.common_mem_init(&pool).unwrap();
    sys.videocap_init().unwrap();
    sys.videoproc_init().unwrap();
    sys.videoenc_init().unwrap();
    let before = DRIVER.with_borrow(|d| d.calls.len());
    assert!(sys.common_mem_init(&pool).is_err());
    assert!(sys.videocap_init().is_err());
    assert!(sys.videoproc_init().is_err());
    assert!(sys.videoenc_init().is_err());
    assert!(sys.common_mem_uninit().is_err());
    assert!(sys.common_uninit().is_err());
    assert_eq!(DRIVER.with_borrow(|d| d.calls.len()), before);
    sys.videoenc_uninit().unwrap();
    sys.videoproc_uninit().unwrap();
    sys.videocap_uninit().unwrap();
    assert!(
        sys.common_uninit().is_err(),
        "memory must be released first"
    );
    sys.common_mem_uninit().unwrap();
    sys.common_uninit().unwrap();
}

#[test]
fn mapped_encoder_cannot_be_reconfigured_until_its_mapping_is_closed() {
    let mut sys = fixture();
    let path = sys.videoenc_open(0).unwrap();
    sys.videoenc_recv(&[path], 0).unwrap();
    let spec = EncSpec {
        dim: Dim { w: 640, h: 480 },
        fps: 10,
        bitrate_bps: 512_000,
        gop: 10,
    };
    let before = DRIVER.with_borrow(|d| d.calls.len());
    assert!(sys.videoenc_configure(path, &spec, &spec).is_err());
    assert_eq!(DRIVER.with_borrow(|d| d.calls.len()), before);
    sys.videoenc_close(path).unwrap();
    let path = sys.videoenc_open(0).unwrap();
    sys.videoenc_configure(path, &spec, &spec).unwrap();
}

#[test]
fn video_wait_must_not_turn_into_a_negative_driver_timeout() {
    let mut sys = fixture();
    let before = DRIVER.with_borrow(|d| d.calls.len());
    for wait in [i32::MAX as u32 + 1, u32::MAX] {
        assert!(sys.videoenc_recv(&[PathId(1)], wait).is_err());
    }
    assert_eq!(DRIVER.with_borrow(|d| d.calls.len()), before);
}

#[test]
fn simultaneous_sys_owners_share_the_same_atomic_reservation() {
    use std::sync::{Arc, Barrier, atomic::AtomicBool};
    let session = Arc::new(AtomicBool::new(false));
    let barrier = Arc::new(Barrier::new(2));
    let threads: Vec<_> = (0..2)
        .map(|_| {
            let session = session.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                DRIVER.with_borrow_mut(|d| d.session = session);
                let mut sys = Sys::new();
                barrier.wait();
                let acquired = sys.common_init().is_ok();
                if acquired {
                    sys.common_uninit().unwrap();
                }
                (acquired, DRIVER.with_borrow(|d| d.calls.clone()))
            })
        })
        .collect();
    let results: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    assert_eq!(results.iter().filter(|r| r.0).count(), 1);
    for (acquired, calls) in results {
        assert_eq!(
            calls,
            if acquired {
                vec!["hd_common_init", "hd_common_uninit"]
            } else {
                vec![]
            }
        );
    }
}

#[test]
fn failed_common_init_and_abandoned_owner_require_recovery() {
    for failed_init in [false, true] {
        DRIVER.with_borrow_mut(|d| {
            *d = h::Driver::default();
            if failed_init {
                d.errors.insert("hd_common_init", -77);
            }
        });
        let mut sys = Sys::new();
        assert_eq!(sys.common_init().is_err(), failed_init);
        drop(sys);
        DRIVER.with_borrow_mut(|d| d.errors.clear());
        assert!(Sys::new().common_init().is_err());
        DRIVER.with_borrow(|d| assert_eq!(d.calls, ["hd_common_init"]));
    }
}

#[test]
fn every_audio_startup_failure_releases_its_successful_acquisitions() {
    use crate::AudioOut;
    for capture in [false, true] {
        // Discover C entry positions, including repeated set/open calls, from successful
        // startup. Fault each in a fresh isolated driver; mmap has a pointer contract.
        let mut baseline = fixture();
        DRIVER.with_borrow_mut(|d| {
            d.bytes.resize(2048, 0);
            d.calls.clear();
        });
        if capture {
            baseline
                .audio_cap_start(&crate::audio::Tuning::stock())
                .unwrap();
        } else {
            baseline.audio_out_start(100).unwrap();
        }
        let startup = DRIVER.with_borrow(|d| d.calls.clone());
        if capture {
            baseline.audio_cap_stop().unwrap();
        } else {
            baseline.audio_out_stop().unwrap();
        }
        drop(baseline);
        for (index, failed) in startup
            .iter()
            .enumerate()
            .filter(|(_, name)| **name != "hd_common_mem_mmap")
        {
            let mut sys = fixture();
            DRIVER.with_borrow_mut(|d| {
                d.bytes.resize(2048, 0);
                d.calls.clear();
                d.fail_at = Some(index + 1);
            });
            let result = if capture {
                sys.audio_cap_start(&crate::audio::Tuning::stock())
            } else {
                sys.audio_out_start(100)
            };
            assert_eq!(result.unwrap_err().code, -77, "{failed} at {index}");
            assert!(sys.audio_cap_recv().is_err());
            assert!(sys.audio_out_push(&[1]).is_err());
            DRIVER.with_borrow(|d| {
                // The hardware boundary must balance every successful acquisition exactly
                // once. The failed call itself never grants a resource.
                assert!(!d.calls.iter().any(|c| c.starts_with("hd_audioenc")));
                for unit in ["audiocap", "audioout"] {
                    for (take, give) in [("init", "uninit"), ("open", "close"), ("start", "stop")] {
                        let take = format!("hd_{unit}_{take}");
                        let give = format!("hd_{unit}_{give}");
                        let acquired = d.calls[..index].iter().filter(|c| **c == take).count();
                        let released = d.calls[index + 1..].iter().filter(|c| **c == give).count();
                        assert_eq!(acquired, released, "{failed} at {index}: {take}/{give}");
                    }
                }
            });
            DRIVER.with_borrow_mut(|d| d.fail_at = None);
            // A clean atomic failure may retry audio within the still-owned common session.
            if capture {
                sys.audio_cap_start(&crate::audio::Tuning::stock()).unwrap();
                sys.audio_cap_stop().unwrap();
            } else {
                sys.audio_out_start(100).unwrap();
                sys.audio_out_stop().unwrap();
            }
        }
    }
}

#[test]
fn speaker_graph_enables_the_board_amplifier_and_disables_it_on_shutdown() {
    use crate::AudioOut;
    use std::sync::{Arc, Mutex};
    let levels = Arc::new(Mutex::new(Vec::new()));
    let recorded = levels.clone();
    let mut sys = fixture().with_speaker_enable(move |on| {
        recorded.lock().unwrap().push(on);
        Ok(())
    });
    DRIVER.with_borrow_mut(|d| d.bytes.resize(2048, 0));
    sys.audio_out_start(100).unwrap();
    assert_eq!(*levels.lock().unwrap(), [false, true]);
    sys.audio_out_push(&[1, 2]).unwrap();
    sys.audio_out_stop().unwrap();
    assert_eq!(*levels.lock().unwrap(), [false, true, false]);
}

#[test]
fn amplifier_enable_failure_unwinds_hdal_and_attempts_to_mute() {
    use crate::AudioOut;
    use std::sync::{Arc, Mutex};
    let levels = Arc::new(Mutex::new(Vec::new()));
    let recorded = levels.clone();
    let mut sys = fixture().with_speaker_enable(move |on| {
        recorded.lock().unwrap().push(on);
        if on { Err(-5) } else { Ok(()) }
    });
    DRIVER.with_borrow_mut(|d| d.bytes.resize(2048, 0));
    assert_eq!(sys.audio_out_start(100).unwrap_err().code, -5);
    assert_eq!(*levels.lock().unwrap(), [false, true, false]);
    assert!(sys.audio_out_push(&[1]).is_err());
    DRIVER.with_borrow(|d| {
        assert!(d.calls.contains(&"hd_common_mem_free"));
        assert!(d.calls.contains(&"hd_audioout_stop"));
        assert!(d.calls.contains(&"hd_audioout_close"));
        assert!(d.calls.contains(&"hd_audioout_uninit"));
    });
}

#[test]
fn failed_amplifier_mute_still_releases_hdal_and_prevents_reuse() {
    use crate::AudioOut;
    let mut writes = 0;
    let mut sys = fixture().with_speaker_enable(move |_| {
        writes += 1;
        if writes == 3 { Err(-5) } else { Ok(()) }
    });
    DRIVER.with_borrow_mut(|d| d.bytes.resize(2048, 0));
    sys.audio_out_start(100).unwrap();
    assert_eq!(sys.audio_out_stop().unwrap_err().code, -5);
    assert!(sys.audio_out_start(100).is_err());
    DRIVER.with_borrow(|d| assert!(d.calls.contains(&"hd_audioout_uninit")));
}

#[test]
fn failed_unmap_during_mapping_validation_also_makes_the_session_terminal() {
    let mut sys = fixture();
    DRIVER.with_borrow_mut(|d| {
        d.map_result = Some(usize::MAX - 2);
        d.errors.insert("hd_common_mem_munmap", -78);
    });
    assert!(sys.audio_cap_start(&crate::audio::Tuning::stock()).is_err());
    DRIVER.with_borrow_mut(|d| {
        d.map_result = None;
        d.errors.clear();
    });
    let before = DRIVER.with_borrow(|d| d.calls.len());
    assert!(sys.audio_cap_start(&crate::audio::Tuning::stock()).is_err());
    assert_eq!(DRIVER.with_borrow(|d| d.calls.len()), before);
}

#[test]
fn nonblocking_media_statuses_and_real_errors_reach_the_safe_caller() {
    use crate::AudioOut;
    for (function, code) in [
        ("hd_videoenc_poll_list", h::HD_ERR_TIMEDOUT),
        ("hd_videoenc_poll_list", -77),
        ("hd_videoenc_pull_out_buf", -77),
        ("hd_videoenc_release_out_buf", -77),
        ("hd_audiocap_pull_out_buf", h::HD_ERR_TIMEDOUT),
        ("hd_audiocap_pull_out_buf", h::HD_ERR_UNDERRUN),
        ("hd_audiocap_pull_out_buf", -77),
        ("hd_audiocap_release_out_buf", -77),
        ("hd_audioout_push_in_buf", h::HD_ERR_TIMEDOUT),
        ("hd_audioout_push_in_buf", h::HD_ERR_OVERRUN),
        ("hd_audioout_push_in_buf", -77),
    ] {
        let mut sys = fixture();
        DRIVER.with_borrow_mut(|d| d.bytes.resize(2048, 0));
        sys.audio_cap_start(&crate::audio::Tuning::stock()).unwrap();
        sys.audio_out_start(100).unwrap();
        DRIVER.with_borrow_mut(|d| {
            d.calls.clear();
            d.errors.insert(function, code);
        });
        if function.starts_with("hd_videoenc") {
            let result = sys.videoenc_recv(&[PathId(1)], 10);
            if code == h::HD_ERR_TIMEDOUT {
                assert!(result.unwrap().is_empty());
            } else {
                assert_eq!(result.unwrap_err().code, code);
            }
            DRIVER.with_borrow(|d| {
                assert_eq!(
                    d.calls.contains(&"hd_videoenc_release_out_buf"),
                    function.ends_with("release_out_buf")
                )
            });
        } else if function.starts_with("hd_audiocap") {
            let result = sys.audio_cap_recv();
            if matches!(code, h::HD_ERR_TIMEDOUT | h::HD_ERR_UNDERRUN) {
                assert!(result.unwrap().is_none());
            } else {
                assert_eq!(result.unwrap_err().code, code);
            }
            DRIVER.with_borrow(|d| {
                assert_eq!(
                    d.calls.contains(&"hd_audiocap_release_out_buf"),
                    function.ends_with("release_out_buf")
                )
            });
        } else {
            let result = sys.audio_out_push(&[1, -2]);
            if code == -77 {
                assert_eq!(result.unwrap_err().code, code);
            } else {
                assert!(!result.unwrap());
            }
        }
        sys.audio_out_stop().unwrap();
        sys.audio_cap_stop().unwrap();
    }
}

#[test]
fn each_audio_cleanup_fault_blocks_both_audio_and_video_but_attempts_the_rest() {
    use crate::AudioOut;
    for operation in [
        "hd_common_mem_free",
        "hd_audioout_stop",
        "hd_audioout_close",
        "hd_audioout_uninit",
        "hd_common_mem_munmap",
        "hd_audiocap_stop",
        "hd_audiocap_close",
        "hd_audiocap_uninit",
    ] {
        let mut sys = fixture();
        DRIVER.with_borrow_mut(|d| d.bytes.resize(2048, 0));
        sys.audio_cap_start(&crate::audio::Tuning::stock()).unwrap();
        sys.audio_out_start(100).unwrap();
        DRIVER.with_borrow_mut(|d| {
            d.calls.clear();
            d.errors.insert(operation, -77);
        });
        let cap = sys.audio_cap_stop();
        let out = sys.audio_out_stop();
        assert!(cap.is_err() || out.is_err(), "{operation}");
        DRIVER.with_borrow(|d| {
            assert!(d.calls.contains(&"hd_audiocap_uninit"));
            assert!(d.calls.contains(&"hd_audioout_uninit"));
        });
        DRIVER.with_borrow_mut(|d| d.errors.clear());
        let before = DRIVER.with_borrow(|d| d.calls.len());
        assert!(sys.audio_cap_start(&crate::audio::Tuning::stock()).is_err());
        assert!(sys.audio_out_start(100).is_err());
        assert!(sys.videoenc_recv(&[PathId(1)], 0).is_err());
        assert_eq!(DRIVER.with_borrow(|d| d.calls.len()), before);
        sys.common_uninit().unwrap();
    }
}

#[test]
fn audio_start_rejects_a_swallowed_ioctl_failure_and_unwinds() {
    let mut sys = fixture();
    DRIVER.with_borrow_mut(|d| d.swallowed_ioctl = Some("hd_audiocap_set"));
    assert_eq!(
        sys.audio_cap_start(&crate::audio::Tuning::stock()),
        Err(Error {
            step: Step::AudioCapStart,
            code: -65550
        })
    );
    DRIVER.with_borrow(|d| {
        assert!(!d.calls.contains(&"hd_audiocap_start"));
        assert!(d.calls.contains(&"hd_audiocap_close"));
        assert!(!d.calls.iter().any(|c| c.starts_with("hd_audioenc")));
        assert!(d.calls.contains(&"hd_audiocap_uninit"));
    });
}

#[test]
fn common_init_rejects_a_swallowed_ioctl_failure() {
    let _ = fixture();
    DRIVER.with_borrow_mut(|d| {
        *d = h::Driver::default();
        d.swallowed_ioctl = Some("hd_common_init");
    });
    assert_eq!(
        Sys::new().common_init(),
        Err(Error {
            step: Step::CommonInit,
            code: -65550
        })
    );
}

#[test]
fn mapping_rejects_swallowed_ioctl_error_before_using_the_pointer() {
    let mut sys = fixture();
    DRIVER.with_borrow_mut(|d| d.swallowed_ioctl = Some("hd_common_mem_mmap"));
    assert_eq!(
        sys.videoenc_recv(&[PathId(1)], 0),
        Err(Error {
            step: Step::VideoencRecv,
            code: -65550
        })
    );
    DRIVER.with_borrow(|d| assert!(!d.calls.contains(&"hd_videoenc_pull_out_buf")));
}

#[test]
fn capture_keeps_first_error_and_does_not_leak_into_the_next_adapter_call() {
    let mut sys = fixture();
    DRIVER.with_borrow_mut(|d| {
        d.swallowed_ioctl = Some("hd_videocap_init");
        d.swallowed_errors = vec![14, 5];
    });
    assert_eq!(
        sys.videocap_init(),
        Err(Error {
            step: Step::VideocapInit,
            code: -65550
        })
    );
    assert!(
        sys.common_mem_init(&MemPool {
            blk_size: 4,
            blk_cnt: 1
        })
        .is_ok()
    );
}

#[test]
fn syscall_errno_cannot_be_mistaken_for_normal_nonblocking_status() {
    let mut sys = fixture();
    DRIVER.with_borrow_mut(|d| {
        d.swallowed_ioctl = Some("hd_videoenc_poll_list");
        d.swallowed_errors = vec![15];
        // Unreported kernel failure: HDAL falsely returns zero, never a real HD_ERR_TIMEDOUT.
    });
    assert_eq!(
        sys.videoenc_recv(&[PathId(1)], 0),
        Err(Error {
            step: Step::VideoencRecv,
            code: -65551
        })
    );
}

#[test]
fn diagnostic_timer_rejects_swallowed_errors_without_poisoning_later_calls() {
    unsafe extern "C" fn failed() -> u32 {
        ioctl_capture::record(-1, 14, 0xc0085400, None);
        0
    }
    unsafe extern "C" fn zero() -> u32 {
        0
    }
    // SAFETY: test functions implement the exact no-argument timer signature.
    assert_eq!(unsafe { read_time_ms(failed) }, Err(-65550));
    // SAFETY: same signature; a zero timer is not in itself a syscall error.
    assert_eq!(unsafe { read_time_ms(zero) }, Ok(0));
}

#[test]
fn overlapping_packs_and_oversize_aus_fail_before_cache_or_copy() {
    for oversize in [false, true] {
        let mut sys = fixture();
        DRIVER.with_borrow_mut(|d| {
            if oversize {
                d.bytes.resize(crate::enc::MAX_AU_BYTES + 1, 0);
                d.size = d.bytes.len() as u32;
                d.video.video_pack[0].size = d.size as u32;
            } else {
                d.video.pack_num = 2;
                d.video.video_pack[1] = d.video.video_pack[0];
            }
        });
        assert!(sys.videoenc_recv(&[PathId(1)], 0).is_err());
        DRIVER.with_borrow(|d| {
            assert!(!d.calls.contains(&"hd_common_mem_flush_cache"));
            // An oversize AU in a ring that may be mapped is pulled, refused and released uncopied.
            assert_eq!(d.calls.last(), Some(&"hd_videoenc_release_out_buf"));
        });
    }
}

#[test]
fn a_ring_beyond_the_mapping_ceiling_is_refused_before_pull() {
    let mut sys = fixture();
    DRIVER.with_borrow_mut(|d| {
        d.size = (crate::enc::MAX_RING_BYTES + 4096) as u32;
    });
    assert!(sys.videoenc_recv(&[PathId(1)], 0).is_err());
    DRIVER.with_borrow(|d| {
        assert!(!d.calls.contains(&"hd_videoenc_pull_out_buf"));
        assert!(!d.calls.contains(&"hd_common_mem_mmap"));
    });
}

#[test]
fn kernel_reported_empty_pull_is_not_an_unreported_syscall_failure() {
    let mut sys = fixture();
    sys.audio_cap_start(&crate::audio::Tuning::stock()).unwrap();
    DRIVER.with_borrow_mut(|d| {
        d.swallowed_ioctl = Some("hd_audiocap_pull_out_buf");
        d.ioctl_command = Some(0xc018490c);
        d.errors
            .insert("hd_audiocap_pull_out_buf", h::HD_ERR_UNDERRUN);
    });
    assert_eq!(sys.audio_cap_recv().unwrap(), None);
    // The same ioctl without a reported negative kernel status still fails closed.
    DRIVER.with_borrow_mut(|d| {
        d.errors.clear();
    });
    assert_eq!(sys.audio_cap_recv().unwrap_err().code, -65550);
}

#[test]
fn speaker_volume_needs_a_running_speaker() {
    use crate::AudioOut;
    let mut sys = fixture();
    assert!(sys.audio_out_volume(50).is_err(), "speaker not started");
    DRIVER.with_borrow_mut(|d| d.bytes.resize(2048, 0));
    sys.audio_out_start(100).unwrap();
    sys.audio_out_volume(50).unwrap();
    // The volume takes effect only when the running io path is started again.
    DRIVER.with_borrow(|d| {
        assert_eq!(
            d.calls[d.calls.len() - 2..],
            ["hd_audioout_set", "hd_audioout_start"]
        )
    });
    sys.audio_out_stop().unwrap();
}

#[test]
fn capture_start_hands_the_mic_volume_to_the_codec_gain_after_the_hdal_volume() {
    use std::sync::{Arc, Mutex};
    let seen = Arc::new(Mutex::new(Vec::new()));
    let recorded = seen.clone();
    let mut sys = fixture().with_mic_gain(move |volume| {
        let hdal_volume = DRIVER.with_borrow(|d| d.cap_volume);
        recorded.lock().unwrap().push((volume, hdal_volume));
        Ok(())
    });
    let mut tuning = crate::audio::Tuning::stock();
    tuning.mic_volume = 70;
    sys.audio_cap_start(&tuning).unwrap();
    sys.audio_cap_stop().unwrap();
    DRIVER.with_borrow_mut(|d| d.cap_volume = None);
    sys.audio_cap_start(&tuning).unwrap();
    assert_eq!(
        *seen.lock().unwrap(),
        [(70, Some(100)), (70, Some(100))],
        "every start, after the HDAL volume"
    );
}

#[test]
fn a_refused_codec_gain_fails_the_capture_start_and_releases_it() {
    let mut sys = fixture().with_mic_gain(|_| Err(-13));
    let err = sys
        .audio_cap_start(&crate::audio::Tuning::stock())
        .unwrap_err();
    assert_eq!(err.code, -13);
    assert!(sys.audio_cap_recv().is_err());
    DRIVER.with_borrow(|d| {
        assert!(d.calls.contains(&"hd_audiocap_stop"));
        assert!(d.calls.contains(&"hd_audiocap_uninit"));
    });
}

#[test]
fn hdal_capture_volume_stays_at_100_whatever_the_mic_volume() {
    let mut sys = fixture().with_mic_gain(|_| Ok(()));
    let mut tuning = crate::audio::Tuning::stock();
    tuning.mic_volume = 30;
    sys.audio_cap_start(&tuning).unwrap();
    DRIVER.with_borrow(|d| assert_eq!(d.cap_volume, Some(100)));
}
