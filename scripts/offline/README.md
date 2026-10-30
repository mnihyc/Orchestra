# Carrying a release into an internal network

Run `gather.sh --help` on the connected side and `install.sh --help` on the internal side. Help includes English and Chinese. The [operator guide](../../docs/operator/offline.md) covers prerequisites, trust, install/update and internal sources.

The behavioral checks use local fixtures without contacting a vendor or starting Docker:

```bash
python3 -I -B -m unittest discover -s scripts/offline -p 'test_*.py'
```
