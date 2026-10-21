#!/usr/bin/env bash
# Repository guards, run by CI and by every task's checks.
# ADR-0008: no tests. ADR-0009: determinism hazards.
set -uo pipefail
command -v rg >/dev/null || { echo "guard: ripgrep (rg) is required"; exit 2; }
fail=0
check() { # $1 = message; the remaining args are a command that must find nothing
  local msg=$1; shift
  "$@"; local rc=$?
  if [ $rc -eq 0 ]; then echo "guard: $msg"; fail=1
  elif [ $rc -gt 1 ]; then echo "guard: error while checking: $msg"; fail=1; fi
}
roots=()
for d in crates driver; do [ -d "$d" ] && roots+=("$d"); done
[ ${#roots[@]} -eq 0 ] && exit 0
check "test attributes are not allowed (ADR-0008)" \
  rg -n --glob '*.rs' '#\[(tokio::)?test\]|#\[cfg\(test\)\]' "${roots[@]}"
check "tests/, benches/ and __tests__/ directories are not allowed (ADR-0008)" \
  sh -c 'find "$@" -type d \( -name tests -o -name benches -o -name __tests__ \) -not -path "*/node_modules/*" | grep -q .' _ "${roots[@]}"
check "test files are not allowed (ADR-0008)" \
  sh -c 'find "$@" \( -name "*.test.ts" -o -name "*.spec.ts" \) -not -path "*/node_modules/*" | grep -q .' _ "${roots[@]}"
check "wall clock outside core/src/clock.rs (ADR-0009)" \
  rg -n --glob '*.rs' --glob '!**/core/src/clock.rs' 'SystemTime::now|Utc::now|Local::now|OffsetDateTime::now|Timestamp::now|Zoned::now' "${roots[@]}"
check "unseeded randomness (ADR-0009)" \
  rg -n --glob '*.rs' 'thread_rng|rand::random|from_entropy|OsRng|rand::rng\(' "${roots[@]}"
check "HashMap/HashSet: use BTreeMap/BTreeSet (ADR-0009)" \
  rg -n --glob '*.rs' '\bHash(Map|Set)\b' "${roots[@]}"
check "process spawning outside crates/runner" \
  rg -n --glob '*.rs' --glob '!**/runner/src/**' 'Command::new' "${roots[@]}"
exit $fail
