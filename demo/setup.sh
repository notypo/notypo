#!/usr/bin/env bash
cd "$(mktemp -d)"
git init -q
eval "$(notypo --alias)"
