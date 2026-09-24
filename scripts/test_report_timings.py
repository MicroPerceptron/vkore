import importlib.util
from pathlib import Path
import unittest
import json
import subprocess
import sys
import tempfile

spec = importlib.util.spec_from_file_location('report_timings', Path(__file__).with_name('report-timings.py'))
report = importlib.util.module_from_spec(spec)
spec.loader.exec_module(report)


class ReportTests(unittest.TestCase):
    @staticmethod
    def event(sequence, kind='validation', payload=None, **extra):
        return {'sequence': sequence, 'prompt_id': 'p', 'timestamp_unix_ms': 1000 + sequence,
                'kind': kind, 'public': False,
                'payload': {'runtime_elapsed_ms': sequence, **(payload or {})}, **extra}

    def test_current_readiness_milestone_is_not_inferred_from_legacy_teardown(self):
        events = [
            self.event(0, payload={'stage': 'runtime_prompt_started'}),
            self.event(1, 'assistant', public=True, payload={'text': 'done'}),
            self.event(2, 'capability', payload={'state': 'Teardown', 'label': 'agent/frontend'}),
            self.event(3, payload={'stage': 'kore_response_published'}),
            self.event(5, payload={'stage': 'runtime_prompt_finished'}),
        ]
        phases = report.analyze(events)['phases']
        self.assertFalse(any('readiness confirmed' in p['to'] for p in phases))
        events.insert(-1, self.event(4, payload={'stage': 'kore_frontend_ready'}))
        phases = report.analyze(events)['phases']
        self.assertTrue(any('readiness confirmed' in p['to'] for p in phases))

    def test_overlapping_tool_calls_match_route_generation_and_call(self):
        def tool(sequence, kind, route, generation, request):
            return self.event(sequence, kind, {'role': 'Agent', 'route': route,
                'generation': generation, 'request': request,
                'payload': json.dumps({'call_id': 1})})
        events = [
            tool(1, 'tool_call', 'A', 1, 'request-1'),
            tool(2, 'tool_call', 'B', 1, 'request-2'),
            tool(3, 'tool_result', 'A', 2, 'wrong-generation'),
            tool(4, 'tool_result', 'B', 1, 'request-3'),
            tool(5, 'tool_result', 'A', 1, 'request-4'),
        ]
        tools = report.analyze(events)['tools']
        self.assertEqual([(t['route'], t['duration_ms']) for t in tools], [('B', 2), ('A', 4)])

    def test_missing_or_duplicate_tool_identity_stays_unmeasured(self):
        payload = {'role': 'Agent', 'route': 'A', 'generation': 1,
                   'payload': json.dumps({'call_id': 1})}
        events = [self.event(1, 'tool_call', payload), self.event(2, 'tool_call', payload),
                  self.event(3, 'tool_result', payload),
                  self.event(4, 'tool_call', {'payload': '{"call_id":2}'}),
                  self.event(5, 'tool_result', {'payload': '{"call_id":2}'})]
        self.assertEqual(report.analyze(events)['tools'], [])

    def test_report_cli_keeps_sessions_separate_and_writes_both_formats(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            events = []
            for index, session in enumerate(['first', 'second']):
                events.extend([
                    self.event(index * 3, session_id=session,
                               payload={'stage': 'runtime_prompt_started'}),
                    self.event(index * 3 + 1, session_id=session,
                               payload={'stage': 'runtime_prompt_finished'}),
                    self.event(index * 3 + 2, 'prompt_terminal', session_id=session,
                               payload={'status': 'failed'}),
                ])
            trace = root / 'events.jsonl'
            trace.write_text(''.join(json.dumps(event) + '\n' for event in events))
            subprocess.run([sys.executable, str(Path(report.__file__)), str(trace),
                            '--markdown', str(root / 'timings.md'),
                            '--json', str(root / 'timings.json')], check=True)
            reports = json.loads((root / 'timings.json').read_text())
            self.assertEqual(len(reports), 2)
            self.assertTrue(all(item['status'] == 'failed' for item in reports))
            self.assertTrue(all(item['response_ms'] is None for item in reports))
            self.assertIn('Status: **failed**', (root / 'timings.md').read_text())

    def test_cumulative_sse_timings_are_not_added(self):
        self.assertEqual(report.model_timings(
            '{"timings":{"predicted_ms":100}}\n'
            '{"timings":{"predicted_ms":200}}\n'
            '{"incomplete"'), {'predicted_ms': 200})

    def test_failed_startup_does_not_invent_response_or_model_time(self):
        events = [
            {'sequence': 1, 'prompt_id': 'p', 'timestamp_unix_ms': 1000,
             'kind': 'validation', 'public': False,
             'payload': {'stage': 'runtime_prompt_started', 'runtime_elapsed_ms': 0}},
            {'sequence': 2, 'prompt_id': 'p', 'timestamp_unix_ms': 1100,
             'kind': 'error', 'public': True,
             'payload': {'message': 'startup failed', 'runtime_elapsed_ms': 100}},
            {'sequence': 3, 'prompt_id': 'p', 'timestamp_unix_ms': 1101,
             'kind': 'validation', 'public': False,
             'payload': {'stage': 'runtime_prompt_finished', 'runtime_elapsed_ms': 101}},
        ]
        result = report.analyze(events)
        self.assertIsNone(result['response_ms'])
        self.assertEqual(result['model_attempts'], [])
        self.assertEqual(result['errors'], ['startup failed'])
        self.assertEqual(sum(p['duration_ms'] for p in result['phases']), 101)


if __name__ == '__main__':
    unittest.main()
