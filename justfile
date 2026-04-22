#!/usr/bin/env -S just --justfile
# Export a Rust compiler flag for all recipes in this Justfile.
# `-C target-cpu=native` tells rustc to optimize for the current CPU.

export CC := 'clang'
export CXX := 'clang++'
export RUSTFLAGS := '-C target-cpu=native -C linker=clang -C link-arg=-fuse-ld=mold'

# Pick a target triple based on the host OS family.
# `os_family()` returns `"unix"` or `"windows"`.
# This selects a Linux musl target for Unix-like systems and a GNU Windows target for Windows.

triple := if os_family() == "unix" { "x86_64-unknown-linux-gnu" } else { "x86_64-pc-windows-gnu" }

default: run-debug

# Build a debug binary for the selected target triple.
build-debug triple=triple:
    cargo build --target='{{ triple }}'

# Run the debug binary for the selected target triple.
run-debug triple=triple:
    -cargo run --target='{{ triple }}'

# Build a release binary for the selected target triple.
build-release triple=triple:
    cargo build --release --target='{{ triple }}'

# Run the release binary for the selected target triple.
run-release triple=triple:
    -cargo run --release --target='{{ triple }}'

# Pull images for all services, skipping services that have no build context.
docker-pull:
    docker compose  pull --ignore-buildable

# Start all services.
docker-up-all:
    just docker-up

# Start one service in detached mode.
docker-up image='postgres':
    docker compose up -d '{{ image }}'

# Stop all running containers in the current Docker Compose project without removing containers, networks, or volumes.
docker-stop:
    docker compose stop

# Stop and remove containers, networks, and default resources created by the current Docker Compose project.
docker-down:
    docker compose down

# Open an interactive shell inside a running service container.
docker-interact image='postgres':
    docker compose exec '{{ image }}' bash

# Attach to a running service container without signal proxying.
docker-attach image='postgres':
    -docker compose attach --sig-proxy=false '{{ image }}' sh

# Show a one-time snapshot of resource usage statistics for containers in the current Docker Compose project.
docker-stats:
    docker compose stats --no-stream

# Remove dangling Docker images that are no longer referenced by any tag.
docker-remove-unused-images:
    #!/usr/bin/env bash
    set -euo pipefail
    docker images -f "dangling=true" -q | xargs -r docker rmi

# Generate SeaORM entity models from a database schema. Uses the provided database URL and schema name (defaults to "public"), and outputs compact-format entities into the `src/entity` directory.
# sea-generate-entity database_url schema='public':
#     sea generate entity --compact-format -u {{ database_url }} -s {{ schema }} -o src/entity

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
