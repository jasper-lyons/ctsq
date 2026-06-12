INSTALL_DIR := $(HOME)/.local/bin

.PHONY: install
install:
	cargo build --release
	install -Dm755 target/release/ctsq $(INSTALL_DIR)/ctsq
