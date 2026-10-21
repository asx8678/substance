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
# §4.1: only intake may grant the permission to run repo-controlled code, and only when
# --allow-exec was given. Every other crate has to ask the caller for a &ExecPermit.
check "ExecPermit grant outside crates/runner and orchestrator intake (M1-T01)" \
  rg -n --glob '*.rs' --glob '!**/runner/src/**' --glob '!**/orchestrator/src/intake.rs' \
    'ExecPermit::(new|grant|default)' "${roots[@]}"
# M1-T07: the rule pack is embedded with include_str!, which needs a literal path, so a rule
# file added to rules/ but not to FILES would be silently ignored — a rule nobody runs. The
# guard is what makes "embedded" a property of the tree rather than of one list.
if [ -d rules ]; then
  check "every rules/<lang>/*.yml file is listed in the embedded rule pack (M1-T07)" \
    sh -c 'for f in rules/*/*.yml; do [ -e "$f" ] || continue
      grep -Fq "\"$f\"" crates/statics/src/rulepack.rs || echo "$f"; done | grep -q .'
  check "every rule file carries metadata.check_id and metadata.sub_check (M1-T07)" \
    sh -c 'for f in rules/*/*.yml; do [ -e "$f" ] || continue
      grep -q "^  check_id:" "$f" && grep -q "^  sub_check:" "$f" || echo "$f"; done | grep -q .'
fi
# §1 rule 9: substance evaluates its own tree, and S05 flags one-off scripts, so a scratch
# file left at the root is a defect the evaluator would report. The guard reads the *index*,
# not the work tree: a scratch file that is not committed cannot fail rule 9, and a `tmp_*.py`
# a developer is using locally must not break the build. `--directory` collapses the output to
# one line per tracked path, so the root files are the ones with no `/` in them.
check "no tracked scratch files at the repository root (rule 9)" \
  sh -c 'git ls-files --directory | grep -v / | grep -Eq "^(tmp_|scratch)|\.(orig|rej)$"'
exit $fail
