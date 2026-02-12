#!/usr/bin/env bash
args=()
for arg in "$@"; do
  case "$arg" in
    --target=x86_64-unknown-linux-gnu) ;;
    --target) shift ;; # if split form, drop next token
    *) args+=("$arg") ;;
  esac
done
exec zig cc -target x86_64-linux-gnu "${args[@]}"

