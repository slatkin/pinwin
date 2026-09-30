.PHONY: all install

all:
	cd penguin && zig build

install: all
	install -Dm755 penguin/zig-out/bin/penguin "$(HOME)/.local/bin/penguin"
