"""Pure exact-backplane/profile controls; no kernel or namespace execution."""
from copy import deepcopy
import unittest

from placement import verify, verify_profile


def facts():
    placement = {'placement': 'backplane', 'gateway_port': 41401, 'backplane_ip': '172.30.0.2', 'backplane_port': 8444, 'connector_process': 11}
    connector = {'network_namespace': 'net:[41]', 'observed_process_name': 'orch-connector',
        'connector_processes': [{'pid': 11}],
        'gateway_tcp_connections': [{'remote_port': 41401, 'inode': '123', 'process_ids': [11]}]}
    gateway = {'network_namespace': 'net:[42]', 'observed_process_name': 'orch-gateway',
        'gateway_tcp_connections': [{'remote_ip': '172.30.0.2', 'remote_port': 8444, 'process_ids': [1]}]}
    netem = {'kind': 'netem', 'bytes': 10000, 'packets': 100, 'drops': 1}
    upload = [{'kind': 'prio', 'handle': '1:', 'options': {'bands': 3, 'priomap': [0] * 16}}, netem]
    egress = [{'kind': 'flower', 'options': {'keys': {'dst_ip': '172.30.0.2/32', 'ip_proto': 'tcp', 'dst_port': 8444}, 'classid': '1:3'}}]
    ingress = [{'kind': 'flower', 'options': {'keys': {'src_ip': '172.30.0.2/32', 'ip_proto': 'tcp', 'src_port': 8444},
        'actions': [{'kind': 'mirred', 'to_dev': 'ifb0', 'direction': 'egress', 'eaction': 'redirect'}]}}]
    return [placement, 'net:[42]', connector, upload, [netem], gateway, egress, ingress]


class Backplane(unittest.TestCase):
    def test_actual_separate_gateway_socket_exact_filters_and_bidirectional_loss(self):
        result = verify(*facts())
        self.assertEqual(result['placement'], 'backplane')
        self.assertFalse(result['complete_wss_release_gate'])

    def test_wrong_namespace_address_port_owner_or_broad_filter_is_not_proof(self):
        for change in ('namespace', 'address', 'port', 'owner', 'prefix', 'default_band', 'redirect'):
            value = deepcopy(facts())
            if change == 'namespace': value[1] = 'net:[41]'
            if change == 'address': value[5]['gateway_tcp_connections'][0]['remote_ip'] = '172.30.0.3'
            if change == 'port': value[5]['gateway_tcp_connections'][0]['remote_port'] = 8443
            if change == 'owner': value[5]['gateway_tcp_connections'][0]['process_ids'] = []
            if change == 'prefix': value[6][0]['options']['keys']['dst_ip'] = '172.30.0.2/24'
            if change == 'default_band': value[3][0]['options']['priomap'][0] = 2
            if change == 'redirect': value[7][0]['options']['actions'][0]['to_dev'] = 'eth0'
            with self.assertRaises(ValueError): verify(*value)

    def test_same_namespace_socket_requires_exact_connector_pid_inode_join(self):
        for change in ('unowned', 'other_pid', 'missing_inode', 'zero_inode', 'other_process', 'ambiguous'):
            value = deepcopy(facts())
            value[0]['placement'] = 'connector'; value[1] = 'net:[41]'
            row = value[2]['gateway_tcp_connections'][0]
            if change == 'unowned': row['process_ids'] = []
            if change == 'other_pid': row['process_ids'] = [12]
            if change == 'missing_inode': row.pop('inode')
            if change == 'zero_inode': row['inode'] = '0'
            if change == 'other_process': value[0]['connector_process'] = 12
            if change == 'ambiguous': value[2]['connector_processes'].append({'pid': 12})
            with self.assertRaises(ValueError): verify(*value)
        value = facts(); value[0]['placement'] = 'connector'; value[1] = 'net:[41]'
        self.assertEqual(verify(*value)['placement'], 'connector')

    def test_actual_profile_text_preserves_existing_rates_delay_loss_and_limit(self):
        for rate in (750000, 1600000, 75000, 160000):
            line = f'qdisc netem 30: parent 1:3 limit 1024 delay 150ms loss 1% rate {rate}bit'
            verify_profile(line, rate)
            for changed in (line.replace('150ms', '300ms'), line.replace('1%', '100%'),
                    line.replace('1024', '2048'), line.replace(str(rate) + 'bit', '999bit')):
                with self.assertRaises(ValueError): verify_profile(changed, rate)


if __name__ == '__main__':
    unittest.main()
