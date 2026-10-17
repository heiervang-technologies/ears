# Benchmark: oracle-text

| input | variant | steps passed (resync) | exact | failed | final exact | final ruff | final AST | parses |
|---|---|---|---|---|---|---|---|---|
| text | strict | 77/78 (99%) | 99% | 1 | 2/5 | 5/5 | 5/5 | 5/5 |
| text | natural | 62/64 (97%) | 95% | 2 | 2/5 | 5/5 | 5/5 | 5/5 |

## Per task (resync steps passed; final AST-equal in the chain run)

| task | text strict | text natural |
|---|---|---|
| 1-argparse-cli | 14/14 ✓ | 14/14 ✓ |
| 2-add-method | 11/12 ✓ | 11/12 ✓ |
| 3-refactor-load | 20/20 ✓ | 14/14 ✓ |
| 4-dataclass | 16/16 ✓ | 15/16 ✓ |
| 5-retry-fix | 16/16 ✓ | 8/8 ✓ |

## Latency (median / p90)

- ASR (uncached requests, with context): -
- translate + apply (event to translator reply, excluding each run's first): 2 / 3 ms (n=272)
- first utterance of a run (translator start-up): 58 / 67 ms (n=10)

## Failed steps (resync run; cascades counted, not listed)

translator: 2, ears-command: 1

- `2-add-method#4` text/strict (ears-command): said 'new line' → line '        ', mode i, failed text,row
- `2-add-method#9` text/natural (translator): said 'blank line' → line '        ', mode i, failed text,row [ears: blank line → <CR>]
- `4-dataclass#12` text/natural (translator): said 'two blank lines' → line '    ', mode i, failed text,row [ears: two blank lines → <CR><CR>]
