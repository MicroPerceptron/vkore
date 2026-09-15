#!/usr/bin/env python3
"""Report host-observed prompt phases from a vkore chat --trace-file JSONL."""

import argparse
from collections import defaultdict
import json
import re
from datetime import datetime
from pathlib import Path


def model_timings(payload):
    """SSE timings are cumulative: retain the last complete sample, never sum deltas."""
    latest = None
    for line in payload.splitlines():
        try:
            item = json.loads(line.removeprefix('data: ').strip())
        except (ValueError, TypeError):
            continue
        if isinstance(item, dict) and isinstance(item.get('timings'), dict):
            latest = item['timings']
    return latest


def tool_identity(payload):
    """Tool results arrive on a later inference request; match the call's route."""
    try:
        body = json.loads(payload.get('payload', ''))
    except (ValueError, TypeError):
        return None
    if not isinstance(body, dict):
        return None
    call_id = body.get('call_id')
    route = payload.get('route')
    generation = payload.get('generation')
    role = payload.get('role')
    if (type(call_id) is not int or call_id <= 0
            or not isinstance(route, str) or not route
            or not isinstance(role, str) or not role
            or type(generation) is not int or generation <= 0):
        return None
    return role, route, generation, call_id


def analyze(events):
    events = sorted(events, key=lambda e: e['sequence'])
    timed = [e for e in events if 'runtime_elapsed_ms' in e['payload']]
    if not timed:
        return {'error': 'No runtime timing events; rebuild the instrumented harness.'}
    def time(e):
        return e['payload']['runtime_elapsed_ms']
    def first(predicate):
        return next((e for e in timed if predicate(e)), None)
    def stage(name):
        return first(lambda e: e['payload'].get('stage') == name)
    def obs(role, kind):
        return first(lambda e: e['payload'].get('role') == role and e['payload'].get('kind') == kind)
    milestones = []
    def add(label, event):
        if event is not None:
            milestones.append((time(event), event['sequence'], label))
    add('Runtime begins', stage('runtime_prompt_started'))
    for name, label in [
        ('model_server_starting', 'Model server startup begins'),
        ('model_server_ready', 'Model server ready'),
        ('workspace_open_starting', 'Workspace initialization begins'),
        ('workspace_open_ready', 'Workspace ready'),
        ('hostlink_bind_starting', 'HostLink listener setup begins'),
        ('hostlink_bind_ready', 'HostLink listeners ready'),
        ('runtime_ready', 'Project runtime ready'),
        ('vm_spawn_starting', 'VM spawn begins'),
        ('vm_process_spawned', 'QEMU process spawned'),
        ('hostlink_epoch_established', 'Guest HostLink connected'),
        ('kore_broker_claimed_intent', 'Broker claimed prompt'),
    ]:
        add(label, stage(name))
    add('Broker model request ready', obs('Broker', 'ModelPrompt'))
    add('Broker decision validated', obs('Broker', 'BrokerProposal'))
    add('First authority event', first(lambda e: e['kind'] == 'capability' and e['payload'].get('state') == 'Installed'))
    add('Worker claimed prompt', stage('kore_agent_claimed_intent'))
    add('Worker first model request ready', obs('Agent', 'ModelPrompt'))
    add('Worker final answer validated', obs('Agent', 'AssistantMessage'))
    add('Public response emitted', first(lambda e: e['kind'] == 'assistant' and e['public']))
    add('Failure reported', first(lambda e: e['kind'] == 'error' and e['public']))
    add('Response publication confirmed', stage('kore_response_published'))
    add('Native retirement and broker readiness confirmed', stage('kore_frontend_ready'))
    add('Failed guest shutdown begins', stage('failed_guest_shutdown'))
    add('Runtime finished', stage('runtime_prompt_finished'))
    add('Timing summary recorded', stage('runtime_timing_summary'))
    milestones.sort()
    phases = [{'from': a[2], 'to': b[2], 'start_ms': a[0], 'end_ms': b[0], 'duration_ms': b[0]-a[0]}
              for a, b in zip(milestones, milestones[1:])]
    attempts, pending, tools, pending_tools = [], {}, [], {}
    for e in timed:
        p = e['payload']
        key = (p.get('role'), p.get('route'), p.get('generation'), p.get('request'), p.get('turn'))
        kind = p.get('kind')
        if kind == 'ModelPrompt':
            if key in pending:
                pending[key]['superseded_without_completion'] = True
            record = {'role': p.get('role'), 'turn': p.get('turn'), 'request': p.get('request'),
                      'start_ms': time(e), 'first_chunk_ms': None, 'end_ms': None}
            attempts.append(record)
            pending[key] = record
        elif kind == 'ModelOutputChunk' and key in pending:
            record = pending[key]
            if record['first_chunk_ms'] is None:
                record['first_chunk_ms'] = time(e)
        elif kind == 'RawModelCompletion' and key in pending:
            pending[key]['end_ms'] = time(e)
            pending[key]['rejected'] = 'rejected' in p.get('detail', '')
        elif kind == 'ReturnedModelFields' and key in pending:
            pending[key]['server_timings'] = model_timings(p.get('payload', ''))
            pending[key]['server_capture_truncated'] = 'capture truncated' in p.get('detail', '')
        if e['kind'] == 'tool_call':
            identity = tool_identity(p)
            if identity is not None:
                # Duplicate calls make the duration ambiguous; do not pick one.
                pending_tools[identity] = (time(e), p.get('payload', '')) if identity not in pending_tools else None
        elif e['kind'] == 'tool_result':
            identity = tool_identity(p)
            matched = pending_tools.pop(identity, None)
            if matched is not None:
                start, call = matched
                tools.append({'start_ms': start, 'end_ms': time(e), 'duration_ms': time(e)-start,
                              'role': identity[0], 'route': identity[1], 'generation': identity[2],
                              'call_id': identity[3], 'call': call[:200]})
    for attempt in attempts:
        if attempt['end_ms'] is not None:
            attempt['wall_ms'] = attempt['end_ms'] - attempt['start_ms']
        if attempt['first_chunk_ms'] is not None:
            attempt['first_chunk_wait_ms'] = attempt['first_chunk_ms'] - attempt['start_ms']
    response = first(lambda e: e['kind'] == 'assistant' and e['public'])
    user = next((e for e in events if e['kind'] == 'user_prompt'), None)
    terminal = next((e for e in events if e['kind'] == 'prompt_terminal'), None)
    summary = stage('runtime_timing_summary')
    boot = [{'observed_ms': time(e), **{k: e['payload'].get(k) for k in ('marker','detail')}}
            for e in timed if e['payload'].get('stage') == 'guest_boot_observation']
    paused, pause_start = [], None
    for e in timed:
        name = e['payload'].get('stage', '')
        if name == 'host_inference_vm_suspended':
            pause_start = time(e)
        elif name == 'host_inference_vm_resumed' and pause_start is not None:
            paused.append({'start_ms': pause_start, 'end_ms': time(e), 'duration_ms': time(e)-pause_start})
            pause_start = None
    total = time(summary or timed[-1])
    result = {'prompt_id': events[0].get('prompt_id'), 'text': user['payload'].get('text') if user else None,
              'status': terminal['payload'].get('status') if terminal else 'incomplete',
              'runtime_total_ms': total, 'response_ms': time(response) if response else None,
              'phases': phases, 'model_attempts': attempts, 'tools': tools,
              'boot_observations': boot, 'vm_pause_intervals': paused,
              'event_publish_ms': summary['payload'].get('event_publish_ms') if summary else None,
              'loop_totals': stage('runtime_loop_totals')['payload'] if stage('runtime_loop_totals') else None,
              'errors': [e['payload'].get('message') for e in timed if e['kind'] == 'error']}
    result['start_unix_ms'] = timed[0]['timestamp_unix_ms']
    result['end_unix_ms'] = timed[-1]['timestamp_unix_ms']
    if user:
        result['queue_to_runtime_ms'] = timed[0]['timestamp_unix_ms'] - user['timestamp_unix_ms']
    if response:
        if user:
            result['submit_event_to_response_ms'] = response['timestamp_unix_ms'] - user['timestamp_unix_ms']
        result['client_response_ms'] = response.get('client_elapsed_ms')
        if 'client_received_unix_ms' in response:
            result['response_event_to_client_ms'] = response['client_received_unix_ms']-response['timestamp_unix_ms']
    return result


def markdown(reports, source):
    lines = ['# vKore measured request timings', '', f'Trace: `{source}`', '',
             'Durations use the host monotonic clock. Milestones are observed at the host; they include guest scheduling and transport. Serial boot markers are polled every 10 ms and may be buffered. Model timings come from llama.cpp when its bounded trace includes them. First-chunk observation can be delayed by worker buffering; it is not a model time-to-first-token measurement.', '',
             'The phase table is sequential. Model, tool, pause, boot-detail and event-publication measurements overlap those phases; do not add them again. Client delivery is measured at the CLI, not native-window paint.', '']
    def seconds(ms):
        return 'unavailable' if ms is None else f'{ms / 1000:.3f} s'
    for report in reports:
        if 'error' in report:
            lines += [report['error'], '']; continue
        lines += [f"## {report['prompt_id']}: {report['text']}", '',
                  f"Status: **{report['status']}**. Public response: **{seconds(report['response_ms'])}** from runtime start. Runtime complete: **{seconds(report['runtime_total_ms'])}**.", '',
                  f"Queue to runtime: {seconds(report.get('queue_to_runtime_ms'))}. Client time to response: {seconds(report.get('client_response_ms'))}. Response event to client: {seconds(report.get('response_event_to_client_ms'))}.", '',
                  '| From | To | Duration |', '|---|---|---:|']
        for phase in report['phases']:
            lines.append(f"| {phase['from']} | {phase['to']} | {seconds(phase['duration_ms'])} |")
        lines += ['', '### Model calls (overlap phases above)', '',
                  '| Role / turn | Observed call | First chunk observed | Server prefill | Server generation | Output tokens |',
                  '|---|---:|---:|---:|---:|---:|']
        for attempt in report['model_attempts']:
            st = attempt.get('server_timings') or {}
            lines.append(f"| {attempt['role']} / {attempt['turn']}{' (rejected)' if attempt.get('rejected') else ''} | {seconds(attempt.get('wall_ms'))} | {seconds(attempt.get('first_chunk_wait_ms'))} | {seconds(st.get('prompt_ms'))} | {seconds(st.get('predicted_ms'))} | {st.get('predicted_n', 'unavailable')} |")
        lines += ['', '### Boot/service observations', '', '| Observed after runtime start | Marker |', '|---:|---|']
        for item in report['boot_observations']:
            lines.append(f"| {seconds(item['observed_ms'])} | {re.sub(r'\x1b\[[0-9;=?]*[A-Za-z]', '', item['detail']).replace('|', '/')} |")
        if not report['boot_observations']:
            lines.append('| — | No new boot markers observed (VM may be reused). |')
        lines += ['', '### Other overhead', '', f"- Synchronous event publication: {seconds(report['event_publish_ms'])} (included above)."]
        loop = report['loop_totals']
        if loop:
            lines.append(f"- Host loop: {loop['ticks']} passes; {seconds(loop['tick_ms'])} inside daemon processing, largest pass {seconds(loop['max_tick_ms'])}, {seconds(loop['sleep_ms'])} in polling sleeps. These are overlapping duty measurements, not pure transport latency.")
        for tool in report['tools']:
            lines.append(f"- Tool call to result observed: {seconds(tool['duration_ms'])}; `{tool['call'].replace('`', '')}`.")
        for interval in report['vm_pause_intervals']:
            lines.append(f"- VM paused during inference: {seconds(interval['duration_ms'])} (overlaps model time).")
        for startup in report.get('server_startups', []):
            lines.append(f"- Model server started/restarted at +{seconds(startup['relative_ms'])}; loading/readiness {seconds(startup['duration_ms'])} (overlaps phases above).")
        for error in report['errors']:
            lines.append(f'- Error: {error}')
        lines.append('')
    return '\n'.join(lines)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('trace', type=Path)
    parser.add_argument('--markdown', type=Path)
    parser.add_argument('--json', type=Path)
    args = parser.parse_args()
    grouped = defaultdict(list)
    for line in args.trace.read_text().splitlines():
        event = json.loads(line)
        if event.get('prompt_id'):
            grouped[(event['session_id'], event['prompt_id'])].append(event)
    reports = [analyze(events) for events in grouped.values()]
    log = args.trace.parent / 'harness.log'
    if log.exists():
        pending_start = None
        for line in log.read_text(errors='replace').splitlines():
            match = re.match(r'\[([^]]+)\].*component=llama event=(starting|ready)(.*)', line)
            if not match:
                continue
            stamp = datetime.fromisoformat(match[1]).timestamp() * 1000
            if match[2] == 'starting':
                pending_start = stamp
            elif pending_start is not None:
                elapsed = re.search(r'elapsed_ms=(\d+)', match[3])
                if elapsed:
                    for report in reports:
                        if report.get('start_unix_ms', float('inf')) <= pending_start <= report.get('end_unix_ms', 0):
                            report.setdefault('server_startups', []).append({
                                'relative_ms': pending_start-report['start_unix_ms'],
                                'duration_ms': int(elapsed[1])})
                pending_start = None
    output = markdown(reports, args.trace)
    if args.markdown:
        args.markdown.write_text(output)
    else:
        print(output)
    if args.json:
        args.json.write_text(json.dumps(reports, indent=2) + '\n')


if __name__ == '__main__':
    main()
