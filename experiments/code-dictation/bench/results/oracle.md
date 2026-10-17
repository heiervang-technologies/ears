# Benchmark: oracle

| input | variant | steps passed (resync) | exact | failed | final exact | final ruff | final AST | parses |
|---|---|---|---|---|---|---|---|---|
| text | strict | 77/78 (99%) | 99% | 1 | 2/5 | 5/5 | 5/5 | 5/5 |
| text | natural | 62/64 (97%) | 95% | 2 | 2/5 | 5/5 | 5/5 | 5/5 |
| af_heart | strict | 68/78 (87%) | 87% | 10 | 1/5 | 1/5 | 1/5 | 4/5 |
| af_heart | natural | 53/64 (83%) | 81% | 11 | 2/5 | 2/5 | 2/5 | 5/5 |
| espeak:en-us+m3 | strict | 50/78 (64%) | 64% | 28 | 0/5 | 0/5 | 0/5 | 4/5 |
| espeak:en-us+m3 | natural | 39/64 (61%) | 59% | 25 | 0/5 | 0/5 | 0/5 | 5/5 |

## Per task (resync steps passed; final AST-equal in the chain run)

| task | text strict | text natural | af_heart strict | af_heart natural | espeak:en-us+m3 strict | espeak:en-us+m3 natural |
|---|---|---|---|---|---|---|
| 1-argparse-cli | 14/14 ✓ | 14/14 ✓ | 10/14 | 11/14 | 6/14 | 9/14 |
| 2-add-method | 11/12 ✓ | 11/12 ✓ | 9/12 | 8/12 | 5/12 | 5/12 |
| 3-refactor-load | 20/20 ✓ | 14/14 ✓ | 19/20 | 14/14 ✓ | 16/20 | 10/14 |
| 4-dataclass | 16/16 ✓ | 15/16 ✓ | 14/16 | 12/16 | 10/16 | 8/16 |
| 5-retry-fix | 16/16 ✓ | 8/8 ✓ | 16/16 ✓ | 8/8 ✓ | 13/16 | 7/8 |

## Latency (median / p90)

- ASR (uncached requests, with context): 89 / 164 ms (n=541)
- af_heart: word error rate 10%, utterances heard word-perfect 121/142; of those, passed 118/121
- espeak:en-us+m3: word error rate 32%, utterances heard word-perfect 89/142; of those, passed 86/89
- translate + apply (event to translator reply, excluding each run's first): 2 / 3 ms (n=816)
- first utterance of a run (translator start-up): 57 / 60 ms (n=30)

## Failed steps (resync run; cascades counted, not listed)

asr: 68, translator: 6, ears-command: 3

- `2-add-method#4` text/strict (ears-command): said 'new line' → line '        ', mode i, failed text,row
- `2-add-method#9` text/natural (translator): said 'blank line' → line '        ', mode i, failed text,row [ears: blank line → <CR>]
- `4-dataclass#12` text/natural (translator): said 'two blank lines' → line '    ', mode i, failed text,row [ears: two blank lines → <CR><CR>]
- `1-argparse-cli#1` af_heart/strict (asr): said 'import argparse' heard 'Import arg parse.' → line '', mode i, failed text
- `1-argparse-cli#4` af_heart/strict (asr): said 'parser equals argparse dot pascal argument parser of description equals string count words in a file' heard 'Parser equals arg parse dot pascal argument parser of description equals string count words in a file.' → line 'def parse_args():', mode i, failed text,row
- `1-argparse-cli#9` af_heart/strict (asr): said 'dedent def main' heard 'Dead and deaf, main.' → line '    ', mode i, failed text
- `1-argparse-cli#13` af_heart/strict (asr): said 'dedent if dunder name equals equals string dunder main' heard 'If dunder name equals equals string dunder main.' → line '    ', mode i, failed text
- `1-argparse-cli#1` af_heart/natural (asr): said 'import argparse' heard 'Import arg parse.' → line '', mode i, failed text
- `1-argparse-cli#4` af_heart/natural (asr): said 'parser equals argparse dot ArgumentParser with description count words in a file' heard 'Parser equals arg parse dot argument parser with description count words in a file.' → line 'def parse_args():', mode i, failed text,row
- `1-argparse-cli#9` af_heart/natural (asr): said 'def main' heard 'Deaf main.' → line '    ', mode i, failed text
- `2-add-method#4` af_heart/strict (ears-command): said 'new line' heard 'New line.' → line '        ', mode i, failed text,row
- `2-add-method#5` af_heart/strict (asr): said 'dedent def remove taking self comma name' heard 'Dead and deaf. Remove taking self, comma name.' → line '        ', mode i, failed text
- `2-add-method#10` af_heart/strict (asr): said 'dedent def total taking self' heard 'Dead and deaf. Total taking self.' → line '        ', mode i, failed text
- `2-add-method#5` af_heart/natural (asr): said 'def remove self comma name' heard 'Def remove self common name.' → line '        ', mode i, failed text
- `2-add-method#8` af_heart/natural (asr): said 'dedent del self dot items in square brackets name' heard 'self name items def add inv Inventory count not if in remove KeyError apple class get raise __init__.' → line '            raise KeyError(name)', mode i, failed text,row
- `2-add-method#9` af_heart/natural (translator): said 'blank line' heard 'Blank line.' → line '        ', mode i, failed text,row [ears: Blank line. → <CR>]
- `2-add-method#10` af_heart/natural (asr): said 'def total self' heard 'Deaf total self.' → line '        ', mode i, failed text
- `3-refactor-load#11` af_heart/strict (asr): said 'open above' heard 'Open a buff.' → line '    return data', mode n, failed text,mode [ears: not a command: Open a buff.]
- `4-dataclass#7` af_heart/strict (asr): said 'at sign dataclass' heard 'At signed data class.' → line '', mode i, failed text
- `4-dataclass#13` af_heart/strict (asr): said 'dedent def unpaid total taking orders' heard 'Debent, deaf, unpaid, total, taking orders.' → line '    ', mode i, failed text
- `4-dataclass#7` af_heart/natural (asr): said 'at dataclass' heard 'At data class.' → line '', mode i, failed text
- `4-dataclass#9` af_heart/natural (asr): said 'id int' heard "It ain't." → line 'class Order:', mode i, failed text,row
- `4-dataclass#12` af_heart/natural (translator): said 'two blank lines' heard 'Two blank lines.' → line '    ', mode i, failed text,row [ears: Two blank lines. → <CR><CR>]
- `4-dataclass#13` af_heart/natural (asr): said 'def unpaid total of orders' heard 'Deaf unpaid total of orders.' → line '    ', mode i, failed text
- `1-argparse-cli#1` espeak:en-us+m3/strict (asr): said 'import argparse' heard 'Import iron bars.' → line '', mode i, failed text
- `1-argparse-cli#3` espeak:en-us+m3/strict (asr): said 'def parse args' heard 'The parsons.' → line '', mode i, failed text
- `1-argparse-cli#4` espeak:en-us+m3/strict (asr): said 'parser equals argparse dot pascal argument parser of description equals string count words in a file' heard 'Parser equals hard parser dot Pascal argument. Parser of description equals string count words in a file.' → line 'def parse_args():', mode i, failed text,row
- `1-argparse-cli#6` espeak:en-us+m3/strict (asr): said 'parser dot add argument of quote dash dash top quote comma type equals int comma default equals ten' heard 'parser dot an argument of quote dash dash top quote comma type equals int comma default equals ten.' → line '    parser.add_argument("path")', mode i, failed text,row
- `1-argparse-cli#9` espeak:en-us+m3/strict (asr): said 'dedent def main' heard 'Version def main.' → line '    ', mode i, failed text
- `1-argparse-cli#10` espeak:en-us+m3/strict (asr): said 'args equals parse args of' heard 'parser add_argument argparse parse_args def ArgumentParser description type main words default top count in int return file path' → line 'def main():', mode i, failed text,row
- `1-argparse-cli#13` espeak:en-us+m3/strict (asr): said 'dedent if dunder name equals equals string dunder main' heard 'Predicate under name equals equals string under main.' → line '    ', mode i, failed text
- `1-argparse-cli#14` espeak:en-us+m3/strict (asr): said 'main of' heard 'May of.' → line 'if __name__ == "__main__":', mode i, failed text,row
- `1-argparse-cli#1` espeak:en-us+m3/natural (asr): said 'import argparse' heard 'Import iron bars.' → line '', mode i, failed text
- `1-argparse-cli#4` espeak:en-us+m3/natural (asr): said 'parser equals argparse dot ArgumentParser with description count words in a file' heard 'Parser equal hard parse dot argument parser with description count words in a file.' → line 'def parse_args():', mode i, failed text,row
- `1-argparse-cli#7` espeak:en-us+m3/natural (asr): said 'return parser dot parse args' heard 'Return parser dot parse_args ArgumentParser type in top import int def default path file' → line '    parser.add_argument("--top", type=int, default=10)', mode i, failed text,row
- `1-argparse-cli#9` espeak:en-us+m3/natural (asr): said 'def main' heard 'Death main.' → line '    ', mode i, failed text
- `1-argparse-cli#10` espeak:en-us+m3/natural (asr): said 'args equals parse args' heard 'args equals parse_args add_argument description ArgumentParser words count in int top path import type main default return file' → line 'def main():', mode i, failed text,row
- `2-add-method#1` espeak:en-us+m3/strict (asr): said 'go to def add' heard 'Go to Neffad.' → line 'class Inventory:', mode n, failed row [ears: not a command: Go to Neffad.]
- `2-add-method#2` espeak:en-us+m3/strict (asr): said 'down' heard 'Town.' → line '    def add(self, name, count=1):', mode n, failed row [ears: not a command: Town.]
- `2-add-method#4` espeak:en-us+m3/strict (ears-command): said 'new line' heard 'New line.' → line '        ', mode i, failed text,row
- `2-add-method#5` espeak:en-us+m3/strict (asr): said 'dedent def remove taking self comma name' heard 'Self items name count add def Inventory inv get __init__ apple' → line '        ', mode i, failed text
- `2-add-method#6` espeak:en-us+m3/strict (asr): said 'if name not in self dot items' heard 'If name not in self.name.items,' → line '    def remove(self, name):', mode i, failed text,row
- `2-add-method#8` espeak:en-us+m3/strict (asr): said 'dedent del self dot items index name' heard 'self name items def add inv count Inventory apple class not in raise remove if KeyError get __init__.' → line '            raise KeyError(name)', mode i, failed text,row
- `2-add-method#10` espeak:en-us+m3/strict (asr): said 'dedent def total taking self' heard 'Bad and theft, total taking self.' → line '        ', mode i, failed text
- `2-add-method#5` espeak:en-us+m3/natural (asr): said 'def remove self comma name' heard 'self items name add def Inventory count inv __init__ get class apple' → line '        ', mode i, failed text
- `2-add-method#6` espeak:en-us+m3/natural (asr): said 'if name not in self dot items' heard 'If name not in self.__items.' → line '    def remove(self, name):', mode i, failed text,row
- `2-add-method#7` espeak:en-us+m3/natural (asr): said 'raise key error name' heard 'Raise KeyError name.' → line '        if name not in self.items:', mode i, failed text,row
- `2-add-method#8` espeak:en-us+m3/natural (asr): said 'dedent del self dot items in square brackets name' heard "Don't tell self dot items in square brackets name." → line '            raise KeyError(name)', mode i, failed text,row
- `2-add-method#9` espeak:en-us+m3/natural (translator): said 'blank line' heard 'Blank line.' → line '        ', mode i, failed text,row [ears: Blank line. → <CR>]
- `2-add-method#10` espeak:en-us+m3/natural (asr): said 'def total self' heard 'self total sell.' → line '        ', mode i, failed text
- `2-add-method#11` espeak:en-us+m3/natural (asr): said 'return the sum of self dot items dot values' heard 'Return the sum of self.items.value.' → line '    def total(self):', mode i, failed text,row
- `3-refactor-load#4` espeak:en-us+m3/strict (asr): said 'down' heard 'town.' → line 'def load_config(path):', mode n, failed row [ears: not a command: town.]
- `3-refactor-load#8` espeak:en-us+m3/strict (asr): said 'down' heard 'Town.' → line '    try:', mode n, failed row [ears: not a command: Town.]
- `3-refactor-load#9` espeak:en-us+m3/strict (asr): said 'indent twice' heard 'Invent twice.' → line '    f = open(path)', mode n, failed text [ears: not a command: Invent twice.]
- `3-refactor-load#15` espeak:en-us+m3/strict (asr): said 'go to amount' heard 'Total amount.' → line '        return {}', mode n, failed row [ears: not a command: Total amount.]
- `3-refactor-load#5` espeak:en-us+m3/natural (asr): said 'new line above' heard 'New light above.' → line '    f = open(path)', mode n, failed text,mode [ears: not a command: New light above.]
- `3-refactor-load#9` espeak:en-us+m3/natural (asr): said 'indent two lines' heard 'Invent two lines.' → line '    f = open(path)', mode n, failed text [ears: not a command: Invent two lines.]
- `3-refactor-load#12` espeak:en-us+m3/natural (asr): said 'except file not found error' heard "Except I'll not found error." → line '    ', mode i, failed text
- `3-refactor-load#20` espeak:en-us+m3/natural (asr): said 'delete the print line' heard 'Verlees de printlijn.' → line '        total += row["price"]', mode n, failed text,row [ears: not a command: Verlees de printlijn.]
- `4-dataclass#2` espeak:en-us+m3/strict (asr): said 'from dataclasses import dataclass' heard 'From Vader classes, import Vader class.' → line '', mode i, failed text
- `4-dataclass#7` espeak:en-us+m3/strict (asr): said 'at sign dataclass' heard 'At sign data class.' → line '', mode i, failed text
- `4-dataclass#9` espeak:en-us+m3/strict (asr): said 'id colon int' heard 'I D colon int.' → line 'class Order:', mode i, failed text,row
- `4-dataclass#13` espeak:en-us+m3/strict (asr): said 'dedent def unpaid total taking orders' heard 'Bed and Tefan paid total taking orders.' → line '    ', mode i, failed text
- `4-dataclass#14` espeak:en-us+m3/strict (asr): said 'unpaid equals brackets o dot price for o in orders if not o dot paid' heard 'Unpaid equals bracket so dot price for loaded orders if not oh dot paid.' → line 'def unpaid_total(orders):', mode i, failed text,row
- `4-dataclass#15` espeak:en-us+m3/strict (asr): said 'log dot info of f string brace len of unpaid close unpaid orders' heard 'log.info of f string bracelet of unpaid close unpaid orders.' → line '    unpaid = [o.price for o in orders if not o.paid]', mode i, failed text,row
- `4-dataclass#2` espeak:en-us+m3/natural (asr): said 'from data classes import data class' heard 'From Vader classes, import Vader class.' → line '', mode i, failed text
- `4-dataclass#7` espeak:en-us+m3/natural (asr): said 'at dataclass' heard 'Eerste class.' → line '', mode i, failed text
- `4-dataclass#9` espeak:en-us+m3/natural (asr): said 'id int' heard "I didn't." → line 'class Order:', mode i, failed text,row
- `4-dataclass#11` espeak:en-us+m3/natural (asr): said 'paid bool defaults to false' heard 'Payed bool defaults to false.' → line '    price: float', mode i, failed text,row
- `4-dataclass#12` espeak:en-us+m3/natural (translator): said 'two blank lines' heard 'Two blank lines.' → line '    ', mode i, failed text,row [ears: Two blank lines. → <CR><CR>]
- `4-dataclass#13` espeak:en-us+m3/natural (asr): said 'def unpaid total of orders' heard 'Number of unpaid total of orders.' → line '    ', mode i, failed text
- `4-dataclass#14` espeak:en-us+m3/natural (asr): said 'unpaid equals a list of o dot price for o in orders if not o dot paid' heard 'Unpaid equals a list of o dot price for loaded orders. If not o dot paid.' → line 'def unpaid_total(orders):', mode i, failed text,row
- `4-dataclass#15` espeak:en-us+m3/natural (asr): said 'log dot info f string len of unpaid in braces then unpaid orders' heard 'log.info f string of unpaid in braces and unpaid orders.' → line '    unpaid = [o.price for o in orders if not o.paid]', mode i, failed text,row
- `5-retry-fix#3` espeak:en-us+m3/strict (asr): said 'five' heard "I've." → line 'def fetch(url, retries=, timeout=5):', mode i, failed text
- `5-retry-fix#13` espeak:en-us+m3/strict (asr): said 'go to giving' heard 'rồi sẽ gây viêm.' → line '        time.sleep(2 ** attempt)', mode n, failed row [ears: not a command: rồi sẽ gây viêm.]
- `5-retry-fix#15` espeak:en-us+m3/strict (asr): said 'f string giving up on brace url' heard 'F string giving up on raise URL.' → line '    raise RuntimeError()', mode i, failed text
- `5-retry-fix#11` espeak:en-us+m3/natural (asr): said 'time dot sleep two to the power of attempt' heard 'Time dot sleep to to the power of attempt.' → line '        ', mode i, failed text
