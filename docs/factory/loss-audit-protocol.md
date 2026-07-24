# Loss Audit Protocol

Compare evidence preservation across pipeline stages.

## Machine check (implemented)

```sh
cassio audit loss --expected PATH.md --actual PATH.md
```

| Signal | Pass rule |
| --- | --- |
| Quote retention | ≥ 80% of `case_study_quotes` from expected appear in actual |
| Instruction delta retention | 100% of expected `instruction_deltas.summary` present (case-insensitive) |
| Correction counts | `corrections.len()` equal (or volume.corrections equal) |

Gate prints `PASS` / `FAIL` and exits non-zero on FAIL.

## Recommended comparisons

1. **Daily chunks → merged daily** (after Phase C live run)  
2. **Union of week dailies → weekly** (after Phase E)  
3. **Weeklies → monthly** vs **dailies → monthly** (after Phase F)

For multi-file expected sets, merge evidence mechanically first:

```text
parse each daily → evidence::merge_evidence(period, kind, parts) → write temp md
cassio audit loss --expected temp.md --actual weekly.md
```

## Human rubric (narrative invention)

Flag FAIL if monthly/weekly:

- Invents `cost_usd` or agent lists not in evidence/rails
- Claims a rule was codified without `instruction_deltas` support
- Drops a major open_thread that was still open at period end

## Go / no-go for production weeklies

- All machine checks PASS on one real week  
- Human rubric clean on one monthly-from-weeklies sample  
- Then enable writing weeklies into `~/personal/transcripts` with git commit
