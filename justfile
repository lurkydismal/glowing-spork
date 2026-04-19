#!/usr/bin/env -S just --justfile
# Export a Rust compiler flag for all recipes in this Justfile.
# `-C target-cpu=native` tells rustc to optimize for the current CPU.

export RUSTFLAGS := '-C target-cpu=native'

# Pick a target triple based on the host OS family.
# `os_family()` returns `"unix"` or `"windows"`.
# This selects a Linux musl target for Unix-like systems and a GNU Windows target for Windows.

triple := if os_family() == "unix" { "x86_64-unknown-linux-musl" } else { "x86_64-pc-windows-gnu" }

default: run-debug

# Build a debug binary for the selected target triple.
build-debug triple=triple:
    cargo build --target='{{ triple }}'

# Run the debug binary for the selected target triple.
run-debug triple=triple:
    cargo run --target='{{ triple }}'

# Build a release binary for the selected target triple.
build-release triple=triple:
    cargo build --release --target='{{ triple }}'

# Run the release binary for the selected target triple.
run-release triple=triple:
    cargo run --release --target='{{ triple }}'
