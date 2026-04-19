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

# Pull images for all services, skipping services that have no build context.
docker-pull:
    docker compose  pull --ignore-buildable

# Start the app service and any dependencies needed by that service.
docker-up-all:
    just docker-up 'app'

# Start one service in detached mode.
docker-up image='postgres':
    docker compose up -d '{{ image }}'

# Open an interactive shell inside a running service container.
docker-interact image='postgres':
    docker compose exec '{{ image }}' bash

# Attach to a running service container without signal proxying.
docker-attach image='postgres':
    docker compose attach --sig-proxy=false '{{ image }}' sh || true

# Remove dangling Docker images that are no longer referenced by any tag.
docker-remove-unused-images:
    #!/usr/bin/env bash
    set -euxo pipefail
    docker rmi $(docker images -f "dangling=true" -q)

# Generate a cryptographically secure random alphanumeric token of length `N`. Uses `openssl rand` as the entropy source, encodes as Base64, removes padding and non-alphanumeric output, then retries until the result is exactly `N` characters using only `[A-Za-z0-9]`.
generate-token length='32':
    #!/usr/bin/env bash
    set -euo pipefail
    while true; do
    s=$(openssl rand -base64 "$(({{ length }} + 3))" | tr -d '\n')
    s="${s%%=*}"   # remove all trailing '='
    s="${s:0:{{ length }}}"   # cut back to requested length
    # allow only base64 alnum
    if [[ "$s" =~ ^[A-Za-z0-9]+$ && ${#s} -eq {{ length }} ]]; then
        echo "$s"
        break
    fi
    done
