.PHONY: all install

all:
	cd pinwin && zig build

install: all
	install -Dm755 pinwin/zig-out/bin/pinwin "$(HOME)/.local/bin/pinwin"
