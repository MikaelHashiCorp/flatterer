#!/bin/bash
source scl_source enable llvm-toolset-7.0
exec /usr/bin/maturin "$@"
