#!/usr/bin/env bash
sed -E 's/@sha256:[[:alnum:]]+//g' Dockerfile > Dockerfile.any-platform
echo "Created platform-agnostic Dockerfile in 'Dockerfile.any-platform'"
