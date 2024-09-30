#!/bin/bash

set -euxo pipefail

cargo install cargo-fuzz

CPUS=16

cargo fuzz run --sanitizer none -j$(nproc) pubsub -- -max_total_time=30
