# Shared-work port probes

Run these disposable programs inside the selected test environment for the
joined Ports walk. The integration runner creates the openings and caller
sign-ins and records the actual program/browser outcomes; the writer starts
no listener. Each program binds only the requested address/port and prints
safe peer/attribution facts, never a signed assertion or credential.

```sh
python3 scripts/smoke/team-work/ports_probe.py --mode tcp --port 41881
python3 scripts/smoke/team-work/ports_probe.py --mode udp --port 41882
python3 scripts/smoke/team-work/ports_probe.py --mode tcp --proxy-v2 --port 41883
```

The ordinary TCP probe echoes bytes exactly, and the UDP probe includes empty
datagrams. The dedicated PROXY v2 probe prints the injected caller first and
then echoes the ordinary caller payload. Use separate inside ports for framed
and unframed TCP. Compare these logged inside peers with the current connection
list before attributing a caller.

For HTTP, fetch the candidate's public `/api/v1/ports/assertion-keys` response
into a public-key JSON file and use the issuer, environment id, current runtime
generation and assertion-header name the candidate declares:

```sh
uv run --with cryptography python scripts/smoke/team-work/ports_probe.py --mode http --port 41884 --keys ./public-port-keys.json --issuer https://orch.example --environment ENVIRONMENT_ID --generation 1 --assertion-header HEADER_NAME
```

Replace the example values with the selected target's actual facts. Signature,
algorithm/type, key identity, issuer, exact target, generation and live time must
all hold before HTTP returns a verified caller. It returns 403 for a wrong or
expired assertion. Use the candidate's existing link/helper and host mappings
to exercise both raw transports, then revoke the opening and observe admission
end. A working echo alone does not prove its audience or signed attribution.
