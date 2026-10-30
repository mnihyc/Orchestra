#!/usr/bin/env python3
"""The release evidence must reject missing, duplicated and another stream's bytes."""
import unittest
from copy import deepcopy
from peer import digest
from verify import verify


class Evidence(unittest.TestCase):
    def fixture(self):
        row = {'seed': 'one', 'expected_bytes': 1024, 'upload_bytes': 1024, 'download_bytes': 1024,
            'download_sha256': digest('one', 'download', 1024), 'cut': False}
        peer = {'one': {**row, 'closed': True, 'upload_sha256': digest('one', 'upload', 1024)}}
        return {'passed': True, 'streams': [row]}, peer

    def test_other_stream_or_direction_is_not_success(self):
        for wrong in [digest('two', 'upload', 1024), digest('one', 'download', 1024)]:
            client, peer = self.fixture()
            peer['one']['upload_sha256'] = wrong
            with self.assertRaisesRegex(ValueError, 'cross-stream'):
                verify(client, peer)

    def test_a_shortened_or_stuck_peer_cannot_be_hidden_by_a_good_client(self):
        for change in [{'upload_bytes': 1023}, {'download_bytes': 1025}, {'send_stuck': True}, {'closed': False}]:
            client, peer = self.fixture()
            peer['one'].update(change)
            with self.assertRaises(ValueError):
                verify(client, peer)

    def test_a_cut_is_distinguished_from_a_complete_transfer(self):
        client, peer = self.fixture()
        verify(client, peer)
        cut = deepcopy(client)
        cut['streams'][0]['cut'] = True
        with self.assertRaisesRegex(ValueError, 'interrupted'):
            verify(cut, peer)
        cut['streams'][0]['download_bytes'] = 512
        verify(cut, peer)


if __name__ == '__main__':
    unittest.main()
