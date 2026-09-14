import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location('report_timings', Path(__file__).with_name('report-timings.py'))
report = importlib.util.module_from_spec(spec)
spec.loader.exec_module(report)


class ReportTests(unittest.TestCase):
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
