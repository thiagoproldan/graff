#!/usr/bin/env bash
# Copies every profile's Claude Code transcripts into an archive that Claude
# Code's own cleanup, which deletes them after 30 days, never touches. Run it
# before reading transcripts: it only adds and updates, never deletes.
#
#   evals/transcripts.sh [archive]    # default ~/.local/share/graff/transcripts
set -euo pipefail

archive="${1:-${XDG_DATA_HOME:-$HOME/.local/share}/graff/transcripts}"
for profile in "$HOME"/.claude*/; do
  [ -d "$profile/projects" ] || continue
  name=$(basename "$profile")
  mkdir -p "$archive/${name#.}"
  rsync -a "$profile/projects/" "$archive/${name#.}/"
  printf '%s: %s files\n' "${name#.}" "$(find "$archive/${name#.}" -name '*.jsonl' | wc -l)"
done
