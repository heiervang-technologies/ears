#!/usr/bin/env bash

set -euo pipefail

requested_tag="${1:-}"

# `--locked` fails if Cargo.toml and Cargo.lock disagree.
cargo metadata --locked --no-deps --format-version 1 >/dev/null

package_id=$(cargo pkgid)
package_version="${package_id##*#}"
package_version="${package_version##*@}"
expected_tag="v${package_version}"

if [[ -n "$requested_tag" && "$requested_tag" != "$expected_tag" ]]; then
  echo "Version mismatch: Cargo package is ${package_version}, but tag is ${requested_tag}" >&2
  exit 1
fi

echo "Version metadata is consistent: package ${package_version}, tag ${expected_tag}"
