PYTHON ?= python3
MI360 := cameras/mi360
REOLINK := cameras/reolink-e1-zoom
DS_KD8003_IME1 := cameras/ds-kd8003-ime1
SDK_HEADERS := $(abspath $(MI360)/sdk)
.PHONY: headers fast area gate build-mi360 build-reolink build-ds-kd8003-ime1
headers:
	$(PYTHON) $(MI360)/scripts/headers.py --out "$(SDK_HEADERS)"
fast: headers
	$(PYTHON) -B scripts/fetch-opus.py
	$(PYTHON) -B scripts/test_link_stubs.py
	$(PYTHON) -B scripts/test_release.py
	cargo test --workspace --locked
	$(PYTHON) -I -m unittest discover -s $(REOLINK)/scripts
	$(PYTHON) -B $(MI360)/toolchain/test_elf_compat.py
	$(PYTHON) -B $(DS_KD8003_IME1)/toolchain/test_elf_tls.py
	$(PYTHON) -B $(MI360)/sdk/test_imp_bridge.py --headers "$(SDK_HEADERS)"
	$(PYTHON) -B $(MI360)/sdk/test_imp_audio.py --headers "$(SDK_HEADERS)"
	$(PYTHON) -B $(MI360)/sdk/test_imp_controls.py --headers "$(SDK_HEADERS)"
	$(PYTHON) -B $(MI360)/sdk/test_imp_tuning.py --headers "$(SDK_HEADERS)"
	$(PYTHON) -B $(MI360)/scripts/test_recovery.py
	$(PYTHON) -B $(MI360)/firmware/test_environment.py
	$(PYTHON) -B $(MI360)/firmware/test_firmware.py
	$(PYTHON) -B $(MI360)/firmware/test_mtd_writer.py
	$(MAKE) -C $(MI360)/probes/controls fast
area: fast
	$(PYTHON) -I $(REOLINK)/scripts/check-tree
	$(PYTHON) -I $(REOLINK)/scripts/check-links
	cargo fmt --all -- --check
	cargo clippy --workspace --all-targets --locked -- -D warnings
	# the DS-KD8003-IME1's runtime compiles for ARM only; check it without linking.
	cargo clippy -p ds-kd8003-ime1-cam --target armv7-unknown-linux-gnueabihf --locked -- -D warnings
	$(MAKE) -C $(MI360)/probes/controls area
gate: area
build-mi360:
	cd $(MI360) && scripts/build-mips
build-reolink:
	$(MAKE) -C $(REOLINK) build
build-ds-kd8003-ime1:
	cd $(DS_KD8003_IME1) && scripts/build-arm
