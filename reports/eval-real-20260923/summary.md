# Real model evaluation

Model: `deepseek/deepseek-v4.1-flash`. No seed support; every run starts from a clean fixture.
Success requires a completed runtime, external command checks, protected files, allowed paths, and host-only held-out tests.
Held-out tests are executed after the model run and are never copied into its workspace.
Gateway cost is unknown unless a dated price table is supplied; token counts are recorded without an invented dollar value.
A timeout before any model response is counted as an infrastructure/response timeout and remains in the end-to-end denominator.

| Baseline | Final pass | Visible pass | Hidden pass | Runs | False complete | Infra/response timeout | Median s | P95 s | Input tokens | Output tokens |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| b0 | 47 | 47 | 54 | 60 | 0 | 3 | 65.765 | 120.031 | 752531 | 35413 |
| b2 | 51 | 51 | 58 | 60 | 0 | 1 | 67.953 | 120.032 | 749751 | 34968 |

| Task | B0 passed/runs | B2 passed/runs |
| --- | ---: | ---: |
| cache_ttl | 3/3 | 3/3 |
| comparison_reversed | 3/3 | 3/3 |
| dedup_logic | 2/3 | 3/3 |
| dict_key_typo | 2/3 | 3/3 |
| division_guard | 3/3 | 3/3 |
| empty_list_bounds | 3/3 | 2/3 |
| fib_seed | 3/3 | 3/3 |
| float_rounding | 2/3 | 2/3 |
| loop_off_by_one | 3/3 | 3/3 |
| min_max_reversed | 3/3 | 3/3 |
| missing_return | 3/3 | 3/3 |
| mutable_default | 2/3 | 3/3 |
| pagination_bounds | 2/3 | 3/3 |
| queue_fifo | 0/3 | 3/3 |
| regex_mismatch | 3/3 | 2/3 |
| slugify_unicode | 1/3 | 1/3 |
| sort_key | 1/3 | 3/3 |
| string_format | 3/3 | 1/3 |
| unit_conversion | 3/3 | 1/3 |
| weekday_math | 2/3 | 3/3 |

Raw records and SQLite traces are under `runs/`.
