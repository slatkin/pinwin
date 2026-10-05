BINARY     := pinwin
INSTALL_DIR := $(HOME)/.local/bin

.PHONY: all build clean check-code-file-lines test-check-code-file-lines

all: build

build:
	cargo build --release

clean:
	cargo clean

check-code-file-lines:
	./scripts/check-code-file-lines.sh

test-check-code-file-lines:
	./scripts/check-code-file-lines-test.sh
