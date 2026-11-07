# Sourced by shell/check.sh and glue/check.sh for --target TRIPLE, inside their check container (CARGO_HOME set).
# target_std TRIPLE puts first on PATH a rustup toolchain of the container's own rustc version (so its clippy lints
# are the Linux lane's) with TRIPLE's std: installed once under ${CARGO_HOME%/*}/rustup and target-home (network the
# first time), reused after.  Returns 3 and says why when that can't be had.
target_std() {
  local triple=$1 ver home
  ver=$(rustc --version | awk '{print $2}')
  home=${CARGO_HOME%/*}/target-home
  export RUSTUP_HOME=${CARGO_HOME%/*}/rustup
  command -v rustup-init >/dev/null 2>&1 || { echo "check.sh: --target unavailable: rustup-init is not installed" >&2; return 3; }
  if [ ! -x "$home/bin/rustup" ]; then
    CARGO_HOME=$home rustup-init -y --no-modify-path --profile minimal --default-toolchain none >/dev/null 2>&1 \
      || { echo "check.sh: --target unavailable: rustup-init failed" >&2; return 3; }
  fi
  CARGO_HOME=$home "$home/bin/rustup" toolchain install "$ver" --profile minimal -c clippy -c rustfmt -t "$triple" \
    >/dev/null 2>&1 || { echo "check.sh: --target unavailable: no $ver toolchain with $triple's std (rustup)" >&2; return 3; }
  export RUSTUP_TOOLCHAIN=$ver
  PATH="$home/bin:$PATH"
}
