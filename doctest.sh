#!/usr/bin/env bash
set -e

function join_by { local IFS="$1"; shift; echo "$*"; }

DEPS=$(cat Cargo.toml | grep -v '#' | grep -v '^\[' | sed -r 's/^([a-z_-]+) =.*/\1/')
REGEX=$(join_by '|' ${DEPS})
RLIBS=$(ls target/debug/deps/*.rlib | egrep "${REGEX//-/_}" | sed -r 's/(.+lib(\w+)-.*\.rlib)/--extern \2=\1/')

export CARGO_PKG_VERSION="0.1"
export CARGO_PKG_AUTHORS="MS"
export CARGO_PKG_DESCRIPTION="wcnt"

rustc --edition=2018 -A dead_code --crate-type lib  \
 -C metadata=e9baf7ccf9face52 \
  --out-dir 'target/debug/deps' \
 ${RLIBS} \
 -L 'dependency=target/debug/deps' \
 --crate-name wcnt src/main.rs

rustdoc --test --edition=2018 --crate-name wcnt src/main.rs --extern wcnt=target/debug/deps/libwcnt.rlib ${RLIBS} \
 -C metadata=e9baf7ccf9face52 \
 -L 'dependency=target/debug/deps'
