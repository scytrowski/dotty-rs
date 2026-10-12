# Frozen resolver prefix attribution for #1022

The pinned #976 report predates identity shells. Its incomplete-class-prefix
section contains **2,744 attributed method/request rows**, representing
**3,771 requests** on **113 distinct prefix identities**. Every state/origin
bucket is `Missing` / `source`. No recorded prefix in this cohort is an external
identity registered by #1019, so **zero frozen rows qualify for the #1022
classpath completion rule**.

For every row the classification of its frozen input state is
`remains_unresolved_unowned_source_prefix`: the resolver cannot complete a
source declaration merely because a classfile with that name might exist.
Source completion remains the source frontend's responsibility. No row in this
cohort can be reported as newly successful, ambiguous, reaching generic
adaptation, or reaching another Typer blocker based on this evidence.

This is an eligibility analysis of the frozen request states, not a re-run of
current source methods. It does not claim the current source pipeline observes
all these same requests, nor that all `ImportQualifierNotFound` rows are
affected. Other historical resolution failures and the new shells introduced
by #1019/#1020 require the separate #1023 remeasurement.

The input remains the unchanged
[pinned audit report](typer-classpath-corpus-audit-3.9.0.md), targeting Scala
revision `777528f19a58e794c9954a42f433373472ec57f8`. To enumerate every frozen
method, prefix, namespace, member key, request space, count, state/origin, and
classification:

```sh
python3 tools/typer-classpath-corpus-audit/frozen_prefix_cohort.py \
  > /tmp/frozen-prefix-cohort.tsv
```

The tool reconciles per-method counts with the origin/state buckets and fails
on missing or conflicting attribution. An external origin alone is not proof
of session ownership; such a future input is labeled
`requires_owned_session_replay`, rather than guessed successful. Output is
sorted deterministically. Parser coverage is run with:

```sh
PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover \
  -s tools/typer-classpath-corpus-audit -p 'test_frozen_prefix_cohort.py'
```

Current audit instrumentation separately reports successful external
class-prefix declarations by field, method, constructor, type/nested member,
and other. It counts requests satisfied after shell completion, distinct
canonical actual-owner classes/member keys, and requests reusing already
complete prefixes. Explicit request owners are used instead of displayed
prefixes; package-type materialization is excluded. Counts describe observed
successful requests even if a later outer expression transaction rolls back.

For `Ok(None)`, a successful load profile rooted at the actual lookup owner
records that its class scope was available during the request, even though
the resolver restores the shell to `Missing`. The outcome and unresolved-kind
counter use the existing `other resolver Ok(None)` category, rather than
reporting an incomplete class identity. Unowned shells without such an
operation observation retain the incomplete-identity classification.
