VERSION := $(shell git describe --tags --always --dirty)
BINARY := target/release/homevault

.PHONY: all clean package release

all: generate-i18n package

generate-i18n:
	python3 scripts/generate_i18n.py

package:
	cargo build --release --bin homevault
	mkdir -p installer/release
	tar -czf installer/release/homevault-$(VERSION).tar.gz \
	    -C target/release homevault

clean:
	cargo clean
	rm -rf installer/release
