#!/bin/bash

set -e

print_help() {
    cat <<EOF
Usage: $0 (-w | -l) [-r | -d] [-h]

Options:
  -w    Build for Windows (x86_64-pc-windows-gnu)
  -l    Build for Linux (x86_64-unknown-linux-musl)
  -r    Build in release mode (adds --release)
  -d    Build in debug mode (default if neither specified)
  -h    Show this help message

Notes:
  -w and -l are mutually exclusive (one is required).
  -r and -d are mutually exclusive.
EOF
}

l_release=0
l_debug=0
l_windows=0
l_linux=0

while getopts ":rdwlh" opt; do
    case "$opt" in
    r) l_release=1 ;;
    d) l_debug=1 ;;
    w) l_windows=1 ;;
    l) l_linux=1 ;;
    h)
        print_help
        exit 0
        ;;
    :)
        echo "Error: option -$OPTARG requires an argument"
        exit 1
        ;;
    \?)
        echo "Error: invalid option -$OPTARG"
        print_help
        exit 1
        ;;
    esac
done

# Validate platform flags
if ((l_windows && l_linux)); then
    echo "Error: -w and -l cannot be used together"
    exit 1
fi

if ((!l_windows && !l_linux)); then
    echo "Error: one of -w or -l must be specified"
    print_help
    exit 1
fi

# Validate build type flags
if ((l_release && l_debug)); then
    echo "Error: -r and -d cannot be used together"
    exit 1
fi

# Build mode
l_build_flag=""
if ((l_release)); then
    l_build_flag="--release"
fi

# Target selection
l_target=""
if ((l_windows)); then
    l_target="--target=x86_64-pc-windows-gnu"
else
    l_target="--target=x86_64-unknown-linux-musl"
fi

RUSTFLAGS='-C target-cpu=native' \
    cargo build $l_target $l_build_flag
