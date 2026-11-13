#!/usr/bin/env python3
"""Verify fixed master gate IDs and complete, unique requirement traceability."""
from collections import Counter
from pathlib import Path
import re

root = Path(__file__).resolve().parents[1]
master = (root / 'docs/Bradian_Master_Architecture_SRS_v1.1.md').read_text()
trace = (root / 'docs/implementation/BRADIAN_MASTER_TRACEABILITY.md').read_text()
roadmap = (root / 'docs/implementation/BRADIAN_MASTER_ROADMAP.md').read_text()
expected = set(re.findall(r'^\| (BRD-[A-Z]+-\d+) \|', master, re.M))
actual = re.findall(r'^\| (BRD-[A-Z]+-\d+) \|', trace, re.M)
assert expected, 'Master requirement IDs missing'
assert set(actual) == expected, f'Missing/unknown requirement IDs: {expected ^ set(actual)}'
assert all(count == 1 for count in Counter(actual).values()), 'Duplicate traceability ID'
for line in trace.splitlines():
    if not line.startswith('| BRD-'):
        continue
    columns = [column.strip() for column in line.split('|')[1:-1]]
    assert columns[1] in set(map(str, range(10))), f'Unknown master gate: {line}'
    assert columns[2] in {'PARTIAL', 'INHERITED', 'OPEN'}, f'Unsupported acceptance claim: {line}'
    for target in re.findall(r'\]\(([^)]+)\)', line):
        assert (root / 'docs/implementation' / target).exists(), f'Broken source: {target}'
gates = re.findall(r'^\| (\d+) \|', roadmap, re.M)
assert gates == list(map(str, range(10))), f'Master gate sequence changed: {gates}'
for path in ['docs/security/BRADIAN_THREAT_MODEL.md', 'docs/implementation/BRADIAN_SOURCE_INVENTORY.md',
             'docs/adr/0040-native-local-sql-cli.md']:
    assert (root / path).is_file(), f'Missing Gate 0 evidence: {path}'
print(f'Passed: {len(expected)} unique master requirements; fixed Gate 0–9 sequence; evidence links exist.')
