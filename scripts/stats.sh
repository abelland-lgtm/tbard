#!/usr/bin/env bash
# Download and traffic statistics for tbard.
#
# Usage: scripts/stats.sh
# Needs gh and jq. Repository traffic is only visible to the owner, so it uses
# the gh account in TBARD_GH_USER (default abelland-lgtm) when it is logged in.
#
# Homebrew and Scoop download the binaries from GitHub Releases, so their
# installs show up in the per-platform release counts. Nix builds from source
# and has no counter. Counts include CI runs and bots.
set -euo pipefail

repo="abelland-lgtm/tbard"
crate="tbard"

for cmd in gh jq curl; do
  command -v "$cmd" >/dev/null || { echo "error: $cmd is not installed" >&2; exit 1; }
done

bold=$'\e[1m' dim=$'\e[2m' accent=$'\e[33m' off=$'\e[0m'
[[ -t 1 ]] || bold='' dim='' accent='' off=''
title() { printf '\n%s%s%s\n' "$bold$accent" "$1" "$off"; }

releases=$(gh api "repos/$repo/releases" --paginate)

title "GitHub Releases"
jq -r --arg dim "$dim" --arg off "$off" '
  .[] | "\(.tag_name)  \($dim)\(.published_at[:10])\($off)",
  (.assets[] | select(.name | test("sha256|manifest|\\.rb$") | not)
             | "  \(.download_count | tostring | (" " * (6 - length)) + .)  \(.name)")
' <<<"$releases"

title "By platform (all releases)"
jq -r '
  [.[].assets[] | select(.name | test("sha256") | not)] as $a
  | def sum(re): [$a[] | select(.name | test(re)) | .download_count] | add // 0;
  "  Linux     \(sum("linux"))",
  "  macOS     \(sum("apple-darwin"))",
  "  Windows   \(sum("windows"))  (msi \(sum("\\.msi$")), zip \(sum("windows-msvc\\.zip$")))",
  "  installer \(sum("installer\\.sh$")) sh, \(sum("installer\\.ps1$")) ps1"
' <<<"$releases"

title "crates.io"
curl -fsS -A "tbard-stats (https://github.com/$repo)" "https://crates.io/api/v1/crates/$crate" |
  jq -r '"  total \(.crate.downloads), last 90 days \(.crate.recent_downloads)",
         (.versions[] | "  \(.num): \(.downloads)")'

title "Repository traffic (last 14 days)"
user="${TBARD_GH_USER:-abelland-lgtm}"
if token=$(gh auth token -u "$user" 2>/dev/null); then
  export GH_TOKEN="$token"
  gh api "repos/$repo/traffic/views" --jq '"  views   \(.count) (\(.uniques) unique)"'
  gh api "repos/$repo/traffic/clones" --jq '"  clones  \(.count) (\(.uniques) unique)"'
  gh api "repos/$repo/traffic/popular/referrers" --jq '.[] | "  from \(.referrer): \(.count) (\(.uniques) unique)"'
  gh api "repos/$repo" --jq '"  stars \(.stargazers_count), watchers \(.subscribers_count), forks \(.forks_count)"'
else
  echo "  ${dim}not available: log in with \`gh auth login\` as $user (owner only)${off}"
fi
