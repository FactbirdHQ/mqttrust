#!/bin/bash

set -euxo pipefail

cargo install cargo-fuzz

CPUS=$(nproc)

cargo fuzz run --sanitizer none -j$CPUS pubsub -- -max_total_time=30
