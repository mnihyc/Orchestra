"""Pure exact-image cleanup controls: Docker is always replaced by an in-memory double."""
import json
import subprocess
import unittest
from unittest.mock import patch

from image_cleanup import retire

TAG = 'orchsmokeownedlinkloadnetem'
IMAGE = 'sha256:' + '1' * 64


def inspected(image=IMAGE, run='owned'):
    return json.dumps([{'Id': image, 'Config': {'Labels': {'org.orch.smoke.run': run}}}])


class OwnedImage(unittest.TestCase):
    def test_only_exact_owned_unused_tag_is_removed_without_force_or_prune(self):
        with patch('image_cleanup.docker', side_effect=[inspected(), '', 'Untagged', '']) as docker:
            result = retire(TAG, IMAGE, 'owned')
        self.assertTrue(result['complete'])
        self.assertEqual(docker.call_args_list[2].args, ('image', 'rm', '--no-prune', TAG))

    def test_replaced_image_or_changed_label_is_retained(self):
        for record in (inspected(image='sha256:' + '2' * 64), inspected(run='another'),
                json.dumps([{'Id': IMAGE, 'Config': {'Labels': None}}])):
            with patch('image_cleanup.docker', return_value=record) as docker:
                with self.assertRaises(ValueError):
                    retire(TAG, IMAGE, 'owned')
                self.assertEqual(docker.call_count, 1)

    def test_in_use_image_is_retained(self):
        with patch('image_cleanup.docker', side_effect=[inspected(), 'referencing-container']) as docker:
            with self.assertRaises(ValueError):
                retire(TAG, IMAGE, 'owned')
            self.assertEqual(docker.call_count, 2)

    def test_docker_errors_and_remaining_tag_are_not_success(self):
        for outcomes in ([subprocess.CalledProcessError(1, ['docker'])],
                [inspected(), '', subprocess.CalledProcessError(1, ['docker'])],
                [inspected(), '', 'Untagged', IMAGE]):
            with patch('image_cleanup.docker', side_effect=outcomes):
                with self.assertRaises((ValueError, subprocess.SubprocessError)):
                    retire(TAG, IMAGE, 'owned')

    def test_partial_build_does_not_guess_an_image_identity(self):
        with patch('image_cleanup.docker', return_value=IMAGE) as docker:
            with self.assertRaises(ValueError):
                retire(TAG, '', 'owned')
            self.assertEqual(docker.call_count, 1)
        with patch('image_cleanup.docker', return_value='') as docker:
            result = retire(TAG, '', 'owned')
            self.assertEqual(result['outcome'], 'build_created_no_tag')
            self.assertEqual(docker.call_count, 1)


if __name__ == '__main__':
    unittest.main()
