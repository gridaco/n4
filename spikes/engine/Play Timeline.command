#!/bin/zsh
# Launch the native review instrument from Finder or a terminal on macOS.
set -eu
engine_dir="${0:a:h}"
export PATH="/opt/homebrew/bin:/usr/local/bin:$HOME/.cargo/bin:$PATH"
export LIBRARY_PATH="$(sdl2-config --prefix)/lib${LIBRARY_PATH:+:$LIBRARY_PATH}"
(cd "$engine_dir" && cargo build --locked --features device-probe,native-decode --bin n4-player)
if (( $# )); then
  exec "$engine_dir/target/debug/n4-player" "$@"
fi
exec "$engine_dir/target/debug/n4-player" "$engine_dir/../examples/signal-atlas"
