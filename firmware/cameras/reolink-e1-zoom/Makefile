TARGET := armv7-unknown-linux-gnueabihf
ZIGTARGET := $(TARGET).2.30
BIN := target/$(TARGET)/release/cam
LAUNCHER := target/$(TARGET)/release/cam-launch
TOOLS := $(abspath ../../.tools)
export PATH := $(TOOLS)/bin:$(TOOLS)/zig:$(PATH)
# CAMERA is the camera's ssh host (root), required by every target that touches the camera.
need-camera = $(if $(CAMERA),,$(error CAMERA=<ssh host of your camera> is required))
# ffmpeg needs the address the ssh host stands for.
CAMERA_ADDR = $(shell ssh -G $(CAMERA) 2>/dev/null | awk '/^hostname /{print $$2}')
SESSION = CAMERA=$(CAMERA) scripts/device-session

# Maintainer-only checks; the file is absent from published copies and make skips it.
-include publish/check.mk

.PHONY: check test-scripts compiler build udiv-guard device-test device-probe device-dump device-serve device-zoom device-ir tools
compiler:
	python3 ../../scripts/fetch-zig.py
	python3 ../../scripts/fetch-opus.py
test-scripts:
	python3 -I -m unittest discover -s scripts

check: test-scripts $(EXTRA_CHECKS)
	python3 -I scripts/check-tree
	python3 -I scripts/check-links
	cargo fmt --manifest-path ../../Cargo.toml --all -- --check
	cargo clippy --manifest-path ../../Cargo.toml -p cam -p hdal -p hdal-sys --all-targets --locked -- -D warnings
	cargo test --manifest-path ../../Cargo.toml -p cam -p hdal -p hdal-sys --locked

# cargo-zigbuild goes into .tools/, not ~/.cargo/bin; Zig is pinned by firmware/scripts/fetch-zig.py (see README).
tools: $(TOOLS)/bin/cargo-zigbuild
$(TOOLS)/bin/cargo-zigbuild:
	cargo install cargo-zigbuild --locked --version 0.24.0 --root $(TOOLS)

build: compiler tools
	CARGO_TARGET_DIR="$(CURDIR)/target" cargo zigbuild --manifest-path ../../Cargo.toml --locked -p cam --release --target $(ZIGTARGET)
	$(MAKE) udiv-guard

udiv-guard:
	scripts/udiv-guard $(BIN)
	scripts/udiv-guard $(LAUNCHER)

# Copies the binary to /mnt/tmp/cam and runs `cam self-test` next to the vendor stack: the self-test makes no
# hd_common_init call, so nothing is stopped and nothing needs a reboot.
device-test: build
	$(need-camera)
	ssh $(CAMERA) 'cat > /mnt/tmp/cam.new && chmod +x /mnt/tmp/cam.new && mv /mnt/tmp/cam.new /mnt/tmp/cam' < $(BIN)
	ssh $(CAMERA) '/mnt/tmp/cam self-test' | tee target/device-test.out
	grep -q '^hd_gettime_ms=[0-9]' target/device-test.out

# The other device targets run inside scripts/device-session: HDAL beside the vendor `device` breaks it, so the
# session stops the vendor stack, holds the watchdog with `cam wdt`, runs the target's commands and reboots.

# HDAL init, memory pool and capture sysinfo, then the sensor's reported mode.
device-probe: build
	$(need-camera)
	$(SESSION) $(BIN) -- sh -c "ssh $(CAMERA) 'cd /mnt/tmp && ./cam probe-videocap; rc=\$$?; dmesg | tail -40; echo probe_exit=\$$rc' | tee target/probe-videocap.out"
	grep -q '^sensor=nvt_sen_os05a10$$' target/probe-videocap.out
	grep -q '^cur_dim=2592x1944$$' target/probe-videocap.out
	awk -F= '/^cur_fps=/{ok=($$2>=25)} END{exit !ok}' target/probe-videocap.out

# The full pipeline: `cam dump` writes main.h264 and sub.h264 to /mnt/tmp, they are copied back to target/dump/
# and decoded with ffprobe.
DUMP_SECONDS ?= 10
device-dump: build
	$(need-camera)
	mkdir -p target/dump
	$(SESSION) $(BIN) -- sh -c "ssh $(CAMERA) 'cd /mnt/tmp && ./cam dump --seconds $(DUMP_SECONDS); echo dump_exit=\$$?' | tee target/dump.out \
	  && ssh $(CAMERA) 'cat /mnt/tmp/main.h264' > target/dump/main.h264 && ssh $(CAMERA) 'cat /mnt/tmp/sub.h264' > target/dump/sub.h264"
	grep -q '^dump_exit=0$$' target/dump.out
	for s in main sub; do ffprobe -v error -count_frames -select_streams v:0 \
	  -show_entries stream=codec_name,profile,width,height,nb_read_frames -of default=nw=1 -f h264 target/dump/$$s.h264 | tee target/dump/$$s.probe; done
	grep -q '^width=2560$$' target/dump/main.probe && grep -q '^height=1440$$' target/dump/main.probe
	grep -q '^width=640$$' target/dump/sub.probe && grep -q '^height=480$$' target/dump/sub.probe

# `cam serve --diagnostic` for SERVE_SECONDS (RAM-only: no handoff, network or health file; production is
# `--attempt TOKEN` and never runs beside `cam wdt`). AUTH is a local file with the `user:password` go2rtc uses;
# probe it from here while it runs (ffprobe rtsp://…@<camera>:554/sub).
SERVE_SECONDS ?= 120
AUTH ?=
need-auth = $(if $(AUTH),,$(error AUTH=<file with user:password> is required))
device-serve: build
	$(need-camera)
	$(need-auth)
	$(SESSION) --auth $(AUTH) $(BIN) -- sh -c "ssh $(CAMERA) 'cd /mnt/tmp && ./cam serve --diagnostic --auth /mnt/tmp/cam-rtsp.auth --seconds $(SERVE_SECONDS); echo serve_exit=\$$?' | tee target/serve.out"
	grep -q '^serve_exit=0$$' target/serve.out

# Zoom and focus with `cam serve` in the background, so the ISP has frames for 3A and the VA statistic: initialise
# the lens motors, then for each end of the zoom range zoom, grab a frame, autofocus and grab another
# (target/zoom/*.jpg).
ZOOM_ENDS ?= 1024 0
device-zoom: build
	$(need-camera)
	$(need-auth)
	mkdir -p target/zoom
	$(SESSION) --auth $(AUTH) $(BIN) -- sh -c "ssh $(CAMERA) 'cd /mnt/tmp && { ./cam serve --diagnostic --auth /mnt/tmp/cam-rtsp.auth --seconds 300 >serve.log 2>&1 & }; \
	  sleep 8; ./cam zoom init && ./cam zoom info' | tee target/zoom.out; \
	  url=\"rtsp://\$$(cat $(AUTH))@$(CAMERA_ADDR):554/main\"; for z in $(ZOOM_ENDS); do \
	  ssh $(CAMERA) \"/mnt/tmp/cam zoom \$$z\" | tee -a target/zoom.out; \
	  ffmpeg -loglevel error -rtsp_transport tcp -i \"\$$url\" -frames:v 1 -y target/zoom/zoom\$$z-before.jpg; \
	  ssh $(CAMERA) '/mnt/tmp/cam focus' | tee -a target/zoom.out; \
	  ffmpeg -loglevel error -rtsp_transport tcp -i \"\$$url\" -frames:v 1 -y target/zoom/zoom\$$z-focused.jpg; done"
	test $$(grep -c '^af_focus=' target/zoom.out) -eq $(words $(ZOOM_ENDS))

# Day and night with `cam serve` in the background, so the ISP switch shows in frames: a frame after `cam ir day`,
# `cam ir night` and `cam ir day` again (target/ir/*.jpg), the status LED toggled, then the auto mode for
# IR_AUTO_SECONDS with thresholds just under the room's reading. The IR LEDs' own light then lowers the reading
# past the day threshold, so it must switch to night and back to day by itself.
IR_AUTO_SECONDS ?= 25
device-ir: build
	$(need-camera)
	$(need-auth)
	mkdir -p target/ir
	$(SESSION) --auth $(AUTH) $(BIN) -- sh -c "ssh $(CAMERA) 'cd /mnt/tmp && { ./cam serve --diagnostic --auth /mnt/tmp/cam-rtsp.auth --seconds 300 >serve.log 2>&1 & }; \
	  sleep 8; ./cam ir status' | tee target/ir.out; \
	  url=\"rtsp://\$$(cat $(AUTH))@$(CAMERA_ADDR):554/sub\"; for m in day night day; do \
	  ssh $(CAMERA) \"/mnt/tmp/cam ir \$$m && /mnt/tmp/cam ir status\" | tee -a target/ir.out; sleep 4; \
	  ffmpeg -loglevel error -rtsp_transport tcp -i \"\$$url\" -frames:v 1 -y target/ir/\$$m-\$$(date +%s).jpg; done; \
	  ssh $(CAMERA) '/mnt/tmp/cam led off && cat /sys/class/gpio/gpio226/value; /mnt/tmp/cam led on && cat /sys/class/gpio/gpio226/value' | tee -a target/ir.out; \
	  L=\$$(ssh $(CAMERA) '/mnt/tmp/cam ir status' | awk -F= '/^light=/{print \$$2}'); \
	  ssh $(CAMERA) \"/mnt/tmp/cam ir auto --to-night \$$((L - 5)) --to-day \$$((L - 15)) --seconds $(IR_AUTO_SECONDS); /mnt/tmp/cam ir day\" | tee -a target/ir.out"
	test "$$(grep '^isp_night=' target/ir.out | sed -n 2,4p | tr '\n' ' ')" = "isp_night=0 isp_night=1 isp_night=0 "
	grep -q 'ir auto: switched to Night' target/ir.out
	grep -q 'ir auto: switched to Day after [1-9]' target/ir.out
