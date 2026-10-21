#!/usr/bin/env bash
# Record the sha256 of the files official scaffolds ship, for S04 and H17 (M1-T08).
#
# WHY THIS IS A SCRIPT AND NOT A COMMITTED TABLE
#
# S04's hash half and H17's scaffold comparison need to know what `create-vite`, `create-next-app`
# and `uv init` actually produce. Those bytes must be recorded from a real template at a real
# version, never invented: a made-up digest matches nothing, and a wrong one would accuse a
# project of being a template it is not.
#
# Running a scaffolder needs the network and installs packages, which is why this is not part of
# any automated check (docs/ACTION_PLAN.md §1). The owner runs it once per scaffold version and
# commits the result; until then `rules/scaffolds.toml` carries only the text markers and S04's
# hash part reports `not_measured` in `parts`.
#
# WHAT IT DOES
#
# For each scaffold: create a throwaway project at the pinned version with every generator
# prompt suppressed, hash every file it produced, and write the entries as [[files]] blocks
# into rules/scaffolds.toml. The marker list in that file is left alone.
#
# USAGE
#
#   scripts/scaffold-hashes.sh            # all three scaffolds, writes rules/scaffolds.toml
#   scripts/scaffold-hashes.sh vite       # one of: vite, next, uv
#   scripts/scaffold-hashes.sh --dry-run  # print the entries it would write
#
# REQUIREMENTS
#
#   node + npm   for create-vite and create-next-app
#   uv           for `uv init`
#   sha256sum    coreutils; on macOS `shasum -a 256` is used instead
#
# The versions below are the ones `rules/scaffolds.toml` records in [[scaffolds.sources]]. Bump
# them together: a version in one place and not the other makes the recorded hashes describe a
# template the file does not name.
set -uo pipefail

readonly VITE_VERSION="5.5.4"
readonly NEXT_VERSION="14.2.5"
readonly UV_VERSION="0.4.27"
readonly REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
readonly OUT="${REPO_ROOT}/rules/scaffolds.toml"

DRY_RUN=0
TARGETS=()
for arg in "$@"; do
  case "$arg" in
    --dry-run) DRY_RUN=1 ;;
    vite|next|uv) TARGETS+=("$arg") ;;
    -h|--help) sed -n '2,40p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "unknown argument: $arg" >&2; exit 2 ;;
  esac
done
[ ${#TARGETS[@]} -eq 0 ] && TARGETS=(vite next uv)

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

sha256_of() { # $1 = file
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d' ' -f1
  else
    shasum -a 256 "$1" | cut -d' ' -f1
  fi
}

entries() { # $1 = tool, $2 = version, $3 = project dir
  local tool=$1 version=$2 dir=$3
  ( cd "$dir" && find . -type f -not -path './.git/*' -not -path './node_modules/*' -not -path './.venv/*' | LC_ALL=C sort ) |
  while IFS= read -r rel; do
    local path=${rel#./}
    printf '[[files]]\npath = "%s"\nsha256 = "%s"\ntool = "%s"\nversion = "%s"\n\n' \
      "$path" "$(sha256_of "$dir/$path")" "$tool" "$version"
  done
}

collect_vite() {
  command -v npm >/dev/null || { echo "create-vite needs npm" >&2; return 1; }
  local dir="$WORK/vite"
  ( cd "$WORK" && npm create "vite@${VITE_VERSION}" vite -- --template react-ts --no-interactive >/dev/null 2>&1 ) || {
    echo "create-vite ${VITE_VERSION} failed" >&2; return 1; }
  entries create-vite "$VITE_VERSION" "$dir"
}

collect_next() {
  command -v npx >/dev/null || { echo "create-next-app needs npx" >&2; return 1; }
  local dir="$WORK/next"
  ( cd "$WORK" && npx --yes "create-next-app@${NEXT_VERSION}" next \
      --ts --eslint --app --src-dir --import-alias '@/*' --no-tailwind --use-npm --no-git \
      >/dev/null 2>&1 ) || { echo "create-next-app ${NEXT_VERSION} failed" >&2; return 1; }
  entries create-next-app "$NEXT_VERSION" "$dir"
}

collect_uv() {
  command -v uv >/dev/null || { echo "uv init needs uv" >&2; return 1; }
  local dir="$WORK/uv"
  ( cd "$WORK" && uv init --name uv-init uv >/dev/null 2>&1 ) || {
    echo "uv init ${UV_VERSION} failed" >&2; return 1; }
  entries uv "$UV_VERSION" "$dir"
}

BODY="$WORK/files.toml"
: > "$BODY"
for target in "${TARGETS[@]}"; do
  case "$target" in
    vite) collect_vite >> "$BODY" || exit 1 ;;
    next) collect_next >> "$BODY" || exit 1 ;;
    uv) collect_uv >> "$BODY" || exit 1 ;;
  esac
done

# The [[files]] section is everything from the first entry onward; the markers above it are
# hand-written and must survive. The file's own comment block says the same thing.
if [ "$DRY_RUN" -eq 1 ]; then
  echo "# would replace the [[files]] section of $OUT with:"
  cat "$BODY"
  exit 0
fi

if grep -q '^\[\[files\]\]' "$OUT"; then
  awk '/^\[\[files\]\]/{exit} {print}' "$OUT" > "$OUT.tmp"
else
  cat "$OUT" > "$OUT.tmp"
fi
printf '# File hashes: path, sha256, and the scaffold + version it came from.\n# Written by scripts/scaffold-hashes.sh; do not edit by hand.\n\n' >> "$OUT.tmp"
cat "$BODY" >> "$OUT.tmp"
mv "$OUT.tmp" "$OUT"
echo "wrote $(grep -c '^\[\[files\]\]' "$OUT") file hashes to $OUT"
echo "S04's hash part and H17's scaffold comparison now have data; re-run C-RUST."
