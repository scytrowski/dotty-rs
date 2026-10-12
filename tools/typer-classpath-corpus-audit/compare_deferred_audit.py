"""Compare the immutable #976 audit with #1023, preserving frozen row identities."""

import argparse
import collections
import csv
import re
from pathlib import Path


COHORTS = {
    'impure_function_baseline_before_after': '#946',
    'source_function_method_outcomes': '#826-829',
    'immutable_class_field_completion_outcomes': '#856-direct',
    'mutable_class_field_completion_outcomes': '#858-direct',
    'immutable_class_field_baseline_method_outcomes': '#856-methods',
    'mutable_class_field_baseline_method_outcomes': '#858-methods',
    'singleton_projection_baseline': '#880',
    'local_value_baseline_outcomes': '#929',
}


def raw(text):
    marker = '## Full raw audit output'
    return text[text.index(marker):] if marker in text else text


def sections(text):
    result = collections.defaultdict(list)
    section = None
    for line in raw(text).splitlines():
        if re.fullmatch(r'[a-zA-Z_0-9]+:', line):
            section = line[:-1]
        elif section:
            result[section].append(line)
    return result


def requests(text):
    result = collections.defaultdict(collections.Counter)
    method = None
    parts = sections(text)
    lines = parts.get('resolver_observations_by_enclosing_method', parts['resolver_581_comparison'])
    for line in lines:
        if match := re.match(r'^    (.*): requests=\d+, distinct_request_outcomes=', line):
            method = match[1]
        if match := re.match(r'^      (?:target=)?(.*?)::outcome=(.*?)=(\d+)$', line):
            if method is None:
                raise ValueError('request has no method attribution')
            result[method][(match[1], match[2])] += int(match[3])
    return result


def scalar(text, section, name):
    found = [int(m[1]) for line in sections(text)[section]
             if (m := re.match(rf'^  {re.escape(name)}=(\d+)(?:\s|$)', line))]
    if len(found) != 1:
        raise ValueError(f'missing or duplicate metric {section}/{name}')
    return found[0]


def successful_targets(text):
    return {target for outcomes in requests(text).values()
            for target, outcome in outcomes if outcome == 'external class success'}


def gate(baseline, current):
    lost = successful_targets(baseline) - successful_targets(current)
    if lost:
        raise ValueError(f'lost previously successful targets: {sorted(lost)}')
    depth = scalar(current, 'dependency_load_profile', 'p95_dependency_depth')
    fanout = scalar(current, 'dependency_load_profile', 'max_distinct_fully_completed_classes_per_request')
    if depth > 70 or fanout > 524:
        raise ValueError(f'dependency reduction gate failed: p95={depth}, fanout={fanout}')
    return depth, fanout


def cohort_records(text):
    records = {}
    for section, cohort in COHORTS.items():
        for line in sections(text)[section]:
            row = line.strip()
            if row.startswith('field:'):
                continue
            if cohort == '#826-829' and 'baseline_form=type::Function' not in row:
                continue
            if not ('first_blocker=' in row or 'outcome=' in row or
                    (cohort.startswith('#899') and '.scala' in row and '=' in row)):
                continue
            # The identity stops before outcome/projection information; it
            # retains the pinned declaration/method/function tree coordinates.
            if cohort == '#946':
                match = re.search(r'path=(\S+) method=(\S+).*?function_tree=(\d+)', row)
                if not match:
                    raise ValueError(f'unattributed impure row: {row}')
                identity = f'{match[1]} method={match[2]} function_tree={match[3]}'
            elif cohort == '#929':
                match = re.search(r'method=(\S+).*?declaration_tree=(\d+)', row)
                if not match:
                    raise ValueError(f'unattributed local value row: {row}')
                identity = f'method={match[1]} declaration_tree={match[2]}'
            elif cohort.startswith('#899'):
                identity = row.rsplit('=', 1)[0]
            else:
                identity = re.split(r' (?:baseline_form|first_blocker|outcome)=', row, maxsplit=1)[0]
            key = (cohort, identity)
            if key in records:
                raise ValueError(f'duplicate cohort identity: {key}')
            records[key] = row
    signature_paths = {
        'SymUtils.scala': 'compiler/src/dotty/tools/dotc/core/SymUtils.scala',
        'Typer.scala': 'compiler/src/dotty/tools/dotc/typer/Typer.scala',
        'MegaPhase.scala': 'compiler/src/dotty/tools/dotc/transform/MegaPhase.scala',
    }
    attempts_by_id, _ = attempts(text)
    for match in re.finditer(r'\| `([^`]+\.scala)#(\d+) ([^`]+)` \| (?:by-name parameters|parameter modifiers) \| `([^`]+)`', text):
        path = signature_paths[match[1]]
        identity = f'{path}#tree={match[2]}'
        if attempts_by_id:
            if identity not in attempts_by_id:
                raise ValueError(f'missing signature attempt: {identity}')
            row = attempts_by_id[identity][1]
        else:
            row = f'{identity} first_blocker={match[4]} method_typed=false'
        records[('#899', identity)] = row
    return records


def attempts(text):
    by_id, by_site = {}, {}
    for line in sections(text)['local_method_attempts']:
        if match := re.match(r'^  method=(.*?)::site=(.*?)::attempt=(.*?)::blocker=(.*?)::typed=(true|false)$', line):
            by_id[match[1]] = (match[3], line.strip())
            by_site.setdefault(match[2], []).append(match[3])
    return by_id, by_site


def lifecycle(text):
    result = {}
    for line in sections(text)['dependency_load_profile']:
        if match := re.match(r'^    method=(.*?)::shells=\[(.*?)\]::completed=\[(.*?)\]$', line):
            result[match[1]] = (match[2], match[3])
    return result


def matching_methods(row, methods, attempt_ids=None, attempt_sites=None):
    attempt_ids, attempt_sites = attempt_ids or {}, attempt_sites or {}
    identities = []
    if match := re.search(r'([\w/.-]+\.scala)#tree=(\d+)', row):
        identities = [f'{match[1]}#tree={match[2]}']
    elif match := re.search(r'path=(\S+) method=([^# ]+)#tree=(\d+)', row):
        identities = [f'{match[1]}#tree={match[3]}']
    elif match := re.search(r'method=([\w/.-]+\.scala:\d+:\S+)', row):
        identities = attempt_sites.get(match[1], [])
    resolved = []
    for identity in identities:
        parent = attempt_ids.get(identity, (identity, ''))[0]
        resolved.extend(m for m in methods if m.startswith(parent + '::'))
    return sorted(set(resolved))


def cohort_rows(baseline, current):
    old, new = cohort_records(baseline), cohort_records(current)
    current_attempts, _ = attempts(current)
    for key in old:
        if key[0] == '#899' and key[1] in current_attempts:
            new[key] = current_attempts[key[1]][1]
    missing = old.keys() - new.keys()
    if missing:
        raise ValueError(f'missing frozen cohort rows: {sorted(missing)}')
    traces, states = requests(current), lifecycle(current)
    attempt_ids, attempt_sites = attempts(current)
    field_traces = {}
    for lines in sections(current).values():
        for line in lines:
            if match := re.match(r'^    field:(.*?)::targets=\[(.*?)\]::shells=\[(.*?)\]::completed=\[(.*?)\]$', line):
                field_traces[match[1]] = (match[2], match[3], match[4])
    rows = []
    for (cohort, identity), before in sorted(old.items()):
        after = new[(cohort, identity)]
        methods = matching_methods(after, traces, attempt_ids, attempt_sites)
        targets = sorted({f'{target}::outcome={outcome}' for method in methods
                          for target, outcome in traces[method]})
        shells = sorted({name for method in methods for name in states.get(method, ('', ''))[0].split(',') if name})
        completed = sorted({name for method in methods for name in states.get(method, ('', ''))[1].split(',') if name})
        attribution = 'exact' if len(methods) == 1 else 'ambiguous' if methods else 'unobserved'
        if cohort.endswith('-direct'):
            match = re.match(r'([\w/.-]+\.scala): tree=(\d+)', after)
            key = f'{match[1]}#tree={match[2]}' if match else None
            if key in field_traces:
                targets_text, shells_text, completed_text = field_traces[key]
                targets = targets_text.split(' | ') if targets_text else []
                shells = shells_text.split(',') if shells_text else []
                completed = completed_text.split(',') if completed_text else []
                attribution = 'direct-field-probe'
        typed = re.search(r'(?:method_typed=|::typed=)(true|false)', after)
        blocker = re.search(r'(?:current_first_blocker=|first_blocker=|::blocker=)([^ :]+)', after)
        if not blocker:
            blocker = re.search(r'outcome=(?:blocked::)?([^ :]+)', after)
        if not blocker and cohort == '#899':
            blocker = re.search(r'first_blocker=([^ :]+)', after)
        rows.append((cohort, identity, before, after,
                     attribution,
                     ' | '.join(targets), ','.join(shells), ','.join(completed),
                     ' | '.join(t for t in targets if t.endswith(('external class success', 'external non-class success'))),
                     blocker[1] if blocker else 'unknown',
                     typed[1] if typed else 'field-probe' if cohort.endswith('-direct') else 'false'))
    return rows


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('current', type=Path)
    parser.add_argument('--baseline', type=Path, default=Path(__file__).resolve().parents[2] / 'docs/typer-classpath-corpus-audit-976-3.9.0.md')
    parser.add_argument('--rows', type=Path)
    args = parser.parse_args()
    baseline = args.baseline.read_text(encoding='utf-8')
    current = args.current.read_text(encoding='utf-8')
    depth, fanout = gate(baseline, current)
    rows = cohort_rows(baseline, current)
    if args.rows:
        with args.rows.open('w', encoding='utf-8', newline='') as output:
            writer = csv.writer(output, delimiter='\t', lineterminator='\n')
            writer.writerow(('cohort', 'frozen_row', 'baseline_outcome', 'current_outcome',
                             'request_attribution', 'current_resolution_targets',
                             'shells_observed', 'completions_observed', 'resolved_requests',
                             'current_first_blocker', 'full_method_typed'))
            writer.writerows(rows)
    print(f'Gate passed: p95 completion depth {depth}/140; maximum completed fanout {fanout}/1048.')
    print(f'Previously successful target keys retained: {len(successful_targets(baseline))}/{len(successful_targets(current))}.')
    print('Frozen cohort rows:', dict(sorted(collections.Counter(row[0] for row in rows).items())))


if __name__ == '__main__':
    main()
