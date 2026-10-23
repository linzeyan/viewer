.PHONY: help check bundle icons release

help:
	@echo "make check                  tests and clippy (macOS)"
	@echo "make bundle                 target/release/Viewer.app"
	@echo "make icons                  regenerate icons from packaging/icon.svg"
	@echo "make release VERSION=x.y.z  bump, check, commit, tag and push; CI builds the release"

check:
	cargo test --release
	cargo clippy --release --all-targets -- -D warnings

bundle:
	bash scripts/bundle-macos.sh

icons:
	bash scripts/make-icons.sh

release:
	@test -n "$(VERSION)" || { echo "usage: make release VERSION=x.y.z"; exit 2; }
	bash scripts/release.sh "$(VERSION)"
